//! Source discovery.
//!
//! Layering (Clean Architecture, kept light for an OMT-only stack):
//!
//! - **Domain** — [`address::OmtAddress`] (`HOSTNAME (Source)`, URL/XML)
//! - **Application** — [`Discovery`] (browse / register / list)
//! - **Infrastructure** — `mdns` (mdns-sd), [`server`] / [`client`] (port 6399 relay)
//!
//! Callers use [`Discovery`] / [`OmtAddress`]; mdns-sd types stay internal.

pub mod address;
pub mod client;
pub mod server;

mod mdns;

use std::time::Duration;

use crate::error::OmtError;
use crate::settings;

pub use address::OmtAddress;
pub use client::DiscoveryClient;
pub use server::DiscoveryServer;

/// High-level discovery API (sync), matching libomtnet `OMTDiscovery` browse path.
///
/// DNS-SD details are handled by the private `mdns` adapter (`mdns-sd` crate).
/// When `DiscoveryServer` is set in [`crate::Settings`], senders register with that
/// server instead of mDNS; browsing still merges DNS-SD and server sources
/// (libomtnet receive path).
#[derive(Debug, Default)]
pub struct Discovery {
    sources: Vec<OmtAddress>,
    registered: Vec<OmtAddress>,
    server_client: Option<DiscoveryClient>,
}

impl Discovery {
    /// Create a discovery instance.
    ///
    /// If [`crate::KEY_DISCOVERY_SERVER`] is set in the process settings, connects
    /// to that discovery server (failures are logged and DNS-SD is used instead).
    pub fn new() -> Result<Self, OmtError> {
        crate::logging::init_logging();
        let mut this = Self::default();
        if let Some(url) = settings::global_discovery_server() {
            match DiscoveryClient::connect_url(&url) {
                Ok(client) => {
                    tracing::info!("OMTDiscovery: using DiscoveryServer {url}");
                    this.server_client = Some(client);
                }
                Err(e) => {
                    tracing::warn!(
                        "OMTDiscovery: DiscoveryServer {url} unavailable ({e}); falling back to DNS-SD"
                    );
                }
            }
        }
        Ok(this)
    }

    /// True when this instance is connected to an OMT discovery server.
    pub fn using_server(&self) -> bool {
        self.server_client.is_some()
    }

    /// Refresh discovered sources via mDNS browse (waits ~1.5s for answers).
    pub fn refresh(&mut self) -> Result<(), OmtError> {
        self.refresh_for(Duration::from_millis(1500))
    }

    /// Refresh with an explicit wait for mDNS responses.
    ///
    /// Sources from a configured discovery server are merged with DNS-SD results.
    pub fn refresh_for(&mut self, wait: Duration) -> Result<(), OmtError> {
        let found = mdns::browse_for(wait)?;
        let mut sources: Vec<OmtAddress> = found
            .into_iter()
            .filter_map(|(fullname, port, addrs)| OmtAddress::from_dns_sd(fullname, port, addrs))
            .collect();
        if let Some(client) = &self.server_client {
            for addr in client.sources() {
                if let Some(existing) = sources
                    .iter_mut()
                    .find(|a| a.instance_name() == addr.instance_name())
                {
                    for ip in addr.addresses {
                        if !existing.addresses.contains(&ip) {
                            existing.addresses.push(ip);
                        }
                    }
                } else {
                    sources.push(addr);
                }
            }
        }
        sources.sort_by_key(|a| a.instance_name());
        self.sources = sources;
        Ok(())
    }

    /// List currently known sources.
    pub fn sources(&self) -> &[OmtAddress] {
        &self.sources
    }

    /// Register a local source name for advertisement.
    pub fn register(&mut self, name: &str, port: u16) -> Result<(), OmtError> {
        let addr = OmtAddress::local(name, port).unwrap_or_else(|_| OmtAddress::new(name, port));
        if let Some(client) = &mut self.server_client {
            client.register(&addr)?;
        } else {
            mdns::advertise(&addr.instance_name(), port)?;
        }
        self.registered.push(addr);
        Ok(())
    }

    /// Deregister a local source.
    pub fn deregister(&mut self, name: &str) -> Result<(), OmtError> {
        let mut remaining = Vec::new();
        for addr in self.registered.drain(..) {
            if addr.instance_name().contains(name) {
                if let Some(client) = &mut self.server_client {
                    let _ = client.deregister(&addr);
                } else {
                    let _ = mdns::withdraw(&addr.instance_name());
                }
            } else {
                remaining.push(addr);
            }
        }
        self.registered = remaining;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_does_not_use_discovery_server() {
        let d = Discovery::default();
        assert!(!d.using_server());
        assert!(d.sources().is_empty());
    }
}
