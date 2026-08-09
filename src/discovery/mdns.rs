//! Infrastructure adapter: DNS-SD via [`mdns_sd`].
//!
//! Not part of the public API — [`super::Discovery`] owns the use cases.
//! Maps mdns-sd events into OMT instance names (`HOSTNAME (Source)`).

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

use crate::error::OmtError;

/// mdns-sd requires a trailing dot on the type domain.
const SERVICE_TYPE: &str = "_omt._tcp.local.";
const BROWSE_WAIT: Duration = Duration::from_millis(1500);

#[derive(Clone, Debug)]
struct Discovered {
    /// DNS-SD fullname or instance (`…._omt._tcp.local.`).
    fullname: String,
    port: u16,
    addresses: Vec<String>,
}

struct MdnsState {
    daemon: ServiceDaemon,
    discovered: HashMap<String, Discovered>,
    /// instance display name → registered fullname (for unregister)
    registered: HashMap<String, String>,
}

static STATE: OnceLock<Arc<Mutex<MdnsState>>> = OnceLock::new();

fn state() -> Result<Arc<Mutex<MdnsState>>, OmtError> {
    if let Some(s) = STATE.get() {
        return Ok(Arc::clone(s));
    }
    let daemon =
        ServiceDaemon::new().map_err(|e| OmtError::Discovery(format!("mdns-sd daemon: {e}")))?;
    let receiver = daemon
        .browse(SERVICE_TYPE)
        .map_err(|e| OmtError::Discovery(format!("mdns-sd browse: {e}")))?;

    let state = Arc::new(Mutex::new(MdnsState {
        daemon,
        discovered: HashMap::new(),
        registered: HashMap::new(),
    }));
    let state_c = Arc::clone(&state);
    thread::Builder::new()
        .name("omt-mdns".into())
        .spawn(move || {
            while let Ok(event) = receiver.recv() {
                match event {
                    ServiceEvent::ServiceResolved(info) => {
                        if let Some(entry) = resolved_entry(&info)
                            && let Ok(mut g) = state_c.lock()
                        {
                            g.discovered.insert(entry.fullname.clone(), entry);
                        }
                    }
                    ServiceEvent::ServiceRemoved(_, fullname) => {
                        if let Ok(mut g) = state_c.lock() {
                            g.discovered.remove(&fullname);
                        }
                    }
                    _ => {}
                }
            }
        })
        .map_err(|e| OmtError::Discovery(format!("mdns worker: {e}")))?;

    let _ = STATE.set(Arc::clone(&state));
    Ok(state)
}

/// Advertise an `_omt._tcp` service (`instance` is `MACHINE (Name)`).
pub(super) fn advertise(instance_name: &str, port: u16) -> Result<(), OmtError> {
    let st = state()?;
    let host_label = host_label_from_instance(instance_name);
    let host_name = format!("{host_label}.local.");
    let info = ServiceInfo::new(SERVICE_TYPE, instance_name, &host_name, "", port, None)
        .map_err(|e| OmtError::Discovery(format!("mdns-sd ServiceInfo: {e}")))?
        .enable_addr_auto();
    let fullname = info.get_fullname().to_string();

    let mut g = st
        .lock()
        .map_err(|_| OmtError::Discovery("mdns lock poisoned".into()))?;
    g.daemon
        .register(info)
        .map_err(|e| OmtError::Discovery(format!("mdns-sd register: {e}")))?;
    g.registered.insert(instance_name.to_string(), fullname);
    Ok(())
}

/// Withdraw an advertisement.
pub(super) fn withdraw(instance_name: &str) -> Result<(), OmtError> {
    let st = state()?;
    let mut g = st
        .lock()
        .map_err(|_| OmtError::Discovery("mdns lock poisoned".into()))?;
    if let Some(fullname) = g.registered.remove(instance_name) {
        let _ = g.daemon.unregister(&fullname);
    }
    Ok(())
}

/// Browse for `_omt._tcp` services (waits briefly for responses).
#[allow(dead_code)]
pub(super) fn browse() -> Result<Vec<(String, u16, Vec<String>)>, OmtError> {
    browse_for(BROWSE_WAIT)
}

/// Browse with an explicit wait window.
///
/// Returns `(fullname_or_instance, port, addresses)` for the application layer
/// to map into [`crate::discovery::OmtAddress`].
pub(super) fn browse_for(wait: Duration) -> Result<Vec<(String, u16, Vec<String>)>, OmtError> {
    let st = state()?;
    let deadline = Instant::now() + wait;
    while Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
    }
    let g = st
        .lock()
        .map_err(|_| OmtError::Discovery("mdns lock poisoned".into()))?;
    let mut out: Vec<(String, u16, Vec<String>)> = g
        .discovered
        .values()
        .filter(|d| d.port != 0)
        .map(|d| (d.fullname.clone(), d.port, d.addresses.clone()))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn resolved_entry(info: &mdns_sd::ResolvedService) -> Option<Discovered> {
    if !info.is_valid() {
        return None;
    }
    let mut addresses: Vec<String> = info
        .addresses
        .iter()
        .map(|a| match a.to_ip_addr() {
            IpAddr::V4(v) => v.to_string(),
            IpAddr::V6(v) => v.to_string(),
        })
        .collect();
    addresses.sort_by_key(|a| ip_preference(a));
    Some(Discovered {
        fullname: info.fullname.clone(),
        port: info.port,
        addresses,
    })
}

fn ip_preference(a: &str) -> u8 {
    if a == "127.0.0.1" || a == "::1" {
        5
    } else if a.starts_with("fe80:") || a.starts_with("169.254.") {
        4
    } else if a.contains(':') {
        3
    } else if a.starts_with("172.") {
        1
    } else {
        0
    }
}

fn host_label_from_instance(instance: &str) -> String {
    let raw = instance.split('(').next().unwrap_or("host").trim();
    let mut out = String::new();
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() || c == '-' {
            out.push(c);
        } else if c == ' ' || c == '_' {
            out.push('-');
        }
    }
    if out.is_empty() { "host".into() } else { out }
}
