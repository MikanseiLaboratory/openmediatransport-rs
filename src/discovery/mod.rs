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

pub use address::OmtAddress;
pub use client::DiscoveryClient;
pub use server::DiscoveryServer;

/// High-level discovery API (sync), matching libomtnet `OMTDiscovery` browse path.
///
/// DNS-SD details are handled by the private `mdns` adapter (`mdns-sd` crate).
#[derive(Debug, Default)]
pub struct Discovery {
    sources: Vec<OmtAddress>,
    registered: Vec<(String, u16)>,
}

impl Discovery {
    /// Create a discovery instance.
    pub fn new() -> Result<Self, OmtError> {
        Ok(Self::default())
    }

    /// Refresh discovered sources via mDNS browse (waits ~1.5s for answers).
    pub fn refresh(&mut self) -> Result<(), OmtError> {
        self.refresh_for(Duration::from_millis(1500))
    }

    /// Refresh with an explicit wait for mDNS responses.
    pub fn refresh_for(&mut self, wait: Duration) -> Result<(), OmtError> {
        let found = mdns::browse_for(wait)?;
        self.sources = found
            .into_iter()
            .filter_map(|(fullname, port, addrs)| OmtAddress::from_dns_sd(fullname, port, addrs))
            .collect();
        self.sources.sort_by_key(|a| a.instance_name());
        Ok(())
    }

    /// List currently known sources.
    pub fn sources(&self) -> &[OmtAddress] {
        &self.sources
    }

    /// Register a local source name for advertisement.
    pub fn register(&mut self, name: &str, port: u16) -> Result<(), OmtError> {
        let addr = OmtAddress::local(name, port).unwrap_or_else(|_| OmtAddress::new(name, port));
        mdns::advertise(&addr.instance_name(), port)?;
        self.registered.push((addr.instance_name(), port));
        Ok(())
    }

    /// Deregister a local source.
    pub fn deregister(&mut self, name: &str) -> Result<(), OmtError> {
        self.registered.retain(|(n, _)| {
            if n.contains(name) {
                let _ = mdns::withdraw(n);
                false
            } else {
                true
            }
        });
        Ok(())
    }
}
