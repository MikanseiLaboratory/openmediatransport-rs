//! Metadata encode/decode tests.

use openmediatransport::protocol::metadata::{
    decode_metadata_xml, encode_metadata_xml, tally_xml, SUBSCRIBE_VIDEO, TALLY_NONE,
    TALLY_PREVIEW, TALLY_PREVIEW_PROGRAM, TALLY_PROGRAM,
};
use openmediatransport::types::Tally;

#[test]
fn metadata_nul_terminated() {
    let bytes = encode_metadata_xml(SUBSCRIBE_VIDEO);
    assert_eq!(*bytes.last().unwrap(), 0);
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
