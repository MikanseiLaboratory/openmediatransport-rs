//! Binary helpers for little-endian protocol fields.

use bytes::{Buf, BufMut, BytesMut};

/// Read a little-endian `i32`.
pub fn read_i32(buf: &mut impl Buf) -> i32 {
    buf.get_i32_le()
}

/// Write a little-endian `i32`.
pub fn write_i32(buf: &mut BytesMut, v: i32) {
    buf.put_i32_le(v);
}

/// Read a little-endian `i64`.
pub fn read_i64(buf: &mut impl Buf) -> i64 {
    buf.get_i64_le()
}

/// Write a little-endian `i64`.
pub fn write_i64(buf: &mut BytesMut, v: i64) {
    buf.put_i64_le(v);
}

/// Read a little-endian `u16`.
pub fn read_u16(buf: &mut impl Buf) -> u16 {
    buf.get_u16_le()
}

/// Write a little-endian `u16`.
pub fn write_u16(buf: &mut BytesMut, v: u16) {
    buf.put_u16_le(v);
}

/// Read a little-endian `u32`.
pub fn read_u32(buf: &mut impl Buf) -> u32 {
    buf.get_u32_le()
}

/// Write a little-endian `u32`.
pub fn write_u32(buf: &mut BytesMut, v: u32) {
    buf.put_u32_le(v);
}

/// Read a little-endian `f32`.
pub fn read_f32(buf: &mut impl Buf) -> f32 {
    buf.get_f32_le()
}

/// Write a little-endian `f32`.
pub fn write_f32(buf: &mut BytesMut, v: f32) {
    buf.put_f32_le(v);
}
