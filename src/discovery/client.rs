//! OMT Discovery Client — infrastructure (connects to Discovery Server).
//!
//! Matches libomtnet `OMTDiscoveryClient`: register/deregister via `<OMTAddress>`
//! metadata frames and collect relayed sources.

use std::io::Write;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::discovery::address::OmtAddress;
use crate::discovery::server::{address_frame_bytes, try_recv_metadata};
use crate::error::OmtError;
use crate::transport::channel::Channel;
use crate::transport::socket::configure_stream;
use crate::types::{DISCOVERY_SERVER_DEFAULT_PORT, FrameType, URL_PREFIX};

/// Client that registers sources with a discovery server.
#[derive(Debug)]
pub struct DiscoveryClient {
    /// Server host.
    pub host: String,
    /// Server port.
    pub port: u16,
    stream: Option<TcpStream>,
    sources: Arc<Mutex<Vec<OmtAddress>>>,
    worker: Option<JoinHandle<()>>,
    stop: Arc<Mutex<bool>>,
}

impl DiscoveryClient {
    /// Connect settings for the default discovery port.
    pub fn new(host: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            port: DISCOVERY_SERVER_DEFAULT_PORT,
            stream: None,
            sources: Arc::new(Mutex::new(Vec::new())),
            worker: None,
            stop: Arc::new(Mutex::new(false)),
        }
    }

    /// Parse `omt://host:port` (or bare host) and connect.
    pub fn connect_url(url: &str) -> Result<Self, OmtError> {
        let (host, port) = parse_server_url(url)?;
        let mut client = Self {
            host,
            port,
            stream: None,
            sources: Arc::new(Mutex::new(Vec::new())),
            worker: None,
            stop: Arc::new(Mutex::new(false)),
        };
        client.connect()?;
        Ok(client)
    }

    /// Connect to the discovery server and start the receive loop.
    pub fn connect(&mut self) -> Result<(), OmtError> {
        let addr = resolve_one(&self.host, self.port)?;
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))?;
        configure_stream(&stream)?;
        let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
        let _ = stream.set_nonblocking(false);
        let recv_stream = stream.try_clone()?;
        self.stream = Some(stream);

        *self.stop.lock().unwrap() = false;
        let sources = Arc::clone(&self.sources);
        let stop = Arc::clone(&self.stop);
        self.worker = Some(thread::spawn(move || {
            let mut stream = recv_stream;
            let mut channel = Channel::new(FrameType::METADATA);
            while !*stop.lock().unwrap() {
                match try_recv_metadata(&mut stream, &mut channel, Duration::from_millis(200)) {
                    Ok(Some(xml)) => {
                        if let Ok(addr) = OmtAddress::from_xml(&xml)
                            && let Ok(mut g) = sources.lock()
                        {
                            if addr.removed {
                                g.retain(|a| {
                                    !(a.instance_name() == addr.instance_name()
                                        && a.port == addr.port)
                                });
                            } else {
                                g.retain(|a| {
                                    !(a.instance_name() == addr.instance_name()
                                        && a.port == addr.port)
                                });
                                g.push(addr);
                                g.sort_by_key(|a| a.instance_name());
                            }
                        }
                    }
                    Ok(None) => break,
                    Err(_) => thread::sleep(Duration::from_millis(50)),
                }
            }
        }));
        Ok(())
    }

    /// Register a source with the server.
    pub fn register(&mut self, address: &OmtAddress) -> Result<(), OmtError> {
        self.send_address(address)
    }

    /// Deregister a source with the server.
    pub fn deregister(&mut self, address: &OmtAddress) -> Result<(), OmtError> {
        let mut a = address.clone();
        a.removed = true;
        self.send_address(&a)
    }

    /// Sources learned from the server so far.
    pub fn sources(&self) -> Vec<OmtAddress> {
        self.sources.lock().map(|g| g.clone()).unwrap_or_default()
    }

    fn send_address(&mut self, address: &OmtAddress) -> Result<(), OmtError> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| OmtError::Discovery("discovery client not connected".into()))?;
        stream.write_all(&address_frame_bytes(address))?;
        stream.flush()?;
        Ok(())
    }
}

impl Drop for DiscoveryClient {
    fn drop(&mut self) {
        if let Ok(mut g) = self.stop.lock() {
            *g = true;
        }
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

fn parse_server_url(url: &str) -> Result<(String, u16), OmtError> {
    let rest = if let Some(r) = url.strip_prefix(URL_PREFIX) {
        r
    } else {
        url
    };
    let rest = rest.split('/').next().unwrap_or(rest);
    if let Some((host, port)) = rest.rsplit_once(':') {
        let port: u16 = port.parse().map_err(|_| {
            OmtError::InvalidArgument(format!("invalid discovery server port: {port}"))
        })?;
        Ok((host.to_string(), port))
    } else {
        Ok((rest.to_string(), DISCOVERY_SERVER_DEFAULT_PORT))
    }
}

fn resolve_one(host: &str, port: u16) -> Result<SocketAddr, OmtError> {
    (host, port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| OmtError::Discovery(format!("could not resolve {host}:{port}")))
}
