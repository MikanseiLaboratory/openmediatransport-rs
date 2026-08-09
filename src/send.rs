//! OMT sender (sync).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use crate::clock::{resolve_timestamp, TimestampClock};
use crate::codec::fpa1;
use crate::error::OmtError;
use crate::protocol::frame::{
    AssembledFrame, AudioHeader, FrameHeader, VideoHeader, AUDIO_EXT_HEADER_SIZE,
    PROTOCOL_VERSION, VIDEO_EXT_HEADER_SIZE,
};
use crate::protocol::metadata::{
    decode_metadata_xml, encode_metadata_xml, tally_xml, PREVIEW_OFF, PREVIEW_ON, SUBSCRIBE_AUDIO,
    SUBSCRIBE_METADATA, SUBSCRIBE_VIDEO,
};
use crate::transport::socket::{configure_stream, into_listener, listen};
use crate::types::{
    Codec, FrameType, MediaFrame, Quality, SenderInfo, Statistics, Tally, VideoFlags,
    NETWORK_PORT_END, NETWORK_PORT_START,
};

/// Peer subscription / control state.
#[derive(Debug, Default, Clone)]
struct PeerState {
    video: bool,
    audio: bool,
    metadata: bool,
    preview: bool,
    quality: Quality,
    tally: Tally,
}

/// Publishes an OMT source and sends media frames.
#[derive(Debug)]
pub struct Sender {
    name: String,
    frame_types: FrameType,
    quality: Quality,
    stats: Statistics,
    info: SenderInfo,
    clock: TimestampClock,
    listener: Option<TcpListener>,
    port: u16,
    peers: Arc<Mutex<HashMap<usize, (TcpStream, PeerState)>>>,
    next_peer_id: usize,
    subscribed: PeerState,
}

impl Sender {
    /// Create a sender that listens on an available port in 6400..=6600.
    pub fn create(name: impl Into<String>, frame_types: FrameType) -> Result<Self, OmtError> {
        let name = name.into();
        if name.is_empty() {
            return Err(OmtError::InvalidArgument("sender name is empty".into()));
        }
        let (listener, port) = bind_port_range()?;
        Ok(Self {
            name,
            frame_types,
            quality: Quality::Default,
            stats: Statistics::default(),
            info: SenderInfo::default(),
            clock: TimestampClock::new(),
            listener: Some(listener),
            port,
            peers: Arc::new(Mutex::new(HashMap::new())),
            next_peer_id: 1,
            subscribed: PeerState::default(),
        })
    }

    /// Create without binding a socket (unit-test / offline mode).
    pub fn create_offline(name: impl Into<String>, frame_types: FrameType) -> Result<Self, OmtError> {
        let name = name.into();
        if name.is_empty() {
            return Err(OmtError::InvalidArgument("sender name is empty".into()));
        }
        Ok(Self {
            name,
            frame_types,
            quality: Quality::Default,
            stats: Statistics::default(),
            info: SenderInfo::default(),
            clock: TimestampClock::new(),
            listener: None,
            port: 0,
            peers: Arc::new(Mutex::new(HashMap::new())),
            next_peer_id: 1,
            subscribed: PeerState::default(),
        })
    }

    /// Source name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Listening TCP port (0 if offline).
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Configured frame types.
    pub fn frame_types(&self) -> FrameType {
        self.frame_types
    }

    /// Set encoding quality policy.
    pub fn set_quality(&mut self, quality: Quality) {
        self.quality = quality;
    }

    /// Set sender product info.
    pub fn set_sender_info(&mut self, info: SenderInfo) {
        self.info = info;
    }

    /// Accept one pending connection if available (non-blocking).
    pub fn poll_accept(&mut self) -> Result<bool, OmtError> {
        let Some(listener) = self.listener.as_ref() else {
            return Ok(false);
        };
        listener.set_nonblocking(true)?;
        match listener.accept() {
            Ok((stream, _)) => {
                configure_stream(&stream)?;
                stream.set_nonblocking(true)?;
                let id = self.next_peer_id;
                self.next_peer_id += 1;
                self.peers
                    .lock()
                    .unwrap()
                    .insert(id, (stream, PeerState::default()));
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// Process inbound metadata from peers (subscribe / preview / tally / quality).
    pub fn poll_peer_metadata(&mut self) -> Result<(), OmtError> {
        let mut peers = self.peers.lock().unwrap();
        let mut dead = Vec::new();
        for (id, (stream, state)) in peers.iter_mut() {
            let mut buf = [0u8; 8192];
            match stream.read(&mut buf) {
                Ok(0) => dead.push(*id),
                Ok(n) => {
                    if let Ok(frame) =
                        crate::protocol::frame::AssembledFrame::from_bytes(&buf[..n])
                    {
                        if let Ok(xml) = decode_metadata_xml(&frame.metadata) {
                            apply_metadata(state, &xml);
                        } else if frame.header.frame_type.contains(FrameType::METADATA) {
                            if let Ok(xml) = decode_metadata_xml(&frame.data) {
                                apply_metadata(state, &xml);
                            }
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => dead.push(*id),
            }
        }
        for id in dead {
            peers.remove(&id);
        }
        drop(peers);
        self.recompute_subscriptions();
        Ok(())
    }

    fn recompute_subscriptions(&mut self) {
        let peers = self.peers.lock().unwrap();
        let mut agg = PeerState::default();
        let mut best_q = Quality::Default;
        for (_, state) in peers.values() {
            agg.video |= state.video;
            agg.audio |= state.audio;
            agg.metadata |= state.metadata;
            agg.preview |= state.preview;
            agg.tally.preview |= state.tally.preview;
            agg.tally.program |= state.tally.program;
            if state.quality as i32 > best_q as i32 {
                best_q = state.quality;
            }
        }
        agg.quality = if self.quality == Quality::Default {
            best_q
        } else {
            self.quality
        };
        self.subscribed = agg;
    }

    /// Whether any peer has subscribed to video.
    pub fn video_subscribed(&self) -> bool {
        self.subscribed.video
    }

    /// Whether any peer has subscribed to audio.
    pub fn audio_subscribed(&self) -> bool {
        self.subscribed.audio
    }

    /// Aggregated tally across peers.
    pub fn tally(&self) -> Tally {
        self.subscribed.tally
    }

    /// Effective quality (local policy or peer suggestions).
    pub fn effective_quality(&self) -> Quality {
        self.subscribed.quality
    }

    /// Preview mode requested by any peer.
    pub fn preview(&self) -> bool {
        self.subscribed.preview
    }

    /// Build a wire frame for a media frame (FPA1 sparse encode for audio).
    pub fn build_frame(&mut self, mut frame: MediaFrame) -> Result<AssembledFrame, OmtError> {
        frame.timestamp = self.clock.resolve(
            frame.timestamp,
            frame.frame_rate_n,
            frame.frame_rate_d,
            frame.sample_rate,
            frame.samples_per_channel,
        );

        let metadata = frame
            .frame_metadata
            .as_deref()
            .map(encode_metadata_xml)
            .unwrap_or_default();
        let metadata_length = metadata.len() as u16;

        if frame.frame_type.contains(FrameType::VIDEO) {
            let codec = Codec::from_i32(frame.codec).unwrap_or(Codec::Uyvy);
            let mut flags = frame.flags;
            if self.subscribed.preview {
                flags = VideoFlags(flags.0 | VideoFlags::PREVIEW.0);
            }
            let video = VideoHeader {
                codec,
                width: frame.width,
                height: frame.height,
                frame_rate_n: frame.frame_rate_n,
                frame_rate_d: frame.frame_rate_d,
                aspect_ratio: frame.aspect_ratio,
                flags,
                color_space: frame.color_space,
            };
            let data_length = (VIDEO_EXT_HEADER_SIZE + frame.data.len() + metadata.len()) as i32;
            Ok(AssembledFrame {
                header: FrameHeader {
                    version: PROTOCOL_VERSION,
                    frame_type: FrameType::VIDEO,
                    timestamp: frame.timestamp,
                    metadata_length,
                    data_length,
                },
                video: Some(video),
                audio: None,
                data: frame.data,
                metadata,
            })
        } else if frame.frame_type.contains(FrameType::AUDIO) {
            let (payload, active) = if frame.active_channels != 0 {
                (frame.data, frame.active_channels)
            } else {
                let channels = frame.channels.max(0) as usize;
                let samples = frame.samples_per_channel.max(0) as usize;
                if channels == 0 || samples == 0 {
                    return Err(OmtError::InvalidArgument(
                        "audio frame missing geometry".into(),
                    ));
                }
                let expected = channels * samples * 4;
                if frame.data.len() < expected {
                    return Err(OmtError::InvalidArgument(
                        "audio frame data too short".into(),
                    ));
                }
                let mut owned: Vec<Vec<f32>> = Vec::with_capacity(channels);
                for ch in 0..channels {
                    let mut plane = Vec::with_capacity(samples);
                    let base = ch * samples * 4;
                    for s in 0..samples {
                        let o = base + s * 4;
                        let b: [u8; 4] = frame.data[o..o + 4].try_into().unwrap();
                        plane.push(f32::from_le_bytes(b));
                    }
                    owned.push(plane);
                }
                let refs: Vec<&[f32]> = owned.iter().map(|p| p.as_slice()).collect();
                fpa1::encode_planar(&refs)?
            };
            let audio = AudioHeader {
                codec: Codec::Fpa1,
                sample_rate: frame.sample_rate,
                samples_per_channel: frame.samples_per_channel,
                channels: frame.channels,
                active_channels: active,
                reserved1: 0,
            };
            let data_length = (AUDIO_EXT_HEADER_SIZE + payload.len() + metadata.len()) as i32;
            Ok(AssembledFrame {
                header: FrameHeader {
                    version: PROTOCOL_VERSION,
                    frame_type: FrameType::AUDIO,
                    timestamp: frame.timestamp,
                    metadata_length,
                    data_length,
                },
                video: None,
                audio: Some(audio),
                data: payload,
                metadata,
            })
        } else {
            let data = if frame.data.is_empty() {
                metadata.clone()
            } else {
                frame.data
            };
            Ok(AssembledFrame {
                header: FrameHeader {
                    version: PROTOCOL_VERSION,
                    frame_type: FrameType::METADATA,
                    timestamp: frame.timestamp,
                    metadata_length: 0,
                    data_length: data.len() as i32,
                },
                video: None,
                audio: None,
                data,
                metadata: Vec::new(),
            })
        }
    }

    /// Send a video frame to subscribed peers (no-op if none subscribed).
    pub fn send_video(&mut self, frame: MediaFrame) -> Result<(), OmtError> {
        if !self.subscribed.video {
            return Ok(());
        }
        let assembled = self.build_frame(frame)?;
        self.broadcast(&assembled)
    }

    /// Send an audio frame to subscribed peers (no-op if none subscribed).
    pub fn send_audio(&mut self, frame: MediaFrame) -> Result<(), OmtError> {
        if !self.subscribed.audio {
            return Ok(());
        }
        let assembled = self.build_frame(frame)?;
        self.broadcast(&assembled)
    }

    /// Send metadata XML.
    pub fn send_metadata(&mut self, timestamp: i64, xml: &str) -> Result<(), OmtError> {
        let ts = resolve_timestamp(timestamp);
        let data = encode_metadata_xml(xml);
        let assembled = AssembledFrame {
            header: FrameHeader {
                version: PROTOCOL_VERSION,
                frame_type: FrameType::METADATA,
                timestamp: ts,
                metadata_length: 0,
                data_length: data.len() as i32,
            },
            video: None,
            audio: None,
            data,
            metadata: Vec::new(),
        };
        self.broadcast(&assembled)
    }

    /// Send current aggregated tally as metadata.
    pub fn send_tally(&mut self) -> Result<(), OmtError> {
        let xml = tally_xml(self.subscribed.tally);
        self.send_metadata(0, xml)
    }

    fn broadcast(&mut self, frame: &AssembledFrame) -> Result<(), OmtError> {
        let bytes = frame.to_bytes();
        let mut peers = self.peers.lock().unwrap();
        let mut dead = Vec::new();
        for (id, (stream, _)) in peers.iter_mut() {
            // Ensure full-frame writes (peers may have been set non-blocking for accept/poll).
            let _ = stream.set_nonblocking(false);
            if stream.write_all(&bytes).is_err() {
                dead.push(*id);
            } else {
                let _ = stream.set_nonblocking(true);
            }
        }
        for id in dead {
            peers.remove(&id);
        }
        self.stats.record_sent(bytes.len());
        Ok(())
    }

    /// Force-mark subscriptions (useful for tests / offline).
    pub fn force_subscribe(&mut self, video: bool, audio: bool, metadata: bool) {
        self.subscribed.video = video;
        self.subscribed.audio = audio;
        self.subscribed.metadata = metadata;
    }

    /// Snapshot of send statistics.
    pub fn statistics(&self) -> Statistics {
        self.stats
    }
}

fn apply_metadata(state: &mut PeerState, xml: &str) {
    if xml == SUBSCRIBE_VIDEO {
        state.video = true;
    } else if xml == SUBSCRIBE_AUDIO {
        state.audio = true;
    } else if xml == SUBSCRIBE_METADATA {
        state.metadata = true;
    } else if xml == PREVIEW_ON {
        state.preview = true;
    } else if xml == PREVIEW_OFF {
        state.preview = false;
    } else if let Some(rest) = xml.strip_prefix(r#"<OMTSettings Quality=""#) {
        if let Some(name) = rest.split('"').next() {
            state.quality = match name {
                "Low" => Quality::Low,
                "Medium" => Quality::Medium,
                "High" => Quality::High,
                _ => Quality::Default,
            };
        }
    } else if xml.contains("OMTTally") {
        state.tally.preview = if xml.contains(r#"Preview="true""#) {
            1
        } else {
            0
        };
        state.tally.program =
            if xml.contains(r#"Program=="true""#) || xml.contains(r#"Program="true""#) {
                1
            } else {
                0
            };
    }
}

fn bind_port_range() -> Result<(TcpListener, u16), OmtError> {
    for port in NETWORK_PORT_START..=NETWORK_PORT_END {
        let addr = SocketAddr::from(([0, 0, 0, 0], port));
        if let Ok(sock) = listen(addr) {
            let listener = into_listener(sock)?;
            return Ok((listener, port));
        }
    }
    Err(OmtError::Network(format!(
        "no free port in {NETWORK_PORT_START}..={NETWORK_PORT_END}"
    )))
}
