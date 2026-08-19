//! OMT Discovery Server (port 6399 by default) — infrastructure.
//!
//! Relays `<OMTAddress>` metadata between connected clients, matching
//! libomtnet `OMTDiscoveryServer` and the official console app
//! (`IPAddress.IPv6Any`, default port 6399).

use std::collections::HashMap;
use std::io::Write;
use std::net::{Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender as EventSender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::discovery::address::OmtAddress;
use crate::error::OmtError;
use crate::protocol::frame::{AssembledFrame, FrameHeader, PROTOCOL_VERSION};
use crate::protocol::metadata::{decode_metadata_xml, encode_metadata_xml};
use crate::transport::channel::Channel;
use crate::transport::socket::{configure_stream, into_listener, listen};
use crate::types::{DISCOVERY_SERVER_DEFAULT_PORT, FrameType};

/// Official default bind: dual-stack IPv6 any (`[::]:port`), matching
/// `new IPEndPoint(IPAddress.IPv6Any, port)`.
pub fn default_bind_addr(port: u16) -> SocketAddr {
    SocketAddr::from((Ipv6Addr::UNSPECIFIED, port))
}

#[derive(Clone, Debug)]
struct Entry {
    address: OmtAddress,
    owner: SocketAddr,
}

struct Shared {
    entries: Vec<Entry>,
    peers: HashMap<SocketAddr, TcpStream>,
    peer_threads: Vec<JoinHandle<()>>,
    bound: Option<SocketAddr>,
}

impl Shared {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
            peers: HashMap::new(),
            peer_threads: Vec::new(),
            bound: None,
        }
    }

    fn reap_peers(&mut self) {
        let mut live = Vec::new();
        for handle in self.peer_threads.drain(..) {
            if handle.is_finished() {
                let _ = handle.join();
            } else {
                live.push(handle);
            }
        }
        self.peer_threads = live;
    }
}

/// Point-in-time view of a running discovery server.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiscoveryServerSnapshot {
    /// Address the server is bound to, if listening.
    pub bind: Option<SocketAddr>,
    /// True while the accept loop is running.
    pub running: bool,
    /// Connected TCP peers.
    pub peers: Vec<SocketAddr>,
    /// Currently registered sources.
    pub sources: Vec<OmtAddress>,
}

impl DiscoveryServerSnapshot {
    /// Number of connected clients.
    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }
}

/// Lifecycle and registry notifications for a discovery server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiscoveryServerEvent {
    /// Accept loop started.
    Started {
        /// Bound listen address.
        bind: SocketAddr,
    },
    /// A client TCP connection was accepted.
    ClientConnected {
        /// Remote address.
        peer: SocketAddr,
    },
    /// A client disconnected (or the connection was dropped).
    ClientDisconnected {
        /// Remote address.
        peer: SocketAddr,
    },
    /// A source was registered and relayed.
    SourceRegistered {
        /// Registered address (IPs replaced with the TCP peer).
        address: OmtAddress,
        /// Client that registered the source.
        peer: SocketAddr,
    },
    /// A source was removed (explicit deregister or client disconnect).
    SourceRemoved {
        /// Removed address (`removed == true`).
        address: OmtAddress,
        /// Client that owned the source.
        peer: SocketAddr,
    },
    /// Non-fatal or terminal error.
    Error {
        /// Human-readable message.
        message: String,
    },
    /// Accept loop exited after [`DiscoveryServerHandle::stop`].
    Stopped,
}

struct HandleInner {
    bind: Mutex<SocketAddr>,
    stop: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    shared: Arc<Mutex<Shared>>,
    events_tx: EventSender<DiscoveryServerEvent>,
    events_rx: Mutex<Receiver<DiscoveryServerEvent>>,
    accept_thread: Mutex<Option<JoinHandle<Result<(), OmtError>>>>,
}

/// Observable start/stop handle for GUI and CLI embedding.
///
/// This is the preferred API for tools: bind, start a background accept loop,
/// poll events, read a snapshot, then stop and join.
#[derive(Clone)]
pub struct DiscoveryServerHandle {
    inner: Arc<HandleInner>,
}

impl DiscoveryServerHandle {
    /// Handle that will bind dual-stack `[::]:6399` when started.
    pub fn new() -> Self {
        Self::with_bind(default_bind_addr(DISCOVERY_SERVER_DEFAULT_PORT))
    }

    /// Handle that will bind dual-stack `[::]:port` when started.
    pub fn with_port(port: u16) -> Self {
        Self::with_bind(default_bind_addr(port))
    }

    /// Handle that will bind `bind` when started.
    pub fn with_bind(bind: SocketAddr) -> Self {
        let (events_tx, events_rx) = mpsc::channel();
        Self {
            inner: Arc::new(HandleInner {
                bind: Mutex::new(bind),
                stop: Arc::new(AtomicBool::new(false)),
                running: Arc::new(AtomicBool::new(false)),
                shared: Arc::new(Mutex::new(Shared::new())),
                events_tx,
                events_rx: Mutex::new(events_rx),
                accept_thread: Mutex::new(None),
            }),
        }
    }

    /// Configured bind address (before start) or the last requested bind.
    pub fn bind(&self) -> SocketAddr {
        self.inner
            .bind
            .lock()
            .map(|g| *g)
            .unwrap_or_else(|_| default_bind_addr(DISCOVERY_SERVER_DEFAULT_PORT))
    }

    /// Replace the bind address. Ignored while the server is running.
    pub fn set_bind(&self, bind: SocketAddr) -> Result<(), OmtError> {
        if self.is_running() {
            return Err(OmtError::Discovery(
                "cannot change bind address while the discovery server is running".into(),
            ));
        }
        *self
            .inner
            .bind
            .lock()
            .map_err(|_| OmtError::Discovery("discovery server lock poisoned".into()))? = bind;
        Ok(())
    }

    /// True while the accept loop is running.
    pub fn is_running(&self) -> bool {
        self.inner.running.load(Ordering::SeqCst)
    }

    /// Bind and spawn the accept loop on a background thread.
    pub fn start(&self) -> Result<(), OmtError> {
        self.reap_accept_thread()?;
        let bind = self.bind();
        let listener = into_listener(listen(bind)?)?;
        let actual = listener.local_addr()?;
        *self
            .inner
            .bind
            .lock()
            .map_err(|_| OmtError::Discovery("discovery server lock poisoned".into()))? = actual;

        self.inner.stop.store(false, Ordering::SeqCst);
        self.inner.running.store(true, Ordering::SeqCst);

        let stop = Arc::clone(&self.inner.stop);
        let running = Arc::clone(&self.inner.running);
        let shared = Arc::clone(&self.inner.shared);
        let events_tx = self.inner.events_tx.clone();
        let handle = thread::spawn(move || {
            let result = serve_listener(
                listener,
                stop,
                Arc::clone(&running),
                shared,
                events_tx.clone(),
            );
            if let Err(ref e) = result {
                let _ = events_tx.send(DiscoveryServerEvent::Error {
                    message: e.to_string(),
                });
            }
            running.store(false, Ordering::SeqCst);
            let _ = events_tx.send(DiscoveryServerEvent::Stopped);
            result
        });
        *self
            .inner
            .accept_thread
            .lock()
            .map_err(|_| OmtError::Discovery("discovery server lock poisoned".into()))? =
            Some(handle);
        Ok(())
    }

    /// Request the accept loop to exit. Does not wait; see [`Self::join`].
    pub fn stop(&self) {
        self.inner.stop.store(true, Ordering::SeqCst);
    }

    /// Stop the server (if running) and wait for the accept loop to finish.
    pub fn join(&self) -> Result<(), OmtError> {
        self.stop();
        let handle = self
            .inner
            .accept_thread
            .lock()
            .map_err(|_| OmtError::Discovery("discovery server lock poisoned".into()))?
            .take();
        if let Some(handle) = handle {
            match handle.join() {
                Ok(result) => result,
                Err(_) => Err(OmtError::Discovery(
                    "discovery server thread panicked".into(),
                )),
            }
        } else {
            Ok(())
        }
    }

    /// Connected peers and registered sources.
    pub fn snapshot(&self) -> DiscoveryServerSnapshot {
        snapshot(
            &self.inner.shared,
            self.is_running(),
            self.is_running().then_some(self.bind()),
        )
    }

    /// Pop the next event if one is queued.
    pub fn poll_event(&self) -> Option<DiscoveryServerEvent> {
        self.inner
            .events_rx
            .lock()
            .ok()
            .and_then(|rx| rx.try_recv().ok())
    }

    /// Drain queued events.
    pub fn drain_events(&self) -> Vec<DiscoveryServerEvent> {
        let mut out = Vec::new();
        while let Some(event) = self.poll_event() {
            out.push(event);
        }
        out
    }

    fn reap_accept_thread(&self) -> Result<(), OmtError> {
        let mut slot = self
            .inner
            .accept_thread
            .lock()
            .map_err(|_| OmtError::Discovery("discovery server lock poisoned".into()))?;
        if let Some(handle) = slot.as_ref()
            && !handle.is_finished()
        {
            return Err(OmtError::Discovery(
                "discovery server is already running".into(),
            ));
        }
        if let Some(handle) = slot.take() {
            match handle.join() {
                Ok(result) => result,
                Err(_) => Err(OmtError::Discovery(
                    "discovery server thread panicked".into(),
                )),
            }
        } else {
            Ok(())
        }
    }
}

impl Default for DiscoveryServerHandle {
    fn default() -> Self {
        Self::new()
    }
}

/// OMT discovery server that relays registration XML over OMT metadata frames.
///
/// Blocking API used by tests and in-process embeds. Prefer
/// [`DiscoveryServerHandle`] for GUI/CLI tools that need start/stop and
/// snapshots.
#[derive(Debug)]
pub struct DiscoveryServer {
    /// Listen port.
    pub port: u16,
    bind: SocketAddr,
    stop: Arc<AtomicBool>,
}

impl DiscoveryServer {
    /// Create a server on the default port (6399), dual-stack IPv6 any.
    pub fn new() -> Self {
        Self::with_port(DISCOVERY_SERVER_DEFAULT_PORT)
    }

    /// Create a server on a custom port, dual-stack IPv6 any.
    pub fn with_port(port: u16) -> Self {
        Self::with_bind(default_bind_addr(port))
    }

    /// Create a server that binds `bind`.
    pub fn with_bind(bind: SocketAddr) -> Self {
        Self {
            port: bind.port(),
            bind,
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Bind and serve until [`DiscoveryServer::stop`] is called (blocking accept loop).
    pub fn run(&mut self) -> Result<(), OmtError> {
        let listener = into_listener(listen(self.bind)?)?;
        self.run_with_listener(listener)
    }

    /// Serve using an already-bound listener.
    pub fn run_with_listener(&mut self, listener: TcpListener) -> Result<(), OmtError> {
        self.stop.store(false, Ordering::SeqCst);
        let running = Arc::new(AtomicBool::new(true));
        let shared = Arc::new(Mutex::new(Shared::new()));
        let (events_tx, _events_rx) = mpsc::channel();
        let result = serve_listener(
            listener,
            Arc::clone(&self.stop),
            Arc::clone(&running),
            shared,
            events_tx,
        );
        running.store(false, Ordering::SeqCst);
        result
    }

    /// Request the accept loop to exit.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Default for DiscoveryServer {
    fn default() -> Self {
        Self::new()
    }
}

fn snapshot(
    shared: &Arc<Mutex<Shared>>,
    running: bool,
    bind: Option<SocketAddr>,
) -> DiscoveryServerSnapshot {
    let Ok(g) = shared.lock() else {
        return DiscoveryServerSnapshot {
            bind,
            running,
            ..DiscoveryServerSnapshot::default()
        };
    };
    DiscoveryServerSnapshot {
        bind: bind.or(g.bound),
        running,
        peers: g.peers.keys().copied().collect(),
        sources: g.entries.iter().map(|e| e.address.clone()).collect(),
    }
}

fn emit(tx: &EventSender<DiscoveryServerEvent>, event: DiscoveryServerEvent) {
    let _ = tx.send(event);
}

fn serve_listener(
    listener: TcpListener,
    stop: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    shared: Arc<Mutex<Shared>>,
    events_tx: EventSender<DiscoveryServerEvent>,
) -> Result<(), OmtError> {
    listener.set_nonblocking(true)?;
    let bound = listener.local_addr().ok();
    {
        let mut g = shared
            .lock()
            .map_err(|_| OmtError::Discovery("discovery server lock poisoned".into()))?;
        g.bound = bound;
        g.entries.clear();
        g.peers.clear();
    }
    running.store(true, Ordering::SeqCst);
    if let Some(bind) = bound {
        emit(&events_tx, DiscoveryServerEvent::Started { bind });
    }

    let result = accept_loop(&listener, &stop, &shared, &events_tx);
    stop.store(true, Ordering::SeqCst);
    join_peers(&shared);
    if let Ok(mut g) = shared.lock() {
        g.entries.clear();
        g.peers.clear();
        g.bound = None;
    }
    running.store(false, Ordering::SeqCst);
    result
}

fn accept_loop(
    listener: &TcpListener,
    stop: &Arc<AtomicBool>,
    shared: &Arc<Mutex<Shared>>,
    events_tx: &EventSender<DiscoveryServerEvent>,
) -> Result<(), OmtError> {
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, peer)) => {
                if let Err(e) = accept_peer(stream, peer, shared, stop, events_tx) {
                    emit(
                        events_tx,
                        DiscoveryServerEvent::Error {
                            message: e.to_string(),
                        },
                    );
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

fn accept_peer(
    stream: TcpStream,
    peer: SocketAddr,
    shared: &Arc<Mutex<Shared>>,
    stop: &Arc<AtomicBool>,
    events_tx: &EventSender<DiscoveryServerEvent>,
) -> Result<(), OmtError> {
    let _ = configure_stream(&stream);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_nonblocking(false);

    let snapshot_addrs: Vec<OmtAddress>;
    {
        let mut g = shared
            .lock()
            .map_err(|_| OmtError::Discovery("discovery server lock poisoned".into()))?;
        g.reap_peers();
        snapshot_addrs = g.entries.iter().map(|e| e.address.clone()).collect();
        g.peers.insert(peer, stream.try_clone()?);
    }
    for addr in snapshot_addrs {
        let _ = send_address_to(&stream, &addr);
    }
    emit(events_tx, DiscoveryServerEvent::ClientConnected { peer });

    let shared_c = Arc::clone(shared);
    let stop_c = Arc::clone(stop);
    let events_c = events_tx.clone();
    let handle = thread::spawn(move || {
        peer_loop(stream, peer, shared_c, stop_c, events_c);
    });
    let mut g = shared
        .lock()
        .map_err(|_| OmtError::Discovery("discovery server lock poisoned".into()))?;
    g.peer_threads.push(handle);
    Ok(())
}

fn join_peers(shared: &Arc<Mutex<Shared>>) {
    let threads = if let Ok(mut g) = shared.lock() {
        g.peer_threads.drain(..).collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    for handle in threads {
        let _ = handle.join();
    }
}

fn peer_loop(
    mut stream: TcpStream,
    peer: SocketAddr,
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    events_tx: EventSender<DiscoveryServerEvent>,
) {
    let mut channel = Channel::new(FrameType::METADATA);
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
    while !stop.load(Ordering::SeqCst) {
        match channel.recv_frame(&mut stream) {
            Ok(Some(frame)) => {
                if let Ok(xml) = metadata_xml_from_frame(&frame)
                    && let Ok(mut addr) = OmtAddress::from_xml(&xml)
                {
                    let _ = handle_address(&shared, peer, &mut addr, &events_tx);
                }
            }
            Ok(None) => break, // EOF
            Err(OmtError::Io(ref e))
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(OmtError::Network(_)) => break,
            Err(_) => break,
        }
    }
    disconnect_peer(&shared, peer, &events_tx);
}

fn disconnect_peer(
    shared: &Arc<Mutex<Shared>>,
    peer: SocketAddr,
    events_tx: &EventSender<DiscoveryServerEvent>,
) {
    let Ok(mut g) = shared.lock() else {
        return;
    };
    let removed: Vec<OmtAddress> = g
        .entries
        .iter()
        .filter(|e| e.owner == peer)
        .map(|e| {
            let mut a = e.address.clone();
            a.removed = true;
            a
        })
        .collect();
    g.entries.retain(|e| e.owner != peer);
    g.peers.remove(&peer);
    let peers: Vec<TcpStream> = g
        .peers
        .values()
        .filter_map(|s| s.try_clone().ok())
        .collect();
    drop(g);
    for addr in removed {
        for p in &peers {
            let _ = send_address_to(p, &addr);
        }
        emit(
            events_tx,
            DiscoveryServerEvent::SourceRemoved {
                address: addr,
                peer,
            },
        );
    }
    emit(events_tx, DiscoveryServerEvent::ClientDisconnected { peer });
}

fn handle_address(
    shared: &Arc<Mutex<Shared>>,
    peer: SocketAddr,
    addr: &mut OmtAddress,
    events_tx: &EventSender<DiscoveryServerEvent>,
) -> Result<(), OmtError> {
    let mut g = shared
        .lock()
        .map_err(|_| OmtError::Discovery("discovery server lock poisoned".into()))?;
    let key = (addr.instance_name(), addr.port);
    let existing = g
        .entries
        .iter()
        .position(|e| e.address.instance_name() == key.0 && e.address.port == key.1);

    if addr.removed {
        if let Some(idx) = existing {
            let mut removed = g.entries.remove(idx).address;
            removed.removed = true;
            let peers: Vec<TcpStream> = g
                .peers
                .values()
                .filter_map(|s| s.try_clone().ok())
                .collect();
            drop(g);
            for p in peers {
                let _ = send_address_to(&p, &removed);
            }
            emit(
                events_tx,
                DiscoveryServerEvent::SourceRemoved {
                    address: removed,
                    peer,
                },
            );
        }
        return Ok(());
    }

    if existing.is_none() {
        // libomtnet clears client-provided IPs and uses the TCP peer address.
        addr.addresses.clear();
        addr.addresses.push(match peer.ip() {
            std::net::IpAddr::V4(v) => v.to_string(),
            std::net::IpAddr::V6(v) => v.to_string(),
        });
        g.entries.push(Entry {
            address: addr.clone(),
            owner: peer,
        });
        let peers: Vec<TcpStream> = g
            .peers
            .values()
            .filter_map(|s| s.try_clone().ok())
            .collect();
        let to_send = addr.clone();
        drop(g);
        for p in peers {
            let _ = send_address_to(&p, &to_send);
        }
        emit(
            events_tx,
            DiscoveryServerEvent::SourceRegistered {
                address: to_send,
                peer,
            },
        );
    }
    Ok(())
}

fn metadata_xml_from_frame(frame: &AssembledFrame) -> Result<String, OmtError> {
    if !frame.metadata.is_empty() {
        decode_metadata_xml(&frame.metadata)
    } else {
        decode_metadata_xml(&frame.data)
    }
}

fn send_address_to(stream: &TcpStream, addr: &OmtAddress) -> Result<(), OmtError> {
    let mut stream = stream.try_clone()?;
    let data = encode_metadata_xml(&addr.to_xml());
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
    stream.flush()?;
    Ok(())
}

/// Helper used by tests / clients: read one metadata XML with timeout.
pub(crate) fn try_recv_metadata(
    stream: &mut TcpStream,
    channel: &mut Channel,
    timeout: Duration,
) -> Result<Option<String>, OmtError> {
    let deadline = std::time::Instant::now() + timeout;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
    while std::time::Instant::now() < deadline {
        match channel.recv_frame(stream) {
            Ok(Some(frame)) => return Ok(Some(metadata_xml_from_frame(&frame)?)),
            Ok(None) => return Ok(None),
            Err(OmtError::Io(ref e))
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                thread::sleep(Duration::from_millis(10));
            }
            Err(e) => return Err(e),
        }
    }
    Err(OmtError::Io(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "metadata receive timed out",
    )))
}

/// Build a metadata frame bytes for an address XML.
pub(crate) fn address_frame_bytes(addr: &OmtAddress) -> Vec<u8> {
    let data = encode_metadata_xml(&addr.to_xml());
    AssembledFrame {
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
    }
    .to_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::client::DiscoveryClient;

    fn wait_until(timeout: Duration, mut pred: impl FnMut() -> bool) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if pred() {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        pred()
    }

    fn start_handle() -> DiscoveryServerHandle {
        let handle = DiscoveryServerHandle::with_bind(SocketAddr::from(([127, 0, 0, 1], 0)));
        handle.start().expect("start discovery server");
        assert!(wait_until(Duration::from_secs(2), || handle.is_running()));
        handle
    }

    fn connect_client(handle: &DiscoveryServerHandle) -> DiscoveryClient {
        let port = handle.bind().port();
        let mut client = DiscoveryClient::new("127.0.0.1");
        client.port = port;
        client.connect().unwrap();
        client
    }

    fn source(name: &str, port: u16) -> OmtAddress {
        let mut addr = OmtAddress::from_full_name(name, port);
        addr.addresses = vec!["10.0.0.1".into()];
        addr
    }

    #[test]
    fn default_bind_is_ipv6_any_like_libomtnet() {
        let addr = default_bind_addr(DISCOVERY_SERVER_DEFAULT_PORT);
        assert!(addr.ip().is_unspecified());
        assert!(addr.is_ipv6());
        assert_eq!(addr.port(), DISCOVERY_SERVER_DEFAULT_PORT);
    }

    #[test]
    fn discovery_server_relays_register() {
        let handle = start_handle();
        let mut client = connect_client(&handle);
        client.register(&source("TESTHOST (Cam1)", 6400)).unwrap();

        let found = wait_until(Duration::from_secs(5), || {
            client.sources().iter().any(|s| {
                s.instance_name() == "TESTHOST (Cam1)"
                    && s.port == 6400
                    && s.addresses
                        .iter()
                        .any(|a| a == "127.0.0.1" || a == "::1" || a.starts_with("127."))
            })
        });
        handle.join().unwrap();
        assert!(found, "expected relayed OMTAddress from discovery server");
    }

    #[test]
    fn snapshot_tracks_peers_and_sources() {
        let handle = start_handle();
        let mut client = connect_client(&handle);
        assert!(wait_until(Duration::from_secs(2), || {
            handle.snapshot().peer_count() == 1
        }));

        client.register(&source("SNAPHOST (Cam1)", 6410)).unwrap();
        assert!(wait_until(Duration::from_secs(5), || {
            handle
                .snapshot()
                .sources
                .iter()
                .any(|s| s.instance_name() == "SNAPHOST (Cam1)" && s.port == 6410)
        }));

        let snap = handle.snapshot();
        assert!(snap.running);
        assert_eq!(snap.peer_count(), 1);
        drop(client);
        assert!(wait_until(Duration::from_secs(3), || {
            handle.snapshot().peer_count() == 0 && handle.snapshot().sources.is_empty()
        }));
        handle.join().unwrap();
    }

    #[test]
    fn new_peer_receives_existing_registrations() {
        let handle = start_handle();
        let mut first = connect_client(&handle);
        first.register(&source("HOSTA (Cam1)", 6420)).unwrap();
        assert!(wait_until(Duration::from_secs(5), || handle
            .snapshot()
            .sources
            .iter()
            .any(|s| s.instance_name() == "HOSTA (Cam1)")));

        let second = connect_client(&handle);
        let found = wait_until(Duration::from_secs(5), || {
            second
                .sources()
                .iter()
                .any(|s| s.instance_name() == "HOSTA (Cam1)" && s.port == 6420)
        });
        handle.join().unwrap();
        assert!(found, "new client should receive the existing source list");
    }

    #[test]
    fn disconnect_broadcasts_removal() {
        let handle = start_handle();
        let mut first = connect_client(&handle);
        first.register(&source("HOSTB (Cam1)", 6430)).unwrap();
        let second = connect_client(&handle);
        assert!(wait_until(Duration::from_secs(5), || second
            .sources()
            .iter()
            .any(|s| s.instance_name() == "HOSTB (Cam1)")));

        drop(first);
        let removed = wait_until(Duration::from_secs(5), || {
            second
                .sources()
                .iter()
                .all(|s| s.instance_name() != "HOSTB (Cam1)")
        });
        handle.join().unwrap();
        assert!(
            removed,
            "remaining clients should drop the disconnected source"
        );
    }

    #[test]
    fn handle_stop_joins_and_clears_state() {
        let handle = start_handle();
        let mut client = connect_client(&handle);
        client.register(&source("HOSTC (Cam1)", 6440)).unwrap();
        assert!(wait_until(Duration::from_secs(5), || !handle
            .snapshot()
            .sources
            .is_empty()));

        handle.join().unwrap();
        assert!(!handle.is_running());
        let snap = handle.snapshot();
        assert!(snap.sources.is_empty());
        assert_eq!(snap.peer_count(), 0);
        assert!(
            handle
                .drain_events()
                .iter()
                .any(|e| matches!(e, DiscoveryServerEvent::Stopped))
        );
    }

    #[test]
    fn handle_starts_and_stops_on_loopback() {
        let handle = DiscoveryServerHandle::with_bind(SocketAddr::from(([127, 0, 0, 1], 0)));
        handle.start().expect("start");
        assert!(handle.is_running());
        assert_ne!(handle.bind().port(), 0);
        handle.join().expect("join");
        assert!(!handle.is_running());
    }

    #[test]
    fn custom_bind_rejects_second_listener() {
        let occupied = TcpListener::bind("127.0.0.1:0").unwrap();
        let bind = occupied.local_addr().unwrap();
        let handle = DiscoveryServerHandle::with_bind(bind);
        let err = handle.start().unwrap_err();
        drop(occupied);
        assert!(
            matches!(err, OmtError::Io(_) | OmtError::Network(_)),
            "second bind should fail: {err}"
        );
    }

    #[test]
    fn events_include_connect_register_and_disconnect() {
        let handle = start_handle();
        let mut client = connect_client(&handle);
        client.register(&source("HOSTD (Cam1)", 6450)).unwrap();
        assert!(wait_until(Duration::from_secs(5), || handle
            .snapshot()
            .sources
            .iter()
            .any(|s| s.instance_name() == "HOSTD (Cam1)")));
        drop(client);
        assert!(wait_until(Duration::from_secs(3), || handle
            .snapshot()
            .peer_count()
            == 0));

        let events = handle.drain_events();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, DiscoveryServerEvent::Started { .. }))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, DiscoveryServerEvent::ClientConnected { .. }))
        );
        assert!(events.iter().any(|e| matches!(
            e,
            DiscoveryServerEvent::SourceRegistered { address, .. }
                if address.instance_name() == "HOSTD (Cam1)"
        )));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, DiscoveryServerEvent::ClientDisconnected { .. }))
        );
        handle.join().unwrap();
    }
}
