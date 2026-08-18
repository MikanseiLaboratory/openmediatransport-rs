//! Public API tests for libomtnet-compatible `settings.xml`.

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use openmediatransport::{
    KEY_DISCOVERY_SERVER, KEY_NETWORK_PORT_END, KEY_NETWORK_PORT_START, NETWORK_PORT_END,
    NETWORK_PORT_START, Settings,
};

fn temp_settings_file() -> std::path::PathBuf {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("omt-rs-settings-{}-{n}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir.join("settings.xml")
}

fn cleanup(path: &std::path::Path) {
    let _ = fs::remove_file(path);
    if let Some(dir) = path.parent() {
        let _ = fs::remove_dir(dir);
    }
}

#[test]
fn load_and_save_match_libomtnet_xml_shape() {
    let path = temp_settings_file();
    let mut settings = Settings::from_path(&path);
    settings.set_string(KEY_DISCOVERY_SERVER, "omt://10.0.0.5:6399");
    settings.set_integer(KEY_NETWORK_PORT_START, 6500);
    settings.set_integer(KEY_NETWORK_PORT_END, 6600);
    settings.save().unwrap();

    let xml = fs::read_to_string(&path).unwrap();
    assert!(xml.contains("<Settings>"));
    assert!(xml.contains("<DiscoveryServer>omt://10.0.0.5:6399</DiscoveryServer>"));
    assert!(xml.contains("<NetworkPortStart>6500</NetworkPortStart>"));
    assert!(xml.contains("<NetworkPortEnd>6600</NetworkPortEnd>"));

    let loaded = Settings::from_path(&path);
    assert_eq!(
        loaded.discovery_server().as_deref(),
        Some("omt://10.0.0.5:6399")
    );
    assert_eq!(loaded.network_port_range(), (6500, 6600));
    cleanup(&path);
}

#[test]
fn blank_discovery_server_means_dns_sd() {
    let path = temp_settings_file();
    let settings = Settings::from_path(&path);
    assert!(settings.discovery_server().is_none());
    assert_eq!(
        settings.network_port_range(),
        (NETWORK_PORT_START, NETWORK_PORT_END)
    );
    cleanup(&path);
}

#[test]
fn missing_keys_use_defaults() {
    let path = temp_settings_file();
    fs::write(
        &path,
        "<Settings>\n  <DiscoveryServer>omt://127.0.0.1:6399</DiscoveryServer>\n</Settings>\n",
    )
    .unwrap();
    let settings = Settings::from_path(&path);
    assert_eq!(
        settings.get_integer(KEY_NETWORK_PORT_START, i32::from(NETWORK_PORT_START)),
        i32::from(NETWORK_PORT_START)
    );
    assert_eq!(settings.get_string("UnknownKey", "fallback"), "fallback");
    cleanup(&path);
}
