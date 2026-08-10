//! OMT receiver (sync).

use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use crate::codec::fpa1;
use crate::discovery::address::OmtAddress;
use crate::error::OmtError;
use crate::protocol::frame::{AssembledFrame, FrameHeader, PROTOCOL_VERSION};
use crate::protocol::metadata::{
    SUBSCRIBE_AUDIO, SUBSCRIBE_METADATA, SUBSCRIBE_VIDEO, encode_metadata_xml,
    suggested_quality_xml,
};
use crate::transport::channel::Channel;
use crate::transport::socket::connect;
use crate::types::{
    Codec, FrameType, MediaFrame, PreferredVideoFormat, Quality, ReceiveFlags, Statistics,
};

/// Receives frames from an OMT source (dual connections for A/V + metadata).
pub struct Receiver {
    address: String,
    parsed: OmtAddress,
    frame_types: FrameType,
    preferred_format: PreferredVideoFormat,
    flags: ReceiveFlags,
    suggested_quality: Quality,
    stats: Statistics,
    av_stream: Option<TcpStream>,
    /// Second TCP session used for audio when video is also requested (libomtnet layout).
    meta_stream: Option<TcpStream>,
    av_channel: Channel,
    /// Reassembly buffer for the dedicated audio socket.
    meta_channel: Channel,
    subscribed: bool,
    /// Cached VMX decoder; reused while `codec.size()` matches the frame.
    vmx_codec: Option<vmx::Codec>,
    /// Reused decode output buffer.
    vmx_decode_buf: Vec<u8>,
}

impl Receiver {
    /// Create a receiver for `address` (e.g. `omt://host:port/Source`).
    pub fn create(address: impl Into<String>, frame_types: FrameType) -> Result<Self, OmtError> {
        let address = address.into();
        if address.is_empty() {
            return Err(OmtError::InvalidArgument(
                "receiver address is empty".into(),
            ));
        }
        let parsed = OmtAddress::from_url(&address)?;
        Ok(Self {
            address,
            parsed,
            frame_types,
            preferred_format: PreferredVideoFormat::Uyvy,
            flags: ReceiveFlags::NONE,
            suggested_quality: Quality::Default,
            stats: Statistics::default(),
            av_stream: None,
            meta_stream: None,
            av_channel: Channel::new(FrameType::VIDEO | FrameType::AUDIO),
            meta_channel: Channel::new(FrameType::AUDIO),
            subscribed: false,
            vmx_codec: None,
            vmx_decode_buf: Vec::new(),
        })
    }

    /// Connection address string.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Configured frame types.
    pub fn frame_types(&self) -> FrameType {
        self.frame_types
    }

    /// Set preferred uncompressed video format.
    pub fn set_preferred_format(&mut self, format: PreferredVideoFormat) {
        self.preferred_format = format;
    }

    /// Preferred format.
    pub fn preferred_format(&self) -> PreferredVideoFormat {
        self.preferred_format
    }

    /// Set receive flags.
    pub fn set_flags(&mut self, flags: ReceiveFlags) {
        self.flags = flags;
    }

    /// Suggest quality to the sender.
    pub fn set_suggested_quality(&mut self, quality: Quality) {
        self.suggested_quality = quality;
    }

    /// Attempt to connect dual TCP sessions and send subscribe commands.
    pub fn connect(&mut self, timeout: Option<Duration>) -> Result<(), OmtError> {
        let host = self
            .parsed
            .addresses
            .first()
            .cloned()
            .unwrap_or_else(|| "127.0.0.1".into());
        let port = if self.parsed.port == 0 {
            return Err(OmtError::InvalidArgument(
                "receiver URL must include a port".into(),
            ));
        } else {
            self.parsed.port
        };
        let addr: SocketAddr = format!("{host}:{port}")
            .parse()
            .map_err(|e| OmtError::InvalidArgument(format!("bad address: {e}")))?;

        // libomtnet: one TCP for video/metadata, a second only for audio.
        let want_video_or_meta = self.frame_types.contains(FrameType::VIDEO)
            || self.frame_types.contains(FrameType::METADATA)
            || self.frame_types == FrameType::NONE;
        let want_audio = self.frame_types.contains(FrameType::AUDIO);

        if want_video_or_meta {
            let av = connect(addr, timeout)?;
            self.av_stream = Some(av);
        }
        if want_audio {
            // Audio uses the metadata stream slot when video is also present;
            // for audio-only, use av_stream as the audio channel.
            if self.av_stream.is_some() {
                let meta = connect(addr, timeout)?;
                self.meta_stream = Some(meta);
            } else {
                let av = connect(addr, timeout)?;
                self.av_stream = Some(av);
            }
        }
        // Keep a second connection for metadata-only quality/tally side-channel
        // when video is requested without audio (matches historical dual-socket scaffold).
        // Prefer libomtnet behavior: everything on the video socket.
        self.send_subscriptions()?;
        self.subscribed = true;
        Ok(())
    }

    fn send_subscriptions(&mut self) -> Result<(), OmtError> {
        let quality_xml = suggested_quality_xml(self.suggested_quality);
        // libomtnet OMTReceive ConnectionCompleted(Video):
        //   SUBSCRIBE_METADATA, [PREVIEW], SUBSCRIBE_VIDEO, suggested quality, tally
        // all on the video socket.
        if let Some(stream) = self.av_stream.as_mut() {
            if self.frame_types.contains(FrameType::VIDEO)
                || self.frame_types.contains(FrameType::METADATA)
            {
                write_metadata_frame(stream, SUBSCRIBE_METADATA)?;
            }
            if self.flags.contains(ReceiveFlags::PREVIEW) {
                write_metadata_frame(stream, crate::protocol::metadata::PREVIEW_ON)?;
            }
            if self.frame_types.contains(FrameType::VIDEO) {
                write_metadata_frame(stream, SUBSCRIBE_VIDEO)?;
                write_metadata_frame(stream, &quality_xml)?;
            }
            // Audio-only uses this same socket.
            if self.frame_types.contains(FrameType::AUDIO) && self.meta_stream.is_none() {
                write_metadata_frame(stream, SUBSCRIBE_AUDIO)?;
            }
            stream.flush()?;
        }
        // Separate audio socket (libomtnet audio connection).
        if let Some(stream) = self.meta_stream.as_mut()
            && self.frame_types.contains(FrameType::AUDIO)
        {
            write_metadata_frame(stream, SUBSCRIBE_AUDIO)?;
            stream.flush()?;
        }
        Ok(())
    }

    /// Receive the next frame, reconnecting on hard errors when possible.
    ///
    /// When a dedicated audio socket is open, both sockets are polled so audio
    /// frames are not starved behind video.
    pub fn receive(&mut self, timeout_ms: i32) -> Result<Option<ReceivedFrame>, OmtError> {
        if self.av_stream.is_none() && self.meta_stream.is_none() {
            let t = if timeout_ms < 0 {
                None
            } else {
                Some(Duration::from_millis(timeout_ms as u64))
            };
            self.connect(t)?;
        }

        let deadline = if timeout_ms < 0 {
            None
        } else {
            Some(Instant::now() + Duration::from_millis(timeout_ms as u64))
        };

        loop {
            if self.av_stream.is_none() && self.meta_stream.is_none() {
                return Ok(None);
            }

            // Drain audio first (short poll) so tones stay live under video load.
            match self.poll_one(true, Duration::from_millis(1))? {
                Some(frame) => return Ok(Some(frame)),
                None => {}
            }

            let slice = match deadline {
                Some(d) => {
                    let remaining = d.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Ok(None);
                    }
                    remaining
                        .min(Duration::from_millis(50))
                        .max(Duration::from_millis(1))
                }
                None => Duration::from_millis(50),
            };

            match self.poll_one(false, slice)? {
                Some(frame) => return Ok(Some(frame)),
                None => {
                    if deadline.is_some_and(|d| Instant::now() >= d) {
                        return Ok(None);
                    }
                }
            }
        }
    }

    /// Poll either the audio (`meta`) or A/V socket once.
    ///
    /// Returns `Ok(Some(frame))` on success, `Ok(None)` on idle/timeout, and
    /// clears both streams when a socket closes.
    fn poll_one(
        &mut self,
        audio: bool,
        timeout: Duration,
    ) -> Result<Option<ReceivedFrame>, OmtError> {
        if audio {
            if self.meta_stream.is_none() {
                return Ok(None);
            }
        } else if self.av_stream.is_none() {
            return Ok(None);
        }

        let result = if audio {
            let stream = self.meta_stream.as_mut().unwrap();
            let _ = stream.set_read_timeout(Some(timeout));
            self.meta_channel.recv_frame(stream)
        } else {
            let stream = self.av_stream.as_mut().unwrap();
            let _ = stream.set_read_timeout(Some(timeout));
            self.av_channel.recv_frame(stream)
        };

        match result {
            Ok(Some(frame)) => {
                let nbytes = frame.to_bytes().len();
                self.stats.record_received(nbytes);
                Ok(Some(decode_received(
                    frame,
                    self.flags,
                    self.preferred_format,
                    &mut self.vmx_codec,
                    &mut self.vmx_decode_buf,
                )?))
            }
            Ok(None) => {
                self.av_stream = None;
                self.meta_stream = None;
                self.subscribed = false;
                Ok(None)
            }
            Err(OmtError::Io(ref e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                Ok(None)
            }
            Err(OmtError::Network(_)) | Err(OmtError::Io(_)) => {
                self.av_stream = None;
                self.meta_stream = None;
                self.subscribed = false;
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// Feed raw bytes into the A/V reassembly buffer (tests / custom IO).
    pub fn push_av_bytes(&mut self, data: &[u8]) -> Result<Option<ReceivedFrame>, OmtError> {
        self.av_channel.push_bytes(data);
        match self.av_channel.try_pop_frame()? {
            Some(frame) => {
                let nbytes = frame.to_bytes().len();
                self.stats.record_received(nbytes);
                Ok(Some(decode_received(
                    frame,
                    self.flags,
                    self.preferred_format,
                    &mut self.vmx_codec,
                    &mut self.vmx_decode_buf,
                )?))
            }
            None => Ok(None),
        }
    }

    /// Snapshot of receive statistics.
    pub fn statistics(&self) -> Statistics {
        self.stats
    }
}

fn write_metadata_frame(stream: &mut TcpStream, xml: &str) -> Result<(), OmtError> {
    let data = encode_metadata_xml(xml);
    let frame = AssembledFrame {
        header: FrameHeader {
            version: PROTOCOL_VERSION,
            frame_type: FrameType::METADATA,
            timestamp: 0,
            metadata_length: 0,
            data_length: data.len() as i32,
        },
        video: None,
        audio: None,
        data,
        metadata: Vec::new(),
    };
    stream.write_all(&frame.to_bytes())?;
    Ok(())
}

fn decode_received(
    frame: AssembledFrame,
    flags: ReceiveFlags,
    preferred: PreferredVideoFormat,
    vmx_codec: &mut Option<vmx::Codec>,
    vmx_decode_buf: &mut Vec<u8>,
) -> Result<ReceivedFrame, OmtError> {
    let metadata = if frame.metadata.is_empty() {
        None
    } else {
        Some(crate::protocol::metadata::decode_metadata_xml(
            &frame.metadata,
        )?)
    };

    let frame_data = frame.data;
    let mut media = MediaFrame {
        frame_type: frame.header.frame_type,
        timestamp: frame.header.timestamp,
        data: Vec::new(),
        frame_metadata: metadata.clone(),
        ..MediaFrame::default()
    };

    if let Some(v) = frame.video {
        media.codec = v.codec.as_i32();
        media.width = v.width;
        media.height = v.height;
        media.frame_rate_n = v.frame_rate_n;
        media.frame_rate_d = v.frame_rate_d;
        media.aspect_ratio = v.aspect_ratio;
        media.flags = v.flags;
        media.color_space = v.color_space;

        if v.codec == Codec::Vmx1 {
            if flags.contains(ReceiveFlags::INCLUDE_COMPRESSED)
                || flags.contains(ReceiveFlags::COMPRESSED_ONLY)
            {
                media.compressed = Some(frame_data.clone());
            }
            if flags.contains(ReceiveFlags::COMPRESSED_ONLY) {
                media.data = frame_data;
            } else if let Ok(decoded) = try_decode_vmx(
                &frame_data,
                v.width,
                v.height,
                v.color_space,
                preferred,
                vmx_codec,
                vmx_decode_buf,
            ) {
                media.data = decoded;
                media.codec = match preferred {
                    PreferredVideoFormat::Bgra => Codec::Bgra.as_i32(),
                    PreferredVideoFormat::P216 => Codec::P216.as_i32(),
                    _ => Codec::Uyvy.as_i32(),
                };
            } else {
                media.data = frame_data;
            }
        } else {
            media.data = frame_data;
        }
    } else if let Some(a) = frame.audio {
        media.codec = a.codec.as_i32();
        media.sample_rate = a.sample_rate;
        media.channels = a.channels;
        media.samples_per_channel = a.samples_per_channel;
        media.active_channels = a.active_channels;
        if a.codec == Codec::Fpa1 {
            let planes = fpa1::decode_planar(
                &frame_data,
                a.channels.max(0) as usize,
                a.samples_per_channel.max(0) as usize,
                a.active_channels,
            )?;
            let mut out = Vec::new();
            for plane in planes {
                for s in plane {
                    out.extend_from_slice(&s.to_le_bytes());
                }
            }
            media.data = out;
            if a.channels > 0 {
                media.active_channels = (1u32 << a.channels) - 1;
            }
        } else {
            media.data = frame_data;
        }
    } else {
        media.data = frame_data;
    }

    let data = media.data.clone();
    Ok(ReceivedFrame {
        frame_type: frame.header.frame_type,
        timestamp: frame.header.timestamp,
        data,
        metadata,
        media,
    })
}

fn try_decode_vmx(
    data: &[u8],
    width: i32,
    height: i32,
    color_space: crate::types::ColorSpace,
    preferred: PreferredVideoFormat,
    cached: &mut Option<vmx::Codec>,
    decode_buf: &mut Vec<u8>,
) -> Result<Vec<u8>, OmtError> {
    if width < vmx::MIN_WIDTH || height < vmx::MIN_HEIGHT {
        return Err(OmtError::Codec("VMX dimensions below minimum".into()));
    }
    let vmx_cs = match color_space {
        crate::types::ColorSpace::Bt601 => vmx::ColorSpace::Bt601,
        crate::types::ColorSpace::Bt709 => vmx::ColorSpace::Bt709,
        crate::types::ColorSpace::Undefined => vmx::ColorSpace::Undefined,
    };
    // Guard against panics inside vmx on corrupt bitstreams / edge sizes.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // Reuse the instance when geometry matches — size lives on the Codec,
        // not a parallel (w,h) key. Profile is decode-irrelevant (quality comes
        // from the bitstream); color_space is baked at create for BGRA convert.
        let reuse = cached.as_ref().is_some_and(|c| {
            let s = c.size();
            s.width == width && s.height == height
        });
        if !reuse {
            *cached = Some(vmx::Codec::new(vmx::Config {
                width,
                height,
                // Threads/quality presets only; bitstream overrides quality.
                profile: vmx::Profile::OmtSq,
                color_space: vmx_cs,
            })?);
        } else if let Some(codec) = cached.as_mut() {
            codec.set_color_space(vmx_cs);
        }
        let codec = cached.as_mut().unwrap();
        codec.load_from(data)?;
        match preferred {
            PreferredVideoFormat::Bgra => {
                let stride = (width as usize) * 4;
                let need = stride * height as usize;
                let mut dst = std::mem::take(decode_buf);
                dst.resize(need, 0);
                codec.decode_bgra(&mut dst, stride)?;
                Ok(dst)
            }
            PreferredVideoFormat::P216 => {
                let y_stride = (width as usize) * 2;
                let uv_stride = width as usize * 2;
                let y_len = y_stride * height as usize;
                let uv_len = uv_stride * height as usize;
                let mut dst = std::mem::take(decode_buf);
                dst.resize(y_len + uv_len, 0);
                let (y, uv) = dst.split_at_mut(y_len);
                codec.decode_p216(y, y_stride, uv, uv_stride)?;
                Ok(dst)
            }
            _ => {
                let stride = (width as usize) * 2;
                let need = stride * height as usize;
                let mut dst = std::mem::take(decode_buf);
                dst.resize(need, 0);
                codec.decode_uyvy(&mut dst, stride)?;
                Ok(dst)
            }
        }
    }))
    .unwrap_or_else(|_| {
        *cached = None;
        Err(OmtError::Codec("VMX decode panicked".into()))
    })
}

/// Received frame with decoded payload when applicable.
#[derive(Debug, Clone)]
pub struct ReceivedFrame {
    /// Frame type.
    pub frame_type: FrameType,
    /// Timestamp.
    pub timestamp: i64,
    /// Payload bytes.
    pub data: Vec<u8>,
    /// Optional per-frame metadata XML.
    pub metadata: Option<String>,
    /// Structured media frame.
    pub media: MediaFrame,
}
