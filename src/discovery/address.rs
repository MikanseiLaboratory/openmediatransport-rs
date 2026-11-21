//! OMT address / URL helpers.

use std::env;
use std::net::IpAddr;

use crate::error::OmtError;
use crate::types::{MAX_INSTANCE_NAME_LENGTH, URL_PREFIX};

/// Discovered or configured OMT source address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OmtAddress {
    /// Display / service name, e.g. `HOSTNAME (Source Name)`.
    pub name: String,
    /// Machine name (when known separately).
    pub machine_name: String,
    /// TCP port.
    pub port: u16,
    /// Candidate IP addresses.
    pub addresses: Vec<String>,
    /// Whether this entry was removed.
    pub removed: bool,
}

impl OmtAddress {
    /// Create a new address entry.
    pub fn new(name: impl Into<String>, port: u16) -> Self {
        let mut addr = Self {
            name: sanitize_name(name.into()),
            machine_name: String::new(),
            port,
            addresses: Vec::new(),
            removed: false,
        };
        addr.limit_name_length();
        addr
    }

    /// Create from a full DNS-SD instance name `MACHINE (Source)`.
    pub fn from_full_name(full_name: &str, port: u16) -> Self {
        let full = sanitize_name(full_name.to_string());
        if let Some((machine, name)) = split_full_name(&full) {
            let mut addr = Self {
                name,
                machine_name: machine,
                port,
                addresses: Vec::new(),
                removed: false,
            };
            addr.limit_name_length();
            addr
        } else {
            Self::new(full, port)
        }
    }

    /// True when the display name looks like a valid OMT source (`MACHINE (Name)`).
    pub fn is_valid_name(full_name: &str) -> bool {
        full_name.contains('(') && full_name.contains(')')
    }

    /// Build from a DNS-SD fullname / instance, port, and resolved addresses.
    ///
    /// Returns `None` when the name is not a valid OMT instance (`MACHINE (Name)`).
    pub fn from_dns_sd(
        fullname_or_instance: impl AsRef<str>,
        port: u16,
        addresses: Vec<String>,
    ) -> Option<Self> {
        let instance = strip_omt_service_suffix(fullname_or_instance.as_ref());
        if !Self::is_valid_name(&instance) || port == 0 {
            return None;
        }
        let mut addr = Self::from_full_name(&instance, port);
        addr.addresses = addresses;
        Some(addr)
    }

    /// Build a local address `HOSTNAME (name)` with truncation to 63 chars.
    pub fn local(source_name: &str, port: u16) -> Result<Self, OmtError> {
        let machine = hostname();
        let mut addrs = local_ips();
        addrs.sort_by_key(|a| a.contains(':'));
        let mut addr = Self {
            name: sanitize_name(source_name.to_string()),
            machine_name: sanitize_name(machine),
            port,
            addresses: addrs,
            removed: false,
        };
        addr.limit_name_length();
        Ok(addr)
    }

    /// DNS-SD instance name: `MACHINE (Name)`, capped at 63 characters.
    pub fn instance_name(&self) -> String {
        if self.machine_name.is_empty() {
            self.name.clone()
        } else {
            format!("{} ({})", self.machine_name, self.name)
        }
    }

    fn limit_name_length(&mut self) {
        // DNS-SD instance labels are capped at 63 bytes. Keep the source name
        // first, then give the machine name the remaining space: gethostname()
        // can return long cloud/CI host names that do not fit on their own, and
        // an over-long label makes the sender undiscoverable at all.
        if self.machine_name.is_empty() {
            self.name = truncate_bytes(&self.name, MAX_INSTANCE_NAME_LENGTH)
                .trim()
                .to_string();
            return;
        }
        const SEPARATOR: usize = 3; // " (" + ")"
        let name_max = MAX_INSTANCE_NAME_LENGTH.saturating_sub(SEPARATOR + 1);
        if self.name.len() > name_max {
            self.name = truncate_bytes(&self.name, name_max).trim().to_string();
        }
        let machine_max = MAX_INSTANCE_NAME_LENGTH
            .saturating_sub(SEPARATOR + self.name.len())
            .max(1);
        if self.machine_name.len() > machine_max {
            self.machine_name = truncate_bytes(&self.machine_name, machine_max)
                .trim()
                .to_string();
        }
    }

    /// Parse `omt://host[:port][/name]`.
    pub fn from_url(url: &str) -> Result<Self, OmtError> {
        if !url.starts_with(URL_PREFIX) {
            return Err(OmtError::InvalidArgument(format!(
                "URL must start with {URL_PREFIX}"
            )));
        }
        let rest = &url[URL_PREFIX.len()..];
        let (host_port, name) = match rest.split_once('/') {
            Some((h, n)) => (h, n),
            None => (rest, ""),
        };
        let (host, port) = match host_port.rsplit_once(':') {
            Some((h, p)) if !h.is_empty() => {
                let port: u16 = p
                    .parse()
                    .map_err(|_| OmtError::InvalidArgument(format!("invalid port in URL: {p}")))?;
                (h, port)
            }
            _ => (host_port, 0),
        };
        let name = if name.is_empty() {
            host.to_string()
        } else {
            name.to_string()
        };
        let mut addr = Self {
            name: sanitize_name(name),
            machine_name: String::new(),
            port,
            addresses: vec![host.to_string()],
            removed: false,
        };
        addr.limit_name_length();
        Ok(addr)
    }

    /// Format as `omt://host[:port][/name]`.
    ///
    /// Prefers [`Self::machine_name`] (libomtnet `ToURL` behavior) so the URL
    /// identifies the advertised host, not a single discovery-time IP that may
    /// be stale or wrong. Callers that need a TCP endpoint should use
    /// [`Self::addresses`] (or resolve the host) at connect time.
    pub fn to_url(&self) -> String {
        let host = if !self.machine_name.is_empty() {
            self.machine_name.as_str()
        } else {
            self.addresses
                .first()
                .map(String::as_str)
                .unwrap_or("127.0.0.1")
        };
        if self.port == 0 {
            format!("{URL_PREFIX}{host}/{}", self.name)
        } else {
            format!("{URL_PREFIX}{host}:{}/{}", self.port, self.name)
        }
    }

    /// Serialize address XML for discovery server (libomtnet `OMTAddress.ToXML`).
    pub fn to_xml(&self) -> String {
        let mut xml = format!(
            "<OMTAddress>\n  <Name>{}</Name>\n  <Port>{}</Port>\n",
            escape_xml(&self.instance_name()),
            self.port
        );
        if self.removed {
            xml.push_str("  <Removed>True</Removed>\n");
        }
        xml.push_str("  <Addresses>\n");
        if self.addresses.is_empty() {
            xml.push_str("    <IPAddress>0.0.0.0</IPAddress>\n");
        } else {
            for a in &self.addresses {
                xml.push_str(&format!("    <IPAddress>{}</IPAddress>\n", escape_xml(a)));
            }
        }
        xml.push_str("  </Addresses>\n</OMTAddress>");
        xml
    }

    /// Serialize register XML for discovery server.
    pub fn to_register_xml(&self) -> String {
        let mut a = self.clone();
        a.removed = false;
        a.to_xml()
    }

    /// Serialize deregister XML.
    pub fn to_deregister_xml(&self) -> String {
        let mut a = self.clone();
        a.removed = true;
        a.addresses.clear();
        a.to_xml()
    }

    /// Parse discovery-server address XML (`OMTAddress.FromXML`).
    pub fn from_xml(xml: &str) -> Result<Self, OmtError> {
        for item in crate::protocol::metadata::parse_metadata(xml) {
            if let crate::protocol::metadata::Metadata::Address {
                name,
                port,
                removed,
                addresses,
            } = item
            {
                if name.is_empty() {
                    return Err(OmtError::InvalidArgument(
                        "OMTAddress XML missing Name".into(),
                    ));
                }
                let mut addr = Self::from_full_name(&name, port);
                addr.removed = removed;
                addr.addresses = addresses;
                return Ok(addr);
            }
        }
        Err(OmtError::InvalidArgument(
            "OMTAddress XML missing Name".into(),
        ))
    }
}

impl std::fmt::Display for OmtAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.instance_name())
    }
}

fn sanitize_name(s: String) -> String {
    s.replace(['\0', '\n', '\r'], "").trim().to_string()
}

/// Longest prefix of `s` that fits in `max` bytes and ends on a char boundary.
fn truncate_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// `HOST (Cam)._omt._tcp.local.` → `HOST (Cam)`.
fn strip_omt_service_suffix(fqdn: &str) -> String {
    let trimmed = fqdn.trim_end_matches('.');
    let lower = trimmed.to_ascii_lowercase();
    if let Some(pos) = lower.find("._omt.") {
        trimmed[..pos].to_string()
    } else {
        trimmed.to_string()
    }
}

fn split_full_name(full: &str) -> Option<(String, String)> {
    let open = full.find('(')?;
    let close = full.rfind(')')?;
    if open == 0 || close <= open {
        return None;
    }
    let machine = full[..open].trim().to_string();
    let name = full[open + 1..close].trim().to_string();
    if machine.is_empty() || name.is_empty() {
        return None;
    }
    Some((machine, name))
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Upper-case machine name used in `MACHINE (Name)` instance names.
///
/// libomtnet reads `Environment.MachineName`, which is `COMPUTERNAME` on
/// Windows and `gethostname(2)` elsewhere. The `HOSTNAME` environment
/// variable is a shell convenience only: GUI-launched processes on macOS
/// (Finder/launchd) do not inherit it, so relying on it advertised
/// `LOCALHOST (Name)` (issue #52).
fn hostname() -> String {
    normalize_hostname(
        os_hostname()
            .or_else(|| env::var("HOSTNAME").ok())
            .or_else(|| env::var("COMPUTERNAME").ok()),
    )
}

/// Normalize a machine label, replacing names that do not identify a host.
///
/// `localhost` (and `*.localhost`) and bare IP literals are not identities:
/// they resolve to this machine, or to the receiving machine, rather than to
/// the sender. A receiver that resolves the label (URL-only connect) would
/// reach its own loopback, and every affected machine shares one DNS-SD
/// identity. Such names are replaced with `OMT-HOST-<primary IPv4>` (or
/// `OMT-HOST`).
fn normalize_hostname(raw: Option<String>) -> String {
    let normalized = raw
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_default()
        .to_ascii_uppercase();
    if normalized.is_empty() || is_reserved_machine_name(&normalized) {
        return fallback_machine_name();
    }
    normalized
}

fn is_reserved_machine_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("LOCALHOST")
        || name.to_ascii_lowercase().ends_with(".localhost")
        || name.parse::<IpAddr>().is_ok()
}

fn fallback_machine_name() -> String {
    local_ips()
        .iter()
        .find_map(|address| {
            address
                .parse::<std::net::Ipv4Addr>()
                .ok()
                .filter(|ip| !ip.is_loopback())
        })
        .map(|ip| format!("OMT-HOST-{}", ip.to_string().replace('.', "-")))
        .unwrap_or_else(|| "OMT-HOST".to_string())
}

#[cfg(windows)]
fn os_hostname() -> Option<String> {
    // Matches `Environment.MachineName` on Windows.
    env::var("COMPUTERNAME").ok()
}

#[cfg(unix)]
fn os_hostname() -> Option<String> {
    // Matches `Environment.MachineName` on Unix (`gethostname`).
    let mut buf = [0u8; 256];
    // SAFETY: `buf` is valid for `buf.len()` writes and `gethostname` writes at
    // most that many bytes, NUL-terminating the name on success.
    if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } != 0 {
        return None;
    }
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    Some(String::from_utf8_lossy(&buf[..len]).into_owned())
}

#[cfg(not(any(unix, windows)))]
fn os_hostname() -> Option<String> {
    None
}

/// Addresses this machine advertises to peers.
///
/// Every operational interface is listed (IPv4 first, then global/ULA IPv6),
/// followed by `127.0.0.1` to match libomtnet's `OMTSend`, which adds
/// `IPAddress.Loopback` to every source. Point-to-point interfaces (VPN
/// tunnels, Tailscale) are kept, but after regular interfaces so the first
/// connect does not wait on a tunnel the peer cannot reach.
///
/// `mdns-sd` announces interface addresses itself, and a compliant discovery
/// server replaces this list with the TCP peer address, so this mainly
/// matters to callers that consume [`OmtAddress`] directly. It also stops
/// multi-homed hosts from advertising only their default-route address.
///
/// Receivers use the loopback entry only when it is the sole candidate for a
/// source on this machine; see the receiver's endpoint policy.
fn local_ips() -> Vec<String> {
    let primary = default_route_ip();
    let mut v4: Vec<(bool, std::net::Ipv4Addr)> = Vec::new();
    let mut v6: Vec<(bool, std::net::Ipv6Addr)> = Vec::new();
    if let Ok(interfaces) = if_addrs::get_if_addrs() {
        for interface in interfaces {
            if interface.is_loopback() || !interface.is_oper_up() {
                continue;
            }
            let p2p = interface.is_p2p();
            match interface.ip() {
                IpAddr::V4(ip) if !ip.is_unspecified() => v4.push((p2p, ip)),
                // Link-local IPv6 needs a zone id that OMTAddress cannot
                // carry (the XML stores a bare address string).
                IpAddr::V6(ip) if !ip.is_unicast_link_local() && !ip.is_unspecified() => {
                    v6.push((p2p, ip));
                }
                IpAddr::V4(_) | IpAddr::V6(_) => {}
            }
        }
    }
    // `false < true`, so regular interfaces sort before point-to-point ones.
    v4.sort_unstable_by_key(|(p2p, ip)| (*p2p, *ip));
    v4.dedup();
    v6.sort_unstable_by_key(|(p2p, ip)| (*p2p, *ip));
    v6.dedup();
    // Try the default-route address first when it is a regular interface;
    // a tunnel keeps its place at the end.
    if let Some(IpAddr::V4(primary)) = primary
        && let Some(pos) = v4.iter().position(|(p2p, ip)| !*p2p && *ip == primary)
    {
        let primary = v4.remove(pos);
        v4.insert(0, primary);
    }
    let mut addresses: Vec<String> = v4.into_iter().map(|(_, ip)| ip.to_string()).collect();
    addresses.extend(v6.into_iter().map(|(_, ip)| ip.to_string()));
    // libomtnet's `OMTSend` adds `IPAddress.Loopback` to every source. Keep
    // that for parity; `OmtAddress::local` sorts it last among IPv4, and the
    // receiver drops it whenever a routable endpoint exists.
    addresses.push("127.0.0.1".to_string());
    addresses
}

/// The address of the interface carrying the default route, if any.
fn default_route_ip() -> Option<IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_unspecified()).then_some(ip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_dns_sd_strips_service_suffix() {
        let a = OmtAddress::from_dns_sd(
            "DESKTOP-BUIITN0 (vMix - Output 1)._omt._tcp.local.",
            6400,
            vec!["192.168.3.3".into()],
        )
        .unwrap();
        assert_eq!(a.instance_name(), "DESKTOP-BUIITN0 (vMix - Output 1)");
        assert_eq!(a.port, 6400);
        assert_eq!(a.addresses, vec!["192.168.3.3"]);
    }

    #[test]
    fn from_dns_sd_rejects_invalid_names() {
        assert!(OmtAddress::from_dns_sd("not-omt._omt._tcp.local.", 6400, vec![]).is_none());
    }

    #[test]
    fn to_url_prefers_machine_name_over_ip() {
        let a = OmtAddress::from_dns_sd(
            "CAMHOST (Output 1)._omt._tcp.local.",
            6400,
            vec!["10.0.0.5".into(), "192.168.1.5".into()],
        )
        .unwrap();
        assert_eq!(a.to_url(), "omt://CAMHOST:6400/Output 1");
    }

    #[test]
    fn from_xml_reads_child_elements() {
        let xml = r#"<OMTAddress>
  <Name>HOST (Cam)</Name>
  <Port>6400</Port>
  <Addresses>
    <IPAddress>10.0.0.1</IPAddress>
  </Addresses>
</OMTAddress>"#;
        let a = OmtAddress::from_xml(xml).unwrap();
        assert_eq!(a.instance_name(), "HOST (Cam)");
        assert_eq!(a.port, 6400);
        assert_eq!(a.addresses, vec!["10.0.0.1"]);
    }

    #[test]
    fn advertised_addresses_include_loopback() {
        // libomtnet's `OMTSend` adds `IPAddress.Loopback` to every source.
        let a = OmtAddress::local("Cam", 6400).unwrap();
        assert!(
            a.addresses.iter().any(|ip| ip == "127.0.0.1"),
            "loopback must be advertised for parity: {:?}",
            a.addresses
        );
    }

    #[test]
    fn hostname_is_normalized() {
        assert_eq!(
            normalize_hostname(Some(" MacBook-Air.local ".into())),
            "MACBOOK-AIR.LOCAL"
        );
        for reserved in [
            Some("   ".into()),
            None,
            Some("localhost".into()),
            Some("127.0.0.1".into()),
            Some("::1".into()),
        ] {
            let name = normalize_hostname(reserved);
            assert!(name.starts_with("OMT-HOST"), "{name}");
            assert!(!is_reserved_machine_name(&name), "{name}");
        }
    }

    #[test]
    fn os_hostname_resolves_on_supported_platforms() {
        // Issue #52: macOS must report the real machine name instead of
        // relying on `HOSTNAME`, which GUI-launched processes do not get.
        #[cfg(any(unix, windows))]
        {
            let name = os_hostname().expect("platform machine name");
            assert!(!name.trim().is_empty());
        }
    }

    #[test]
    fn local_address_uses_machine_name() {
        let a = OmtAddress::local("Camera", 6400).unwrap();
        assert!(!a.machine_name.is_empty());
        assert!(a.instance_name().ends_with("(Camera)"));
    }

    #[test]
    fn long_machine_name_still_fits_dns_sd_label() {
        // GitHub's macOS runners report host names like this; the machine part
        // alone is longer than a DNS-SD instance label may be.
        let mut a = OmtAddress::new("Camera", 6400);
        a.machine_name =
            "SAT12-BQ152-62568726-D57A-443C-8E0C-892490A3068B-3E13BAA0E36F.LOCAL".into();
        a.limit_name_length();
        let instance = a.instance_name();
        assert!(instance.len() <= MAX_INSTANCE_NAME_LENGTH, "{instance}");
        assert!(instance.ends_with(" (Camera)"), "{instance}");
        assert!(!a.machine_name.is_empty());
    }

    #[test]
    fn long_source_name_is_truncated_on_char_boundaries() {
        let mut a = OmtAddress::new("カメラ".repeat(10), 6400);
        a.machine_name = "HOST".into();
        a.limit_name_length();
        assert!(a.instance_name().len() <= MAX_INSTANCE_NAME_LENGTH);
        assert!(!a.name.is_empty());
        assert!(!a.name.contains('\u{FFFD}'));
    }

    #[test]
    fn long_name_without_machine_fits_label() {
        let a = OmtAddress::new("x".repeat(80), 6400);
        assert!(a.instance_name().len() <= MAX_INSTANCE_NAME_LENGTH);
    }
}
