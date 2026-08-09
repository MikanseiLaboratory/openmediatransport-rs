//! OMT receiver (sync).

use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use crate::codec::fpa1;
use crate::discovery::address::OmtAddress;
use crate::error::OmtError;
use crate::protocol::frame::{AssembledFrame, FrameHeader, PROTOCOL_VERSION};
use crate::protocol::metadata::{
    encode_metadata_xml, suggested_quality_xml, SUBSCRIBE_AUDIO, SUBSCRIBE_METADATA,
    SUBSCRIBE_VIDEO,
};
use crate::transport::channel::Channel;
use crate::transport::socket::connect;
use crate::types::{
    Codec, FrameType, MediaFrame, PreferredVideoFormat, Quality, ReceiveFlags, Statistics,
};

/// Receives frames from an OMT source (dual connections for A/V + metadata).
#[derive(Debug)]
pub struct Receiver {
    address: String,
    parsed: OmtAddress,
    frame_types: FrameType,
    preferred_format: PreferredVideoFormat,
    flags: ReceiveFlags,
    suggested_quality: Quality,
    stats: Statistics,
    av_stream: Option<TcpStream>,
    meta_stream: Option<TcpStream>,
    av_channel: Channel,
    meta_channel: Channel,
    subscribed: bool,
}

impl Receiver {
    /// Create a receiver for `address` (e.g. `omt://host:port/Source`).
    pub fn create(address: impl Into<String>, frame_types: FrameType) -> Result<Self, OmtError> {
        let address = address.into();
        if address.is_empty() {
            return Err(OmtError::InvalidArgument("receiver address is empty".into()));
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
            meta_channel: Channel::new(FrameType::METADATA),
            subscribed: false,
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

        let av = connect(addr, timeout)?;
        let meta = connect(addr, timeout)?;
        self.av_stream = Some(av);
        self.meta_stream = Some(meta);
        self.send_subscriptions()?;
        self.subscribed = true;
        Ok(())
    }

    fn send_subscriptions(&mut self) -> Result<(), OmtError> {
        let quality_xml = suggested_quality_xml(self.suggested_quality);
        if let Some(stream) = self.meta_stream.as_mut() {
            if self.frame_types.contains(FrameType::METADATA) {
                write_metadata_frame(stream, SUBSCRIBE_METADATA)?;
            }
            write_metadata_frame(stream, &quality_xml)?;
        }
        if let Some(stream) = self.av_stream.as_mut() {
            if self.frame_types.contains(FrameType::VIDEO) {
                write_metadata_frame(stream, SUBSCRIBE_VIDEO)?;
            }
            if self.frame_types.contains(FrameType::AUDIO) {
                write_metadata_frame(stream, SUBSCRIBE_AUDIO)?;
            }
        }
        Ok(())
    }

    /// Receive the next frame, reconnecting on hard errors when possible.
    pub fn receive(&mut self, timeout_ms: i32) -> Result<Option<ReceivedFrame>, OmtError> {
        if self.av_stream.is_none() {
            let t = if timeout_ms < 0 {
                None
            } else {
                Some(Duration::from_millis(timeout_ms as u64))
            };
            self.connect(t)?;
        }

        if let Some(stream) = self.av_stream.as_mut() {
            if timeout_ms >= 0 {
                let _ = stream.set_read_timeout(Some(Duration::from_millis(timeout_ms as u64)));
            }
            match self.av_channel.recv_frame(stream) {
                Ok(Some(frame)) => {
                    let nbytes = frame.to_bytes().len();
                    self.stats.record_received(nbytes);
                    return Ok(Some(decode_received(
                        frame,
                        self.flags,
                        self.preferred_format,
                    )?));
                }
                Ok(None) => return Ok(None),
                Err(OmtError::Network(_)) | Err(OmtError::Io(_)) => {
                    self.av_stream = None;
                    self.meta_stream = None;
                    self.subscribed = false;
                    return Ok(None);
                }
                Err(e) => return Err(e),
            }
        }
        Ok(None)
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
) -> Result<ReceivedFrame, OmtError> {
    let metadata = if frame.metadata.is_empty() {
        None
    } else {
        Some(crate::protocol::metadata::decode_metadata_xml(
            &frame.metadata,
        )?)
    };

    let mut media = MediaFrame {
        frame_type: frame.header.frame_type,
        timestamp: frame.header.timestamp,
        data: frame.data.clone(),
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
                media.compressed = Some(frame.data.clone());
            }
            if flags.contains(ReceiveFlags::COMPRESSED_ONLY) {
                media.data.clear();
            } else if let Ok(decoded) =
                try_decode_vmx(&frame.data, v.width, v.height, preferred)
            {
                media.data = decoded;
                media.codec = match preferred {
                    PreferredVideoFormat::Bgra => Codec::Bgra.as_i32(),
                    PreferredVideoFormat::P216 => Codec::P216.as_i32(),
                    _ => Codec::Uyvy.as_i32(),
                };
            }
        }
    }

    if let Some(a) = frame.audio {
        media.codec = a.codec.as_i32();
        media.sample_rate = a.sample_rate;
        media.channels = a.channels;
        media.samples_per_channel = a.samples_per_channel;
        media.active_channels = a.active_channels;
        if a.codec == Codec::Fpa1 {
            let planes = fpa1::decode_planar(
                &frame.data,
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
        }
    }

    Ok(ReceivedFrame {
        frame_type: frame.header.frame_type,
        timestamp: frame.header.timestamp,
        data: media.data.clone(),
        metadata,
        media,
    })
}

fn try_decode_vmx(
    data: &[u8],
    width: i32,
    height: i32,
    preferred: PreferredVideoFormat,
) -> Result<Vec<u8>, OmtError> {
    if width < vmx::MIN_WIDTH || height < vmx::MIN_HEIGHT {
        return Err(OmtError::Codec("VMX dimensions below minimum".into()));
    }
    // Guard against panics inside vmx on corrupt bitstreams / edge sizes.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let config = vmx::Config::new(width, height);
        let mut codec = vmx::Codec::new(config)?;
        codec.load_from(data)?;
        match preferred {
            PreferredVideoFormat::Bgra => {
                let stride = (width as usize) * 4;
                let mut dst = vec![0u8; stride * height as usize];
                codec.decode_bgra(&mut dst, stride)?;
                Ok(dst)
            }
            PreferredVideoFormat::P216 => {
                let y_stride = (width as usize) * 2;
                let uv_stride = width as usize * 2;
                let mut y = vec![0u8; y_stride * height as usize];
                let mut uv = vec![0u8; uv_stride * height as usize];
                codec.decode_p216(&mut y, y_stride, &mut uv, uv_stride)?;
                y.extend_from_slice(&uv);
                Ok(y)
            }
            _ => {
                let stride = (width as usize) * 2;
                let mut dst = vec![0u8; stride * height as usize];
                codec.decode_uyvy(&mut dst, stride)?;
                Ok(dst)
            }
        }
    }))
    .unwrap_or_else(|_| Err(OmtError::Codec("VMX decode panicked".into())))
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
