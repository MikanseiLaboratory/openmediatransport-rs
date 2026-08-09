//! FPA1 audio codec tests.

use openmediatransport::codec::fpa1::{decode_planar, encode_planar};

#[test]
fn fpa1_roundtrip() {
    let left = vec![0.0f32, 0.5, -0.5, 1.0];
    let right = vec![1.0f32, 0.25, -0.25, 0.0];
    let (encoded, active) = encode_planar(&[&left, &right]).unwrap();
    assert_eq!(active, 0b11);
    let decoded = decode_planar(&encoded, 2, 4, active).unwrap();
    assert_eq!(decoded[0], left);
    assert_eq!(decoded[1], right);
}

#[test]
fn fpa1_skips_silent_channel() {
    let left = vec![0.5f32, 0.25];
    let silent = vec![0.0f32, 0.0];
    let right = vec![-0.5f32, 1.0];
    let (encoded, active) = encode_planar(&[&left, &silent, &right]).unwrap();
    assert_eq!(active, 0b101);
    assert_eq!(encoded.len(), 2 * 2 * 4); // two active planes
    let decoded = decode_planar(&encoded, 3, 2, active).unwrap();
    assert_eq!(decoded[0], left);
    assert_eq!(decoded[1], silent);
    assert_eq!(decoded[2], right);
}

#[test]
fn fpa1_rejects_mismatched_lengths() {
    let a = vec![0.0f32; 4];
    let b = vec![0.0f32; 3];
    assert!(encode_planar(&[&a, &b]).is_err());
}
