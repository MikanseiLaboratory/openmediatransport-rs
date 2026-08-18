//! Protocol framing and metadata.

pub mod binary;
pub mod frame;
pub mod metadata;
/// Tolerant XML parser for metadata (roxmltree, plus the libomtnet `Program==` quirk).
pub mod xml;
