//! OMT Discovery Server (port 6399 by default) — infrastructure.
//!
//! Relays `<OMTAddress>` metadata between connected clients, matching
//! libomtnet `OMTDiscoveryServer`.

use std::collections::HashMap;
use std::io::Write;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
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

#[derive(Clone, Debug)]
struct Entry {
    address: OmtAddress,
    owner: SocketAddr,
}

struct Shared {
    entries: Vec<Entry>,
    peers: HashMap<SocketAddr, TcpStream>,
}

/// OMT discovery server that relays registration XML over OMT metadata frames.
#[derive(Debug)]
pub struct DiscoveryServer {
    /// Listen port.
    pub port: u16,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl DiscoveryServer {
    /// Create a server on the default port (6399).
    pub fn new() -> Self {
        Self::with_port(DISCOVERY_SERVER_DEFAULT_PORT)
    }

    /// Create a server on a custom port.
    pub fn with_port(port: u16) -> Self {
        Self {
            port,
            stop: Arc::new(AtomicBool::new(false)),
            threads: Vec::new(),
        }
    }

    /// Bind and serve until [`DiscoveryServer::stop`] is called (blocking accept loop).
    pub fn run(&mut self) -> Result<(), OmtError> {
        let addr = SocketAddr::from(([0, 0, 0, 0], self.port));
        let listener = into_listener(listen(addr)?)?;
        self.run_with_listener(listener)
    }

    /// Serve using an already-bound listener.
    pub fn run_with_listener(&mut self, listener: TcpListener) -> Result<(), OmtError> {
        listener.set_nonblocking(true)?;
        let shared = Arc::new(Mutex::new(Shared {
            entries: Vec::new(),
            peers: HashMap::new(),
        }));
        self.stop.store(false, Ordering::SeqCst);

        while !self.stop.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, peer)) => {
                    let _ = configure_stream(&stream);
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
                    let _ = stream.set_nonblocking(false);
                    {
                        let mut g = shared.lock().map_err(|_| {
                            OmtError::Discovery("discovery server lock poisoned".into())
                        })?;
                        // Snapshot current entries for the new peer.
                        let snapshot: Vec<OmtAddress> =
                            g.entries.iter().map(|e| e.address.clone()).collect();
                        g.peers.insert(peer, stream.try_clone()?);
                        drop(g);
                        for addr in snapshot {
                            let _ = send_address_to(&stream, &addr);
                        }
                    }
                    let shared_c = Arc::clone(&shared);
                    let stop_c = Arc::clone(&self.stop);
                    self.threads.push(thread::spawn(move || {
                        peer_loop(stream, peer, shared_c, stop_c);
                    }));
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(20));
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
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

fn peer_loop(
    mut stream: TcpStream,
    peer: SocketAddr,
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
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
                    let _ = handle_address(&shared, peer, &mut addr);
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
    // Disconnect: remove all entries owned by this peer and notify others.
    if let Ok(mut g) = shared.lock() {
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
        }
    }
}

fn handle_address(
    shared: &Arc<Mutex<Shared>>,
    peer: SocketAddr,
    addr: &mut OmtAddress,
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
    Ok(None)
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

    #[test]
    fn discovery_server_relays_register() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let mut server = DiscoveryServer {
            port,
            stop: Arc::clone(&stop),
            threads: Vec::new(),
        };
        let handle = thread::spawn(move || server.run_with_listener(listener));

        thread::sleep(Duration::from_millis(100));

        let mut client = DiscoveryClient::new("127.0.0.1");
        client.port = port;
        client.connect().unwrap();

        let mut addr = OmtAddress::from_full_name("TESTHOST (Cam1)", 6400);
        addr.addresses = vec!["10.0.0.1".into()];
        client.register(&addr).unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut found = false;
        while std::time::Instant::now() < deadline {
            let sources = client.sources();
            if sources
                .iter()
                .any(|s| s.instance_name() == "TESTHOST (Cam1)" && s.port == 6400)
            {
                found = true;
                // Server replaces client IPs with peer address (127.0.0.1 here).
                assert!(sources.iter().any(|s| {
                    s.addresses
                        .iter()
                        .any(|a| a == "127.0.0.1" || a == "::1" || a.starts_with("127."))
                }));
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        stop.store(true, Ordering::SeqCst);
        let _ = handle.join();
        assert!(found, "expected relayed OMTAddress from discovery server");
    }
}
