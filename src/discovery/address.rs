//! OMT address / URL helpers.

use std::env;

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
        let full = self.instance_name();
        if full.len() <= MAX_INSTANCE_NAME_LENGTH {
            return;
        }
        let oversize = full.len() - MAX_INSTANCE_NAME_LENGTH;
        if oversize < self.name.len() {
            let keep = self.name.len() - oversize;
            self.name = self.name[..keep].trim().to_string();
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
        let name = xml_text(xml, "Name")
            .ok_or_else(|| OmtError::InvalidArgument("OMTAddress XML missing Name".into()))?;
        let port: u16 = xml_text(xml, "Port")
            .ok_or_else(|| OmtError::InvalidArgument("OMTAddress XML missing Port".into()))?
            .parse()
            .map_err(|_| OmtError::InvalidArgument("OMTAddress XML invalid Port".into()))?;
        let mut addr = Self::from_full_name(&name, port);
        addr.removed = xml_text(xml, "Removed")
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        addr.addresses = xml_all_text(xml, "IPAddress");
        Ok(addr)
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

fn xml_text(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].trim().to_string())
}

fn xml_all_text(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(pos) = rest.find(&open) {
        let start = pos + open.len();
        if let Some(end_rel) = rest[start..].find(&close) {
            let end = start + end_rel;
            out.push(rest[start..end].trim().to_string());
            rest = &rest[end + close.len()..];
        } else {
            break;
        }
    }
    out
}

fn hostname() -> String {
    env::var("COMPUTERNAME")
        .or_else(|_| env::var("HOSTNAME"))
        .unwrap_or_else(|_| "localhost".into())
        .to_ascii_uppercase()
}

fn local_ips() -> Vec<String> {
    let mut out = vec!["127.0.0.1".to_string()];
    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0")
        && socket.connect("8.8.8.8:80").is_ok()
        && let Ok(local) = socket.local_addr()
    {
        let ip = local.ip().to_string();
        if ip != "0.0.0.0" && !out.contains(&ip) {
            out.insert(0, ip);
        }
    }
    out
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
}
