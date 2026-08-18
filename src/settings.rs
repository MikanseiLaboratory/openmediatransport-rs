//! Persistent OMT settings (`settings.xml`), matching libomtnet `OMTSettings`.
//!
//! Keys are first-level XML elements under `<Settings>` — there is no fixed
//! schema. Typical keys:
//!
//! - [`KEY_DISCOVERY_SERVER`] (`DiscoveryServer`) — `omt://host:port`; blank = DNS-SD
//! - [`KEY_NETWORK_PORT_START`] (`NetworkPortStart`) — first sender listen port (default 6400)
//! - [`KEY_NETWORK_PORT_END`] (`NetworkPortEnd`) — last sender listen port (default 6600)
//!
//! Storage location (file name is always `settings.xml`):
//!
//! - Windows: `%ProgramData%\OMT\` (`C:\ProgramData\OMT\`)
//! - macOS / Linux: `~/.OMT/`
//! - Override directory with the [`OMT_STORAGE_PATH`] environment variable
//!
//! [`Settings::global`] loads the file once per process. [`Settings::set_string`]
//! updates memory only; call [`Settings::save`] to persist. To override discovery
//! for a single process without a file, set `DiscoveryServer` before constructing
//! [`crate::Discovery`]:
//!
//! ```no_run
//! use openmediatransport::{Settings, KEY_DISCOVERY_SERVER};
//!
//! Settings::global()
//!     .lock()
//!     .expect("settings lock")
//!     .set_string(KEY_DISCOVERY_SERVER, "omt://127.0.0.1:6399");
//! ```

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::error::OmtError;
use crate::types::{NETWORK_PORT_END, NETWORK_PORT_START};

/// Environment variable overriding the settings storage directory.
pub const OMT_STORAGE_PATH: &str = "OMT_STORAGE_PATH";

/// File name used under the storage directory (libomtnet `settings.xml`).
pub const SETTINGS_FILE_NAME: &str = "settings.xml";

/// Discovery server URL (`omt://host:port`). Empty = default DNS-SD.
pub const KEY_DISCOVERY_SERVER: &str = "DiscoveryServer";

/// First TCP port used when binding a sender (default [`NETWORK_PORT_START`]).
pub const KEY_NETWORK_PORT_START: &str = "NetworkPortStart";

/// Last TCP port used when binding a sender (default [`NETWORK_PORT_END`]).
pub const KEY_NETWORK_PORT_END: &str = "NetworkPortEnd";

static GLOBAL: OnceLock<Mutex<Settings>> = OnceLock::new();

/// Key/value settings store backed by `settings.xml`.
#[derive(Debug, Clone)]
pub struct Settings {
    path: PathBuf,
    values: BTreeMap<String, String>,
}

impl Settings {
    /// Process-wide settings, loaded from the platform `settings.xml`.
    ///
    /// Matches libomtnet `OMTSettings.GetInstance()`.
    pub fn global() -> &'static Mutex<Settings> {
        GLOBAL.get_or_init(|| Mutex::new(load_default()))
    }

    /// Load settings from `path` (missing or unreadable files yield empty values).
    pub fn from_path(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let values = match fs::read_to_string(&path) {
            Ok(xml) => parse_settings_xml(&xml),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => {
                tracing::warn!("OMTSettings: failed to read {}: {e}", path.display());
                BTreeMap::new()
            }
        };
        Self { path, values }
    }

    /// Path of the XML file this instance reads and writes.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Number of stored keys.
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// True when no keys are stored.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// True when `key` is present (even if the value is empty).
    pub fn contains_key(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }

    /// Keys in sorted order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.values.keys().map(String::as_str)
    }

    /// Key/value pairs in sorted key order.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &str)> {
        self.values.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// All key/value pairs as owned strings (sorted by key).
    pub fn to_vec(&self) -> Vec<(String, String)> {
        self.values
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Remove a key from memory. Does not write the file until [`Self::save`].
    pub fn remove(&mut self, key: &str) -> Option<String> {
        self.values.remove(key)
    }

    /// Reload values from disk, replacing the in-memory map.
    ///
    /// Missing or unreadable files yield an empty map (same as [`Self::from_path`]).
    pub fn reload(&mut self) {
        *self = Self::from_path(self.path.clone());
    }

    /// Look up a string key, returning `default` when missing.
    pub fn get_string(&self, key: &str, default: &str) -> String {
        self.values
            .get(key)
            .cloned()
            .unwrap_or_else(|| default.to_string())
    }

    /// Set a string key in memory. Does not write the file until [`Self::save`].
    ///
    /// Invalid XML element names are ignored (libomtnet would throw from
    /// `XmlDocument.CreateElement`).
    pub fn set_string(&mut self, key: &str, value: impl Into<String>) {
        if !is_xml_name(key) {
            tracing::warn!("OMTSettings: ignoring invalid key {key:?}");
            return;
        }
        self.values.insert(key.to_string(), value.into());
    }

    /// Look up an integer key, returning `default` when missing or unparsable.
    pub fn get_integer(&self, key: &str, default: i32) -> i32 {
        let value = self.get_string(key, "");
        if value.is_empty() {
            return default;
        }
        value.parse().unwrap_or(default)
    }

    /// Set an integer key in memory. Does not write the file until [`Self::save`].
    pub fn set_integer(&mut self, key: &str, value: i32) {
        self.set_string(key, value.to_string());
    }

    /// Configured discovery server URL, or `None` when DNS-SD should be used.
    pub fn discovery_server(&self) -> Option<String> {
        let url = self.get_string(KEY_DISCOVERY_SERVER, "");
        let url = url.trim();
        if url.is_empty() {
            None
        } else {
            Some(url.to_string())
        }
    }

    /// Sender listen-port range from settings, falling back to OMT defaults.
    pub fn network_port_range(&self) -> (u16, u16) {
        (
            port_or_default(
                self.get_integer(KEY_NETWORK_PORT_START, i32::from(NETWORK_PORT_START)),
                NETWORK_PORT_START,
            ),
            port_or_default(
                self.get_integer(KEY_NETWORK_PORT_END, i32::from(NETWORK_PORT_END)),
                NETWORK_PORT_END,
            ),
        )
    }

    /// Write current keys to disk as indented `<Settings>` XML.
    pub fn save(&self) -> Result<(), OmtError> {
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.path, to_xml(&self.values))?;
        Ok(())
    }
}

/// Directory that contains `settings.xml`.
pub fn storage_dir() -> PathBuf {
    if let Ok(p) = env::var(OMT_STORAGE_PATH)
        && !p.is_empty()
    {
        return PathBuf::from(p);
    }
    default_storage_dir()
}

/// Full path of the process-wide `settings.xml`.
pub fn settings_file_path() -> PathBuf {
    storage_dir().join(SETTINGS_FILE_NAME)
}

/// Sender port range from the process-wide settings file (or OMT defaults).
pub(crate) fn global_network_port_range() -> (u16, u16) {
    Settings::global()
        .lock()
        .map(|s| s.network_port_range())
        .unwrap_or((NETWORK_PORT_START, NETWORK_PORT_END))
}

/// Discovery server URL from the process-wide settings file.
pub(crate) fn global_discovery_server() -> Option<String> {
    Settings::global()
        .lock()
        .ok()
        .and_then(|s| s.discovery_server())
}

fn load_default() -> Settings {
    Settings::from_path(settings_file_path())
}

fn default_storage_dir() -> PathBuf {
    #[cfg(windows)]
    {
        env::var_os("ProgramData")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
            .join("OMT")
    }
    #[cfg(not(windows))]
    {
        home_dir().join(".OMT")
    }
}

#[cfg(not(windows))]
fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn port_or_default(value: i32, default: u16) -> u16 {
    u16::try_from(value).unwrap_or(default)
}

fn is_xml_name(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn unescape_xml(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn parse_settings_xml(xml: &str) -> BTreeMap<String, String> {
    let xml = xml.trim_start_matches('\u{feff}');
    let Some(body) = inner_element(xml, "Settings") else {
        tracing::warn!("OMTSettings: no <Settings> root; using empty settings");
        return BTreeMap::new();
    };
    parse_children(body)
}

fn inner_element<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}");
    let start = xml.find(&open)?;
    let after_name = start + open.len();
    let gt = xml[after_name..].find('>')? + after_name;
    let open_tag = xml[start..=gt].trim_end();
    if open_tag.ends_with("/>") {
        return Some("");
    }
    let close = format!("</{tag}>");
    let content_start = gt + 1;
    let end = xml[content_start..].find(&close)? + content_start;
    Some(&xml[content_start..end])
}

fn parse_children(body: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let mut rest = body;
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        if let Some(stripped) = rest.strip_prefix("<!--") {
            match stripped.find("-->") {
                Some(end) => rest = &stripped[end + 3..],
                None => break,
            }
            continue;
        }
        if rest.starts_with("<?") {
            match rest.find("?>") {
                Some(end) => rest = &rest[end + 2..],
                None => break,
            }
            continue;
        }
        if !rest.starts_with('<') {
            match rest.find('<') {
                Some(pos) => rest = &rest[pos..],
                None => break,
            }
            continue;
        }
        if rest.starts_with("</") {
            break;
        }
        let after_lt = &rest[1..];
        let name_end = after_lt
            .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
            .unwrap_or(after_lt.len());
        let name = &after_lt[..name_end];
        if name.is_empty() || !is_xml_name(name) {
            break;
        }
        let after_name = &after_lt[name_end..];
        let Some(gt) = after_name.find('>') else {
            break;
        };
        let self_closing = after_name[..gt].trim_end().ends_with('/');
        rest = &after_name[gt + 1..];
        if self_closing {
            map.insert(name.to_string(), String::new());
            continue;
        }
        let close = format!("</{name}>");
        let Some(end) = rest.find(&close) else {
            break;
        };
        let inner = unescape_xml(rest[..end].trim());
        map.insert(name.to_string(), inner);
        rest = &rest[end + close.len()..];
    }
    map
}

fn to_xml(values: &BTreeMap<String, String>) -> String {
    let mut out = String::from("<Settings>\n");
    for (k, v) in values {
        out.push_str("  <");
        out.push_str(k);
        out.push('>');
        out.push_str(&escape_xml(v));
        out.push_str("</");
        out.push_str(k);
        out.push_str(">\n");
    }
    out.push_str("</Settings>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_settings_path() -> PathBuf {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir()
            .join(format!("omt-settings-{}-{n}", std::process::id()))
            .join(SETTINGS_FILE_NAME)
    }

    #[test]
    fn parse_documented_discovery_server_xml() {
        let xml = r#"<Settings>
  <DiscoveryServer>omt://x.x.x.x:port</DiscoveryServer>
</Settings>"#;
        let map = parse_settings_xml(xml);
        assert_eq!(
            map.get(KEY_DISCOVERY_SERVER).map(String::as_str),
            Some("omt://x.x.x.x:port")
        );
    }

    #[test]
    fn roundtrip_string_and_integer_keys() {
        let path = temp_settings_path();
        let mut settings = Settings::from_path(&path);
        settings.set_string(KEY_DISCOVERY_SERVER, "omt://127.0.0.1:6399");
        settings.set_integer(KEY_NETWORK_PORT_START, 6500);
        settings.set_integer(KEY_NETWORK_PORT_END, 6510);
        settings.save().unwrap();

        let loaded = Settings::from_path(&path);
        assert_eq!(
            loaded.get_string(KEY_DISCOVERY_SERVER, ""),
            "omt://127.0.0.1:6399"
        );
        assert_eq!(loaded.get_integer(KEY_NETWORK_PORT_START, 0), 6500);
        assert_eq!(loaded.get_integer(KEY_NETWORK_PORT_END, 0), 6510);
        assert_eq!(loaded.network_port_range(), (6500, 6510));
        assert_eq!(
            loaded.discovery_server().as_deref(),
            Some("omt://127.0.0.1:6399")
        );

        let _ = fs::remove_file(&path);
        if let Some(dir) = path.parent() {
            let _ = fs::remove_dir(dir);
        }
    }

    #[test]
    fn missing_file_yields_defaults() {
        let path = temp_settings_path();
        let settings = Settings::from_path(&path);
        assert_eq!(
            settings.get_string(KEY_DISCOVERY_SERVER, "fallback"),
            "fallback"
        );
        assert_eq!(
            settings.network_port_range(),
            (NETWORK_PORT_START, NETWORK_PORT_END)
        );
        assert!(settings.discovery_server().is_none());
    }

    #[test]
    fn get_integer_invalid_value_returns_default() {
        let mut settings = Settings::from_path(temp_settings_path());
        settings.set_string(KEY_NETWORK_PORT_START, "nope");
        assert_eq!(settings.get_integer(KEY_NETWORK_PORT_START, 6400), 6400);
    }

    #[test]
    fn xml_escape_roundtrip() {
        let path = temp_settings_path();
        let mut settings = Settings::from_path(&path);
        settings.set_string("Note", "a < b & c > \"d\"");
        settings.save().unwrap();
        let loaded = Settings::from_path(&path);
        assert_eq!(loaded.get_string("Note", ""), "a < b & c > \"d\"");
        let _ = fs::remove_file(&path);
        if let Some(dir) = path.parent() {
            let _ = fs::remove_dir(dir);
        }
    }

    #[test]
    fn default_storage_dir_is_platform_specific() {
        let dir = default_storage_dir();
        #[cfg(windows)]
        {
            assert!(
                dir.ends_with("OMT"),
                "Windows storage dir should end with OMT: {}",
                dir.display()
            );
        }
        #[cfg(not(windows))]
        {
            assert!(
                dir.ends_with(".OMT"),
                "Unix storage dir should end with .OMT: {}",
                dir.display()
            );
        }
    }

    #[test]
    fn settings_file_name_matches_libomtnet() {
        assert!(settings_file_path().ends_with(SETTINGS_FILE_NAME));
    }

    #[test]
    fn enumerate_remove_and_reload_preserve_unknown_keys() {
        let path = temp_settings_path();
        let mut settings = Settings::from_path(&path);
        settings.set_string(KEY_DISCOVERY_SERVER, "omt://127.0.0.1:6399");
        settings.set_string("CustomFlag", "yes");
        settings.save().unwrap();

        let mut loaded = Settings::from_path(&path);
        assert!(loaded.contains_key("CustomFlag"));
        assert_eq!(loaded.len(), 2);
        let keys: Vec<_> = loaded.keys().collect();
        assert_eq!(keys, vec!["CustomFlag", KEY_DISCOVERY_SERVER]);
        assert_eq!(loaded.remove("CustomFlag").as_deref(), Some("yes"));
        assert!(!loaded.contains_key("CustomFlag"));
        loaded.save().unwrap();

        loaded.set_string("Stale", "memory-only");
        loaded.reload();
        assert!(!loaded.contains_key("CustomFlag"));
        assert!(!loaded.contains_key("Stale"));
        assert_eq!(
            loaded.get_string(KEY_DISCOVERY_SERVER, ""),
            "omt://127.0.0.1:6399"
        );

        let _ = fs::remove_file(&path);
        if let Some(dir) = path.parent() {
            let _ = fs::remove_dir(dir);
        }
    }
}
