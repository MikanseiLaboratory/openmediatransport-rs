//! OMT sender (sync).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::clock::{TimestampClock, resolve_timestamp};
use crate::codec::fpa1;
use crate::error::OmtError;
use crate::protocol::frame::{
    AUDIO_EXT_HEADER_SIZE, AssembledFrame, AudioHeader, FrameHeader, PROTOCOL_VERSION,
    VIDEO_EXT_HEADER_SIZE, VideoHeader,
};
use crate::protocol::metadata::{
    PREVIEW_OFF, PREVIEW_ON, SUBSCRIBE_AUDIO, SUBSCRIBE_METADATA, SUBSCRIBE_VIDEO,
    decode_metadata_xml, encode_metadata_xml, tally_xml,
};
use crate::transport::socket::{configure_stream_buffers, into_listener, listen};
use crate::types::{
    Codec, FrameType, MediaFrame, NETWORK_ASYNC_COUNT, NETWORK_PORT_END, NETWORK_PORT_START,
    NETWORK_SEND_BUFFER, NETWORK_SEND_RECEIVE_BUFFER, Quality, SenderInfo, Statistics, Tally,
    VideoFlags,
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

/// Sender transport / buffering configuration.
///
/// Defaults match libomtnet (`OMTConstants`):
/// - socket send buffer: 64 KiB (`NETWORK_SEND_BUFFER`)
/// - socket recv buffer on sender peers: 64 KiB (`NETWORK_SEND_RECEIVE_BUFFER`)
/// - outstanding send queue depth: 4 (`NETWORK_ASYNC_COUNT`); when full, frames are dropped
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SenderConfig {
    /// TCP `SO_SNDBUF` for peer connections.
    pub send_buffer: usize,
    /// TCP `SO_RCVBUF` for peer connections (control / subscribe traffic).
    pub recv_buffer: usize,
    /// Max frames queued for async socket writes.
    ///
    /// `0` = blocking synchronous `write_all` on the calling thread.
    /// Default `4` matches libomtnet's async send pool; overflow drops the frame.
    pub send_queue_depth: usize,
}

impl Default for SenderConfig {
    fn default() -> Self {
        Self {
            send_buffer: NETWORK_SEND_BUFFER,
            recv_buffer: NETWORK_SEND_RECEIVE_BUFFER,
            send_queue_depth: NETWORK_ASYNC_COUNT,
        }
    }
}

/// Publishes an OMT source and sends media frames.
#[derive(Debug)]
pub struct Sender {
    name: String,
    frame_types: FrameType,
    quality: Quality,
    stats: Statistics,
    info: SenderInfo,
    video_clock: TimestampClock,
    audio_clock: TimestampClock,
    listener: Option<TcpListener>,
    port: u16,
    peers: Arc<Mutex<HashMap<usize, (TcpStream, PeerState)>>>,
    next_peer_id: usize,
    subscribed: PeerState,
    config: SenderConfig,
    /// When set, `broadcast` enqueues bytes for a background writer (libomtnet-style).
    outbound_tx: Option<SyncSender<Vec<u8>>>,
}

impl Sender {
    /// Create a sender that listens on an available port in 6400..=6600.
    pub fn create(name: impl Into<String>, frame_types: FrameType) -> Result<Self, OmtError> {
        Self::create_with_config(name, frame_types, SenderConfig::default())
    }

    /// Create a sender with explicit transport / buffering settings.
    pub fn create_with_config(
        name: impl Into<String>,
        frame_types: FrameType,
        config: SenderConfig,
    ) -> Result<Self, OmtError> {
        let name = name.into();
        if name.is_empty() {
            return Err(OmtError::InvalidArgument("sender name is empty".into()));
        }
        let (listener, port) = bind_port_range()?;
        let peers = Arc::new(Mutex::new(HashMap::new()));
        let outbound_tx = spawn_outbound_writer(config.send_queue_depth, Arc::clone(&peers));
        Ok(Self {
            name,
            frame_types,
            quality: Quality::Default,
            stats: Statistics::default(),
            info: SenderInfo::default(),
            video_clock: TimestampClock::new(),
            audio_clock: TimestampClock::new(),
            listener: Some(listener),
            port,
            peers,
            next_peer_id: 1,
            subscribed: PeerState::default(),
            config,
            outbound_tx,
        })
    }

    /// Create without binding a socket (unit-test / offline mode).
    pub fn create_offline(
        name: impl Into<String>,
        frame_types: FrameType,
    ) -> Result<Self, OmtError> {
        Self::create_offline_with_config(name, frame_types, SenderConfig::default())
    }

    /// Offline sender with explicit transport config (queue unused without peers).
    pub fn create_offline_with_config(
        name: impl Into<String>,
        frame_types: FrameType,
        config: SenderConfig,
    ) -> Result<Self, OmtError> {
        let name = name.into();
        if name.is_empty() {
            return Err(OmtError::InvalidArgument("sender name is empty".into()));
        }
        let peers = Arc::new(Mutex::new(HashMap::new()));
        let outbound_tx = spawn_outbound_writer(config.send_queue_depth, Arc::clone(&peers));
        Ok(Self {
            name,
            frame_types,
            quality: Quality::Default,
            stats: Statistics::default(),
            info: SenderInfo::default(),
            video_clock: TimestampClock::new(),
            audio_clock: TimestampClock::new(),
            listener: None,
            port: 0,
            peers,
            next_peer_id: 1,
            subscribed: PeerState::default(),
            config,
            outbound_tx,
        })
    }

    /// Current transport / buffering configuration.
    pub fn transport_config(&self) -> SenderConfig {
        self.config
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
            Ok((mut stream, _)) => {
                configure_stream_buffers(&stream, self.config.send_buffer, self.config.recv_buffer)?;
                // libomtnet sends OMTInfo immediately on accept.
                if !self.info.product_name.is_empty()
                    || !self.info.manufacturer.is_empty()
                    || !self.info.version.is_empty()
                {
                    let _ = stream.set_nonblocking(false);
                    let data = encode_metadata_xml(&self.info.to_xml());
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
                    let _ = stream.write_all(&frame.to_bytes());
                }
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
                    let mut rest = &buf[..n];
                    while rest.len() >= crate::protocol::frame::HEADER_SIZE {
                        let header = match FrameHeader::from_bytes(rest) {
                            Ok(h) => h,
                            Err(_) => break,
                        };
                        if header.data_length < 0 {
                            break;
                        }
                        let total =
                            crate::protocol::frame::HEADER_SIZE + header.data_length as usize;
                        if rest.len() < total {
                            break;
                        }
                        if let Ok(frame) = AssembledFrame::from_bytes(&rest[..total]) {
                            if frame.header.frame_type.contains(FrameType::METADATA) {
                                if let Ok(xml) = decode_metadata_xml(&frame.data) {
                                    apply_metadata(state, &xml);
                                }
                            } else if let Ok(xml) = decode_metadata_xml(&frame.metadata) {
                                apply_metadata(state, &xml);
                            }
                        }
                        rest = &rest[total..];
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
        let is_audio = frame.frame_type.contains(FrameType::AUDIO)
            && !frame.frame_type.contains(FrameType::VIDEO);
        frame.timestamp = if is_audio {
            self.audio_clock.resolve(
                frame.timestamp,
                frame.frame_rate_n,
                frame.frame_rate_d,
                frame.sample_rate,
                frame.samples_per_channel,
            )
        } else if frame.frame_type.contains(FrameType::VIDEO) {
            self.video_clock.resolve(
                frame.timestamp,
                frame.frame_rate_n,
                frame.frame_rate_d,
                0,
                0,
            )
        } else {
            resolve_timestamp(frame.timestamp)
        };

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
        let nbytes = bytes.len();
        if let Some(tx) = &self.outbound_tx {
            match tx.try_send(bytes) {
                Ok(()) => {
                    // Count as sent when accepted into the pool (matches libomtnet semantics).
                    // Actual socket write happens on the writer thread.
                    self.stats.record_sent(nbytes);
                }
                Err(std::sync::mpsc::TrySendError::Full(_)) => {
                    self.stats.record_dropped();
                }
                Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                    self.stats.record_dropped();
                }
            }
            return Ok(());
        }

        write_peers(&self.peers, &bytes);
        self.stats.record_sent(nbytes);
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

fn spawn_outbound_writer(
    depth: usize,
    peers: Arc<Mutex<HashMap<usize, (TcpStream, PeerState)>>>,
) -> Option<SyncSender<Vec<u8>>> {
    if depth == 0 {
        return None;
    }
    let (tx, rx) = sync_channel::<Vec<u8>>(depth);
    thread::Builder::new()
        .name("omt-send".into())
        .spawn(move || {
            while let Ok(bytes) = rx.recv() {
                write_peers(&peers, &bytes);
            }
        })
        .ok();
    Some(tx)
}

fn write_peers(peers: &Mutex<HashMap<usize, (TcpStream, PeerState)>>, bytes: &[u8]) {
    let mut peers = peers.lock().unwrap();
    let mut dead = Vec::new();
    for (id, (stream, _)) in peers.iter_mut() {
        // Ensure full-frame writes (peers may have been set non-blocking for accept/poll).
        let _ = stream.set_nonblocking(false);
        if stream.write_all(bytes).is_err() {
            dead.push(*id);
        } else {
            let _ = stream.set_nonblocking(true);
        }
    }
    for id in dead {
        peers.remove(&id);
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
