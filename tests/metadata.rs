//! Metadata encode/decode tests.

use std::thread;
use std::time::{Duration, Instant};

use openmediatransport::protocol::metadata::{
    SUBSCRIBE_VIDEO, TALLY_NONE, TALLY_PREVIEW, TALLY_PREVIEW_PROGRAM, TALLY_PROGRAM,
    decode_metadata_xml, encode_metadata_xml, parse_metadata, tally_xml,
};
use openmediatransport::types::Tally;
use openmediatransport::{FrameType, Metadata, ReceiverConfig, ReceiverSession, Sender};

#[test]
fn metadata_roundtrip_matches_libomtnet() {
    // libomtnet OMTBuffer.FromMetadata does not append a trailing NUL.
    let bytes = encode_metadata_xml(SUBSCRIBE_VIDEO);
    assert_ne!(*bytes.last().unwrap(), 0);
    assert_eq!(bytes, SUBSCRIBE_VIDEO.as_bytes());
    let xml = decode_metadata_xml(&bytes).unwrap();
    assert_eq!(xml, SUBSCRIBE_VIDEO);
}

#[test]
fn metadata_with_nul_still_decodes() {
    let mut bytes = SUBSCRIBE_VIDEO.as_bytes().to_vec();
    bytes.push(0);
    let xml = decode_metadata_xml(&bytes).unwrap();
    assert_eq!(xml, SUBSCRIBE_VIDEO);
}

#[test]
fn metadata_without_nul_still_decodes() {
    let xml = decode_metadata_xml(b"<OMTTally Preview=\"true\" Program==\"false\" />").unwrap();
    assert!(xml.contains("OMTTally"));
}

#[test]
fn tally_program_double_equals_quirk() {
    assert!(TALLY_PREVIEW.contains(r#"Program=="false""#));
    assert!(TALLY_PROGRAM.contains(r#"Program=="true""#));
    assert!(TALLY_PREVIEW_PROGRAM.contains(r#"Program=="true""#));
    assert!(TALLY_NONE.contains(r#"Program=="false""#));
    // Must NOT use single-equals Program=
    assert!(!TALLY_PREVIEW.contains(r#"Program="false""#));
}

#[test]
fn tally_xml_mapping() {
    assert_eq!(tally_xml(Tally::new(0, 0)), TALLY_NONE);
    assert_eq!(tally_xml(Tally::new(1, 0)), TALLY_PREVIEW);
    assert_eq!(tally_xml(Tally::new(0, 1)), TALLY_PROGRAM);
    assert_eq!(tally_xml(Tally::new(1, 1)), TALLY_PREVIEW_PROGRAM);
}

#[test]
fn parse_metadata_reads_attribute_keys() {
    let items = parse_metadata(r#"<OMTTally Preview="true" Program=="true" />"#);
    assert_eq!(items, vec![Metadata::Tally(Tally::new(1, 1))]);
}

#[test]
fn receiver_tally_updates_sender() {
    let mut sender =
        Sender::create("TallySrc", FrameType::VIDEO | FrameType::METADATA).expect("sender");
    let url = format!("omt://127.0.0.1:{}", sender.port());
    let rx = ReceiverSession::connect(
        url,
        ReceiverConfig {
            frame_types: FrameType::VIDEO | FrameType::METADATA,
            auto_reconnect: false,
            connect_timeout: Duration::from_secs(3),
            ..ReceiverConfig::default()
        },
    )
    .expect("rx");

    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        let _ = sender.poll_accept();
        let _ = sender.poll_peer_metadata();
        if sender.connection_count() > 0 {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(sender.connection_count() > 0, "receiver did not connect");

    rx.set_tally(Tally::new(1, 1)).expect("set_tally");

    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        let _ = sender.poll_accept();
        let _ = sender.poll_peer_metadata();
        if sender.tally() == Tally::new(1, 1) {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("expected program+preview tally, got {:?}", sender.tally());
}
