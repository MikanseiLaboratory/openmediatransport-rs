//! OMT receiver session — dedicated I/O + decode threads with bounded queues.

use std::collections::VecDeque;
use std::io::Write;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver as MpscReceiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::codec::fpa1;
use crate::discovery::address::OmtAddress;
use crate::error::OmtError;
use crate::protocol::frame::{AssembledFrame, FrameHeader, PROTOCOL_VERSION};
use crate::protocol::metadata::{
    PREVIEW_OFF, PREVIEW_ON, SUBSCRIBE_AUDIO, SUBSCRIBE_METADATA, SUBSCRIBE_VIDEO,
    encode_metadata_xml, suggested_quality_xml,
};
use crate::transport::channel::Channel;
use crate::transport::pool::BufferPool;
use crate::transport::socket::connect;
use crate::types::{
    AUDIO_MAX_SIZE, Codec, ColorSpace, DecodedAudioFrame, DecodedVideoFrame, FrameType,
    MetadataFrame, Quality, SessionStatistics, VIDEO_MAX_SIZE, VideoFlags,
};

/// Wire-compressed video queue depth (backpressure → drop).
const VIDEO_WIRE_Q: usize = 3;
/// Decoded video depth-1 FIFO (latest-wins when full).
const VIDEO_DECODED_Q: usize = 1;
const AUDIO_Q: usize = 10;
const METADATA_Q: usize = 60;
const RECONNECT_MIN: Duration = Duration::from_millis(250);
const RECONNECT_MAX: Duration = Duration::from_secs(2);

/// High-level receiver connection state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionState {
    /// Initial TCP connect / subscribe in progress.
    #[default]
    Connecting,
    /// At least one media socket is active.
    Connected,
    /// Waiting before a reconnect attempt.
    Reconnecting,
    /// [`ReceiverSession::disconnect`] requested.
    Stopping,
    /// All threads have exited.
    Stopped,
}

/// Configuration for [`ReceiverSession`].
#[derive(Debug, Clone)]
pub struct ReceiverConfig {
    /// Frame types to subscribe to.
    pub frame_types: FrameType,
    /// Suggested encode quality sent to the peer.
    pub quality: Quality,
    /// Request 1/8 preview video (`<OMTSettings Preview="true" />`).
    ///
    /// Progressive VMX1 only; decoded frames are BGRA at `width/8 × height/8`
    /// (width aligned to 2). Interlace / alpha / high-bit-depth preview is not
    /// supported in this build.
    pub preview: bool,
    /// TCP connect timeout.
    pub connect_timeout: Duration,
    /// Automatically reconnect after socket failures (250 ms … 2 s backoff).
    pub auto_reconnect: bool,
}

impl Default for ReceiverConfig {
    fn default() -> Self {
        Self {
            frame_types: FrameType::VIDEO | FrameType::AUDIO | FrameType::METADATA,
            quality: Quality::Default,
            preview: false,
            connect_timeout: Duration::from_secs(5),
            auto_reconnect: true,
        }
    }
}

struct WireVideo {
    timestamp: i64,
    width: i32,
    height: i32,
    frame_rate_n: i32,
    frame_rate_d: i32,
    color_space: ColorSpace,
    /// Wire `VideoFlags::PREVIEW` — decode via `decode_preview_bgra`.
    preview: bool,
    payload: Vec<u8>,
    metadata: Option<Arc<str>>,
    enqueued_at: Instant,
}

/// Bounded FIFO handoff for decoded video (preserves frame order for playout).
struct DecodedVideoQueue {
    slot: Mutex<VecDeque<DecodedVideoFrame>>,
    cv: Condvar,
    depth: AtomicU32,
    overwrites: AtomicU64,
    cap: usize,
}

impl DecodedVideoQueue {
    fn new(cap: usize) -> Self {
        Self {
            slot: Mutex::new(VecDeque::with_capacity(cap)),
            cv: Condvar::new(),
            depth: AtomicU32::new(0),
            overwrites: AtomicU64::new(0),
            cap: cap.max(1),
        }
    }

    fn publish(&self, frame: DecodedVideoFrame, stats: &Mutex<SessionStatistics>) {
        let mut g = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        while g.len() >= self.cap {
            g.pop_front();
            self.overwrites.fetch_add(1, Ordering::Relaxed);
            record_drop_decode(stats);
        }
        g.push_back(frame);
        self.depth.store(g.len() as u32, Ordering::Relaxed);
        self.cv.notify_one();
    }

    fn try_take(&self) -> Option<DecodedVideoFrame> {
        let mut g = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        let out = g.pop_front();
        self.depth.store(g.len() as u32, Ordering::Relaxed);
        out
    }

    fn wait_take(&self, timeout: Duration, stop: &AtomicBool) -> Option<DecodedVideoFrame> {
        let deadline = Instant::now() + timeout;
        let mut g = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(frame) = g.pop_front() {
                self.depth.store(g.len() as u32, Ordering::Relaxed);
                return Some(frame);
            }
            if stop.load(Ordering::Acquire) {
                return None;
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            let (guard, _) = self
                .cv
                .wait_timeout(g, deadline.saturating_duration_since(now))
                .unwrap_or_else(|e| e.into_inner());
            g = guard;
        }
    }
}

struct Shared {
    stop: AtomicBool,
    stats: Mutex<SessionStatistics>,
    state: Mutex<SessionState>,
    last_error: Mutex<Option<String>>,
    video: DecodedVideoQueue,
    wire_depth: AtomicU32,
}

impl Shared {
    fn set_state(&self, state: SessionState) {
        if let Ok(mut g) = self.state.lock() {
            *g = state;
        }
    }

    fn set_error(&self, msg: impl Into<String>) {
        if let Ok(mut g) = self.last_error.lock() {
            *g = Some(msg.into());
        }
    }

    fn bump_reconnect(&self) {
        if let Ok(mut g) = self.stats.lock() {
            g.reconnects = g.reconnects.saturating_add(1);
        }
    }
}

/// Multi-threaded OMT receiver: video I/O, video decode, and audio I/O run on
/// dedicated OS threads. Decoded frames are published on bounded channels /
/// a latest-wins video slot.
pub struct ReceiverSession {
    address: String,
    config: ReceiverConfig,
    shared: Arc<Shared>,
    audio_rx: MpscReceiver<DecodedAudioFrame>,
    metadata_rx: MpscReceiver<MetadataFrame>,
    joins: Vec<JoinHandle<()>>,
}

impl ReceiverSession {
    /// Connect and spawn reader / decoder threads.
    pub fn connect(address: impl Into<String>, config: ReceiverConfig) -> Result<Self, OmtError> {
        Self::connect_with_addresses(address, &[], config)
    }

    /// Connect using a URL plus optional discovery-time IP candidates.
    ///
    /// Endpoints are tried in order (discovery addresses first, then any host
    /// resolved from the URL) until one TCP connect succeeds — matching
    /// libomtnet `BeginConnect(IPAddress[], port)` behavior.
    pub fn connect_with_addresses(
        address: impl Into<String>,
        extra_addresses: &[String],
        config: ReceiverConfig,
    ) -> Result<Self, OmtError> {
        crate::logging::init_logging();
        let address = address.into();
        if address.is_empty() {
            return Err(OmtError::InvalidArgument(
                "receiver address is empty".into(),
            ));
        }
        let parsed = OmtAddress::from_url(&address)?;
        if parsed.port == 0 {
            return Err(OmtError::InvalidArgument(
                "receiver URL must include a port".into(),
            ));
        }
        let endpoints = resolve_endpoints(&parsed, extra_addresses)?;

        let want_video = config.frame_types.contains(FrameType::VIDEO)
            || config.frame_types.contains(FrameType::METADATA)
            || config.frame_types == FrameType::NONE;
        let want_audio = config.frame_types.contains(FrameType::AUDIO);

        // Initial connect is synchronous so callers learn about refusal immediately.
        let mut initial_video = None;
        let mut initial_audio = None;
        if want_video {
            let (mut stream, _) = connect_first(&endpoints, Some(config.connect_timeout))?;
            send_subscriptions(
                Some(&mut stream),
                None,
                config.frame_types,
                config.quality,
                config.preview,
                false,
            )?;
            initial_video = Some(stream);
        }
        if want_audio {
            let (mut stream, _) = connect_first(&endpoints, Some(config.connect_timeout))?;
            send_subscriptions(
                None,
                Some(&mut stream),
                config.frame_types,
                config.quality,
                false,
                false,
            )?;
            initial_audio = Some(stream);
        }

        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            stats: Mutex::new(SessionStatistics::default()),
            state: Mutex::new(SessionState::Connected),
            last_error: Mutex::new(None),
            video: DecodedVideoQueue::new(VIDEO_DECODED_Q),
            wire_depth: AtomicU32::new(0),
        });

        let (video_wire_tx, video_wire_rx) = sync_channel::<WireVideo>(VIDEO_WIRE_Q);
        let (audio_tx, audio_rx) = sync_channel::<DecodedAudioFrame>(AUDIO_Q);
        let (metadata_tx, metadata_rx) = sync_channel::<MetadataFrame>(METADATA_Q);

        let mut joins = Vec::new();

        {
            let shared_c = Arc::clone(&shared);
            joins.push(
                thread::Builder::new()
                    .name("omt-rx-vmx-decode".into())
                    .spawn(move || video_decode_loop(shared_c, video_wire_rx))
                    .map_err(|e| OmtError::Network(e.to_string()))?,
            );
        }

        if let Some(stream) = initial_video {
            let shared_c = Arc::clone(&shared);
            let cfg = config.clone();
            let endpoints = endpoints.clone();
            let video_wire_tx = video_wire_tx.clone();
            let metadata_tx = metadata_tx.clone();
            let audio_on_av = audio_tx.clone();
            joins.push(
                thread::Builder::new()
                    .name("omt-rx-video-io".into())
                    .spawn(move || {
                        socket_supervisor(
                            endpoints,
                            cfg,
                            shared_c,
                            SocketRole::Video {
                                video_tx: video_wire_tx,
                                metadata_tx,
                                audio_tx: audio_on_av,
                                decode_audio: false,
                            },
                            Some(stream),
                        );
                    })
                    .map_err(|e| OmtError::Network(e.to_string()))?,
            );
        }

        if let Some(stream) = initial_audio {
            let shared_c = Arc::clone(&shared);
            let cfg = config.clone();
            let endpoints = endpoints.clone();
            let audio_tx = audio_tx.clone();
            joins.push(
                thread::Builder::new()
                    .name("omt-rx-audio-io".into())
                    .spawn(move || {
                        socket_supervisor(
                            endpoints,
                            cfg,
                            shared_c,
                            SocketRole::Audio { audio_tx },
                            Some(stream),
                        );
                    })
                    .map_err(|e| OmtError::Network(e.to_string()))?,
            );
        }

        drop(video_wire_tx);
        drop(audio_tx);
        drop(metadata_tx);

        Ok(Self {
            address,
            config,
            shared,
            audio_rx,
            metadata_rx,
            joins,
        })
    }

    /// Connect using a discovered [`OmtAddress`] (URL + all candidate IPs).
    pub fn connect_from_address(
        addr: &OmtAddress,
        config: ReceiverConfig,
    ) -> Result<Self, OmtError> {
        Self::connect_with_addresses(addr.to_url(), &addr.addresses, config)
    }

    /// Connection address string.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Configured frame types.
    pub fn frame_types(&self) -> FrameType {
        self.config.frame_types
    }

    /// Current session state.
    pub fn state(&self) -> SessionState {
        self.shared
            .state
            .lock()
            .map(|g| *g)
            .unwrap_or(SessionState::Stopped)
    }

    /// Last transport / decode error message, if any.
    pub fn last_error(&self) -> Option<String> {
        self.shared.last_error.lock().ok().and_then(|g| g.clone())
    }

    /// Non-blocking poll for the next decoded video frame (latest-wins slot).
    pub fn try_recv_video(&self) -> Option<DecodedVideoFrame> {
        self.shared.video.try_take()
    }

    /// Blocking receive of the next decoded video frame (or `None` on timeout / shutdown).
    pub fn recv_video_timeout(&self, timeout: Duration) -> Option<DecodedVideoFrame> {
        self.shared.video.wait_take(timeout, &self.shared.stop)
    }

    /// Non-blocking poll for decoded audio.
    pub fn try_recv_audio(&self) -> Option<DecodedAudioFrame> {
        self.audio_rx.try_recv().ok()
    }

    /// Non-blocking poll for metadata.
    pub fn try_recv_metadata(&self) -> Option<MetadataFrame> {
        self.metadata_rx.try_recv().ok()
    }

    /// Snapshot of session statistics (includes live queue depths).
    pub fn statistics(&self) -> SessionStatistics {
        let mut s = self.shared.stats.lock().map(|g| *g).unwrap_or_default();
        s.wire_queue_depth = self.shared.wire_depth.load(Ordering::Relaxed);
        s.decoded_queue_depth = self.shared.video.depth.load(Ordering::Relaxed);
        s
    }

    /// Signal threads to stop and join them.
    pub fn disconnect(mut self) {
        self.shared.set_state(SessionState::Stopping);
        self.shared.stop.store(true, Ordering::Release);
        // Wake any waiter on the latest-video condvar.
        self.shared.video.cv.notify_all();
        for j in self.joins.drain(..) {
            let _ = j.join();
        }
        self.shared.set_state(SessionState::Stopped);
    }
}

impl Drop for ReceiverSession {
    fn drop(&mut self) {
        self.shared.set_state(SessionState::Stopping);
        self.shared.stop.store(true, Ordering::Release);
        self.shared.video.cv.notify_all();
        while let Some(j) = self.joins.pop() {
            let _ = j.join();
        }
        self.shared.set_state(SessionState::Stopped);
    }
}

enum SocketRole {
    Video {
        video_tx: SyncSender<WireVideo>,
        metadata_tx: SyncSender<MetadataFrame>,
        audio_tx: SyncSender<DecodedAudioFrame>,
        decode_audio: bool,
    },
    Audio {
        audio_tx: SyncSender<DecodedAudioFrame>,
    },
}

fn socket_supervisor(
    endpoints: Vec<SocketAddr>,
    config: ReceiverConfig,
    shared: Arc<Shared>,
    role: SocketRole,
    mut primed: Option<TcpStream>,
) {
    let mut backoff = RECONNECT_MIN;
    let mut use_primed = primed.is_some();
    loop {
        if shared.stop.load(Ordering::Acquire) {
            break;
        }

        let stream = if let Some(s) = primed.take() {
            s
        } else {
            if !use_primed {
                // Reconnect path (not the very first primed hand-off).
                shared.set_state(SessionState::Reconnecting);
                shared.bump_reconnect();
                if !interruptible_sleep(&shared.stop, backoff) {
                    break;
                }
                backoff = (backoff.saturating_mul(2)).min(RECONNECT_MAX);
            }
            use_primed = false;
            shared.set_state(SessionState::Connecting);
            match connect_first(&endpoints, Some(config.connect_timeout)) {
                Ok((s, _)) => s,
                Err(e) => {
                    shared.set_error(format!("connect failed: {e}"));
                    if !config.auto_reconnect {
                        break;
                    }
                    continue;
                }
            }
        };

        let is_reconnect = !use_primed;
        use_primed = false;
        if is_reconnect && let Err(e) = subscribe_socket(&stream, &config, &role) {
            shared.set_error(format!("subscribe failed: {e}"));
            if !config.auto_reconnect {
                break;
            }
            continue;
        }

        shared.set_state(SessionState::Connected);
        backoff = RECONNECT_MIN;

        match &role {
            SocketRole::Video {
                video_tx,
                metadata_tx,
                audio_tx,
                decode_audio,
            } => {
                video_reader_loop(
                    stream,
                    &shared,
                    video_tx,
                    metadata_tx,
                    audio_tx,
                    *decode_audio,
                );
            }
            SocketRole::Audio { audio_tx } => {
                audio_reader_loop(stream, &shared, audio_tx);
            }
        }

        if shared.stop.load(Ordering::Acquire) {
            break;
        }
        shared.set_error(match &role {
            SocketRole::Video { .. } => String::from("video socket closed"),
            SocketRole::Audio { .. } => String::from("audio socket closed"),
        });
        if !config.auto_reconnect {
            break;
        }
    }
}

/// Build TCP endpoints from URL host + discovery-time address candidates.
fn resolve_endpoints(
    parsed: &OmtAddress,
    extra_addresses: &[String],
) -> Result<Vec<SocketAddr>, OmtError> {
    let port = parsed.port;
    let mut hosts: Vec<String> = Vec::new();
    for host in extra_addresses.iter().chain(parsed.addresses.iter()) {
        let host = host.trim();
        if host.is_empty() {
            continue;
        }
        if !hosts.iter().any(|h| h.eq_ignore_ascii_case(host)) {
            hosts.push(host.to_string());
        }
    }
    if hosts.is_empty() {
        return Err(OmtError::InvalidArgument(
            "receiver URL has no host addresses".into(),
        ));
    }

    let mut endpoints: Vec<SocketAddr> = Vec::new();
    for host in &hosts {
        if let Ok(ip) = host.parse::<std::net::IpAddr>() {
            let addr = SocketAddr::new(ip, port);
            if !endpoints.contains(&addr) {
                endpoints.push(addr);
            }
            continue;
        }
        match (host.as_str(), port).to_socket_addrs() {
            Ok(iter) => {
                for addr in iter {
                    if !endpoints.contains(&addr) {
                        endpoints.push(addr);
                    }
                }
            }
            Err(e) => {
                // Keep going — another candidate may still work.
                let _ = e;
            }
        }
    }

    if endpoints.is_empty() {
        return Err(OmtError::InvalidArgument(format!(
            "could not resolve receiver host(s): {}",
            hosts.join(", ")
        )));
    }

    // Prefer ordinary IPv4, then 172.*, then IPv6 / link-local / loopback.
    endpoints.sort_by_key(endpoint_preference);
    Ok(endpoints)
}

fn endpoint_preference(addr: &SocketAddr) -> u8 {
    match addr.ip() {
        std::net::IpAddr::V4(v4) => {
            let o = v4.octets();
            if v4.is_loopback() {
                5
            } else if o[0] == 169 && o[1] == 254 {
                4
            } else if o[0] == 172 {
                1
            } else {
                0
            }
        }
        std::net::IpAddr::V6(v6) => {
            if v6.is_loopback() {
                5
            } else if (v6.segments()[0] & 0xffc0) == 0xfe80 {
                4
            } else {
                3
            }
        }
    }
}

fn connect_first(
    endpoints: &[SocketAddr],
    timeout: Option<Duration>,
) -> Result<(TcpStream, SocketAddr), OmtError> {
    let mut last_err: Option<OmtError> = None;
    for addr in endpoints {
        match connect(*addr, timeout) {
            Ok(stream) => return Ok((stream, *addr)),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err
        .unwrap_or_else(|| OmtError::Network("connect failed: no endpoints available".into())))
}

fn subscribe_socket(
    stream: &TcpStream,
    config: &ReceiverConfig,
    role: &SocketRole,
) -> Result<(), OmtError> {
    let mut stream = stream.try_clone()?;
    match role {
        SocketRole::Video { decode_audio, .. } => {
            send_subscriptions(
                Some(&mut stream),
                None,
                config.frame_types,
                config.quality,
                config.preview,
                *decode_audio,
            )?;
        }
        SocketRole::Audio { .. } => {
            send_subscriptions(
                None,
                Some(&mut stream),
                config.frame_types,
                config.quality,
                false,
                false,
            )?;
        }
    }
    Ok(())
}

fn interruptible_sleep(stop: &AtomicBool, total: Duration) -> bool {
    let deadline = Instant::now() + total;
    while Instant::now() < deadline {
        if stop.load(Ordering::Acquire) {
            return false;
        }
        let slice = Duration::from_millis(25);
        let rem = deadline.saturating_duration_since(Instant::now());
        thread::sleep(slice.min(rem));
    }
    !stop.load(Ordering::Acquire)
}

fn send_subscriptions(
    av: Option<&mut TcpStream>,
    meta: Option<&mut TcpStream>,
    frame_types: FrameType,
    quality: Quality,
    preview: bool,
    include_audio_on_av: bool,
) -> Result<(), OmtError> {
    let quality_xml = suggested_quality_xml(quality);
    if let Some(stream) = av {
        if frame_types.contains(FrameType::VIDEO) || frame_types.contains(FrameType::METADATA) {
            write_metadata_frame(stream, SUBSCRIBE_METADATA)?;
        }
        // libomtnet order: optional Preview settings, then SubscribeVideo + quality.
        if frame_types.contains(FrameType::VIDEO) {
            write_metadata_frame(stream, if preview { PREVIEW_ON } else { PREVIEW_OFF })?;
            write_metadata_frame(stream, SUBSCRIBE_VIDEO)?;
            write_metadata_frame(stream, &quality_xml)?;
        }
        if include_audio_on_av && frame_types.contains(FrameType::AUDIO) {
            write_metadata_frame(stream, SUBSCRIBE_AUDIO)?;
        }
        stream.flush()?;
    }
    if let Some(stream) = meta
        && frame_types.contains(FrameType::AUDIO)
    {
        write_metadata_frame(stream, SUBSCRIBE_AUDIO)?;
        stream.flush()?;
    }
    Ok(())
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

fn record_bytes(stats: &Mutex<SessionStatistics>, n: usize) {
    if let Ok(mut g) = stats.lock() {
        g.bytes_received = g.bytes_received.saturating_add(n as u64);
        g.bytes_received_since_last = g.bytes_received_since_last.saturating_add(n as u64);
    }
}

fn record_drop_wire(stats: &Mutex<SessionStatistics>) {
    if let Ok(mut g) = stats.lock() {
        g.frames_dropped_wire = g.frames_dropped_wire.saturating_add(1);
    }
}

fn record_drop_decode(stats: &Mutex<SessionStatistics>) {
    if let Ok(mut g) = stats.lock() {
        g.frames_dropped_decode = g.frames_dropped_decode.saturating_add(1);
    }
}

fn record_drop_audio(stats: &Mutex<SessionStatistics>) {
    if let Ok(mut g) = stats.lock() {
        g.frames_dropped_audio = g.frames_dropped_audio.saturating_add(1);
    }
}

fn record_codec_time(stats: &Mutex<SessionStatistics>, ns: u64, age_us: u64) {
    if let Ok(mut g) = stats.lock() {
        g.codec_time_ns = g.codec_time_ns.saturating_add(ns);
        g.frames_decoded = g.frames_decoded.saturating_add(1);
        // Ignore cold-start frames when tracking peak (thread-pool / cache warmup).
        if g.frames_decoded <= 30 || ns > g.codec_time_ns_peak {
            g.codec_time_ns_peak = ns;
        }
        if age_us > g.frame_age_us_peak {
            g.frame_age_us_peak = age_us;
        }
    }
}

fn video_reader_loop(
    mut stream: TcpStream,
    shared: &Shared,
    video_tx: &SyncSender<WireVideo>,
    metadata_tx: &SyncSender<MetadataFrame>,
    audio_tx: &SyncSender<DecodedAudioFrame>,
    decode_audio_here: bool,
) {
    let mut channel = Channel::new(FrameType::VIDEO | FrameType::AUDIO | FrameType::METADATA);
    let mut pool = BufferPool::video();
    let mut read_buf = pool.take(crate::types::NETWORK_RECEIVE_MAX_TRANSFER);
    read_buf.resize(crate::types::NETWORK_RECEIVE_MAX_TRANSFER, 0);

    while !shared.stop.load(Ordering::Acquire) {
        let _ = stream.set_read_timeout(Some(Duration::from_millis(20)));
        match channel.recv_frame_into(&mut stream, &mut read_buf) {
            Ok(Some(frame)) => {
                let nbytes = frame.header.data_length.max(0) as usize + 16;
                record_bytes(&shared.stats, nbytes);
                dispatch_av_frame(
                    frame,
                    video_tx,
                    metadata_tx,
                    audio_tx,
                    shared,
                    decode_audio_here,
                    &mut pool,
                );
            }
            Ok(None) => break,
            Err(OmtError::Io(ref e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) => {}
            Err(OmtError::Protocol(ref msg)) => {
                shared.set_error(format!("protocol: {msg}"));
                record_drop_wire(&shared.stats);
            }
            Err(e) => {
                shared.set_error(format!("video I/O: {e}"));
                break;
            }
        }
    }
    pool.give(read_buf);
}

fn audio_reader_loop(
    mut stream: TcpStream,
    shared: &Shared,
    audio_tx: &SyncSender<DecodedAudioFrame>,
) {
    let mut channel = Channel::new(FrameType::AUDIO);
    let mut pool = BufferPool::audio();
    let mut read_buf = pool.take(crate::types::NETWORK_RECEIVE_MAX_TRANSFER);
    read_buf.resize(crate::types::NETWORK_RECEIVE_MAX_TRANSFER, 0);
    let mut pcm_scratch = pool.take_audio();

    while !shared.stop.load(Ordering::Acquire) {
        let _ = stream.set_read_timeout(Some(Duration::from_millis(10)));
        match channel.recv_frame_into(&mut stream, &mut read_buf) {
            Ok(Some(frame)) => {
                let nbytes = frame.header.data_length.max(0) as usize + 16;
                record_bytes(&shared.stats, nbytes);
                if let Some(decoded) = decode_audio_frame(frame, &mut pcm_scratch) {
                    match audio_tx.try_send(decoded) {
                        Ok(()) => {}
                        Err(TrySendError::Full(_)) => record_drop_audio(&shared.stats),
                        Err(TrySendError::Disconnected(_)) => break,
                    }
                } else {
                    record_drop_audio(&shared.stats);
                }
            }
            Ok(None) => break,
            Err(OmtError::Io(ref e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) => {}
            Err(e) => {
                shared.set_error(format!("audio I/O: {e}"));
                break;
            }
        }
    }
    pool.give(read_buf);
    pool.give(pcm_scratch);
}

fn dispatch_av_frame(
    frame: AssembledFrame,
    video_tx: &SyncSender<WireVideo>,
    metadata_tx: &SyncSender<MetadataFrame>,
    audio_tx: &SyncSender<DecodedAudioFrame>,
    shared: &Shared,
    decode_audio_here: bool,
    pool: &mut BufferPool,
) {
    let meta = if frame.metadata.is_empty() {
        None
    } else {
        crate::protocol::metadata::decode_metadata_xml(&frame.metadata)
            .ok()
            .map(Arc::<str>::from)
    };

    if frame.header.frame_type.contains(FrameType::VIDEO) {
        let Some(v) = frame.video else {
            record_drop_wire(&shared.stats);
            return;
        };
        if v.codec != Codec::Vmx1 {
            record_drop_wire(&shared.stats);
            return;
        }
        // Preview is decoded via decode_preview_bgra; alpha / HBD still unsupported.
        if v.flags.contains(VideoFlags::ALPHA) || v.flags.contains(VideoFlags::HIGH_BIT_DEPTH) {
            record_drop_wire(&shared.stats);
            return;
        }
        if frame.data.len() > VIDEO_MAX_SIZE {
            record_drop_wire(&shared.stats);
            return;
        }
        let mut payload = pool.take(frame.data.len().max(1));
        payload.clear();
        payload.extend_from_slice(&frame.data);
        let wire = WireVideo {
            timestamp: frame.header.timestamp,
            width: v.width,
            height: v.height,
            frame_rate_n: v.frame_rate_n,
            frame_rate_d: v.frame_rate_d.max(1),
            color_space: v.color_space,
            preview: v.flags.contains(VideoFlags::PREVIEW),
            payload,
            metadata: meta,
            enqueued_at: Instant::now(),
        };
        match video_tx.try_send(wire) {
            Ok(()) => {
                shared.wire_depth.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Full(wire)) => {
                record_drop_wire(&shared.stats);
                pool.give(wire.payload);
            }
            Err(TrySendError::Disconnected(wire)) => {
                pool.give(wire.payload);
            }
        }
        return;
    }

    if frame.header.frame_type.contains(FrameType::AUDIO) && decode_audio_here {
        let mut scratch = pool.take_audio();
        if let Some(decoded) = decode_audio_frame(frame, &mut scratch) {
            match audio_tx.try_send(decoded) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => record_drop_audio(&shared.stats),
                Err(TrySendError::Disconnected(_)) => {}
            }
        } else {
            record_drop_audio(&shared.stats);
        }
        pool.give(scratch);
        return;
    }

    if frame.header.frame_type.contains(FrameType::METADATA) {
        let xml = meta.unwrap_or_else(|| {
            let raw = if frame.data.is_empty() {
                String::new()
            } else {
                String::from_utf8_lossy(&frame.data).into_owned()
            };
            Arc::<str>::from(raw)
        });
        let mf = MetadataFrame {
            timestamp: frame.header.timestamp,
            xml,
        };
        match metadata_tx.try_send(mf) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => {}
        }
    }
}

fn decode_audio_frame(
    frame: AssembledFrame,
    pcm_scratch: &mut Vec<u8>,
) -> Option<DecodedAudioFrame> {
    let a = frame.audio?;
    if a.codec != Codec::Fpa1 {
        return None;
    }
    if frame.data.len() > AUDIO_MAX_SIZE {
        return None;
    }
    let channels = a.channels.max(0) as usize;
    let samples = a.samples_per_channel.max(0) as usize;
    fpa1::decode_planar_into(
        &frame.data,
        channels,
        samples,
        a.active_channels,
        pcm_scratch,
    )
    .ok()?;
    Some(DecodedAudioFrame {
        timestamp: frame.header.timestamp,
        sample_rate: a.sample_rate,
        channels: a.channels,
        samples_per_channel: a.samples_per_channel,
        active_channels: a.active_channels,
        pcm_planar_f32: Arc::from(pcm_scratch.as_slice()),
        frame_metadata: None,
    })
}

fn video_decode_loop(shared: Arc<Shared>, wire_rx: MpscReceiver<WireVideo>) {
    let mut codec: Option<vmx::Codec> = None;
    let mut decode_buf = Vec::new();
    let mut pool = BufferPool::video();

    while !shared.stop.load(Ordering::Acquire) {
        let wire = match wire_rx.recv_timeout(Duration::from_millis(20)) {
            Ok(w) => {
                let _ = shared
                    .wire_depth
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                        Some(v.saturating_sub(1))
                    });
                w
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };

        let age_us = wire.enqueued_at.elapsed().as_micros() as u64;
        let t0 = Instant::now();
        let WireVideo {
            timestamp,
            width,
            height,
            frame_rate_n,
            frame_rate_d,
            color_space,
            preview,
            payload,
            metadata,
            ..
        } = wire;
        match decode_vmx_bgra(
            width,
            height,
            color_space,
            timestamp,
            frame_rate_n,
            frame_rate_d,
            preview,
            metadata,
            payload.as_slice(),
            &mut codec,
            &mut decode_buf,
        ) {
            Ok(frame) => {
                let ns = t0.elapsed().as_nanos() as u64;
                record_codec_time(&shared.stats, ns, age_us);
                shared.video.publish(frame, &shared.stats);
            }
            Err(e) => {
                shared.set_error(format!("decode: {e}"));
                record_drop_decode(&shared.stats);
                codec = None;
            }
        }
        pool.give(payload);
    }
}

#[allow(clippy::too_many_arguments)]
fn decode_vmx_bgra(
    width: i32,
    height: i32,
    color_space: ColorSpace,
    timestamp: i64,
    frame_rate_n: i32,
    frame_rate_d: i32,
    preview: bool,
    metadata: Option<Arc<str>>,
    payload: &[u8],
    cached: &mut Option<vmx::Codec>,
    decode_buf: &mut Vec<u8>,
) -> Result<DecodedVideoFrame, OmtError> {
    if width < vmx::MIN_WIDTH || height < vmx::MIN_HEIGHT {
        return Err(OmtError::Codec("VMX dimensions below minimum".into()));
    }
    let vmx_cs = match color_space {
        ColorSpace::Bt601 => vmx::ColorSpace::Bt601,
        ColorSpace::Bt709 => vmx::ColorSpace::Bt709,
        ColorSpace::Undefined => vmx::ColorSpace::Undefined,
    };

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let reuse = cached.as_ref().is_some_and(|c| {
            let s = c.size();
            s.width == width && s.height == height
        });
        if !reuse {
            *cached = Some(vmx::Codec::new(vmx::Config {
                width,
                height,
                profile: vmx::Profile::OmtSq,
                color_space: vmx_cs,
            })?);
        } else if let Some(c) = cached.as_mut() {
            c.set_color_space(vmx_cs);
        }
        let codec = cached.as_mut().unwrap();
        codec.load_from(payload)?;
        let (out_w, out_h) = if preview {
            let ps = codec.preview_size();
            (ps.width as u32, ps.height as u32)
        } else {
            (width as u32, height as u32)
        };
        let stride = (out_w as usize) * 4;
        let need = stride * out_h as usize;
        decode_buf.resize(need, 0);
        if preview {
            codec.decode_preview_bgra(decode_buf, stride)?;
        } else {
            codec.decode_bgra(decode_buf, stride)?;
        }
        let pixels: Arc<[u8]> = Arc::from(decode_buf.as_slice());
        Ok::<_, OmtError>(DecodedVideoFrame {
            width: out_w,
            height: out_h,
            stride: stride as u32,
            timestamp,
            frame_rate_n,
            frame_rate_d,
            color_space,
            pixels,
            frame_metadata: metadata,
        })
    }));

    match result {
        Ok(inner) => inner,
        Err(_) => {
            *cached = None;
            Err(OmtError::Codec("VMX decode panicked".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoded_video_queue_drops_oldest_when_full() {
        let slot = DecodedVideoQueue::new(1);
        let mk = |ts| DecodedVideoFrame {
            width: 2,
            height: 2,
            stride: 8,
            timestamp: ts,
            frame_rate_n: 60,
            frame_rate_d: 1,
            color_space: ColorSpace::Bt709,
            pixels: Arc::from([0u8; 16].as_slice()),
            frame_metadata: None,
        };
        let stats = Mutex::new(SessionStatistics::default());
        slot.publish(mk(1), &stats);
        slot.publish(mk(2), &stats);
        assert_eq!(slot.overwrites.load(Ordering::Relaxed), 1);
        assert_eq!(stats.lock().unwrap().frames_dropped_decode, 1);
        let f = slot.try_take().unwrap();
        assert_eq!(f.timestamp, 2);
        assert!(slot.try_take().is_none());
    }

    #[test]
    fn decoded_video_queue_is_fifo() {
        let slot = DecodedVideoQueue::new(4);
        let mk = |ts| DecodedVideoFrame {
            width: 2,
            height: 2,
            stride: 8,
            timestamp: ts,
            frame_rate_n: 60,
            frame_rate_d: 1,
            color_space: ColorSpace::Bt709,
            pixels: Arc::from([0u8; 16].as_slice()),
            frame_metadata: None,
        };
        let stats = Mutex::new(SessionStatistics::default());
        slot.publish(mk(1), &stats);
        slot.publish(mk(2), &stats);
        assert_eq!(slot.try_take().unwrap().timestamp, 1);
        assert_eq!(slot.try_take().unwrap().timestamp, 2);
    }

    #[test]
    fn interruptible_sleep_stops_early() {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_c = Arc::clone(&stop);
        let t = thread::spawn(move || {
            thread::sleep(Duration::from_millis(30));
            stop_c.store(true, Ordering::Release);
        });
        let ok = interruptible_sleep(&stop, Duration::from_secs(5));
        assert!(!ok);
        t.join().unwrap();
    }
}
