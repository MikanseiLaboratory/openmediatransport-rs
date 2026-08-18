//! Public API tests for the observable discovery server handle.

use std::net::SocketAddr;
use std::thread;
use std::time::Duration;

use openmediatransport::{
    DISCOVERY_SERVER_DEFAULT_PORT, DiscoveryClient, DiscoveryServerEvent, DiscoveryServerHandle,
    OmtAddress, OmtError, default_bind_addr,
};

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
    let mut client = DiscoveryClient::new("127.0.0.1");
    client.port = handle.bind().port();
    client.connect().unwrap();
    client
}

fn source(name: &str, port: u16) -> OmtAddress {
    let mut addr = OmtAddress::from_full_name(name, port);
    addr.addresses = vec!["10.0.0.1".into()];
    addr
}

#[test]
fn default_bind_is_ipv6_any_like_official_app() {
    let addr = default_bind_addr(DISCOVERY_SERVER_DEFAULT_PORT);
    assert!(addr.ip().is_unspecified());
    assert!(addr.is_ipv6());
    assert_eq!(addr.port(), DISCOVERY_SERVER_DEFAULT_PORT);
}

#[test]
fn handle_relays_register_and_replaces_client_ip() {
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
fn custom_bind_rejects_second_listener() {
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
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
