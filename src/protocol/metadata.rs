//! Metadata XML templates and helpers.
//!
//! These strings must match libomtnet exactly — receivers use string equality,
//! not XML parsing. Note the intentional `Program==` double-equals quirk.

use crate::types::{Quality, Tally};

/// Subscribe to video frames.
pub const SUBSCRIBE_VIDEO: &str = r#"<OMTSubscribe Video="true" />"#;
/// Subscribe to audio frames.
pub const SUBSCRIBE_AUDIO: &str = r#"<OMTSubscribe Audio="true" />"#;
/// Subscribe to metadata frames.
pub const SUBSCRIBE_METADATA: &str = r#"<OMTSubscribe Metadata="true" />"#;

/// Enable preview mode.
pub const PREVIEW_ON: &str = r#"<OMTSettings Preview="true" />"#;
/// Disable preview mode.
pub const PREVIEW_OFF: &str = r#"<OMTSettings Preview="false" />"#;

/// Suggested quality template (`Default` / `Low` / `Medium` / `High`).
pub const SUGGESTED_QUALITY: &str = r#"<OMTSettings Quality="Default" />"#;
/// Prefix used when building a quality suggestion dynamically.
pub const SUGGESTED_QUALITY_PREFIX: &str = r#"<OMTSettings Quality="#;

/// Tally: preview on, program off.
///
/// NOTE: `Program==` (double equals) is intentional and must be preserved.
pub const TALLY_PREVIEW: &str = r#"<OMTTally Preview="true" Program=="false" />"#;
/// Tally: preview off, program on.
pub const TALLY_PROGRAM: &str = r#"<OMTTally Preview="false" Program=="true" />"#;
/// Tally: both on.
pub const TALLY_PREVIEW_PROGRAM: &str = r#"<OMTTally Preview="true" Program=="true" />"#;
/// Tally: both off.
pub const TALLY_NONE: &str = r#"<OMTTally Preview="false" Program=="false" />"#;

/// Sender info element name.
pub const SENDER_INFO_NAME: &str = "OMTInfo";
/// Sender info XML prefix.
pub const SENDER_INFO_PREFIX: &str = "<OMTInfo";
/// Address element name.
pub const ADDRESS_NAME: &str = "OMTAddress";
/// Redirect element name.
pub const REDIRECT_NAME: &str = "OMTRedirect";
/// Redirect XML prefix.
pub const REDIRECT_PREFIX: &str = "<OMTRedirect";

/// Build a suggested-quality metadata string for `quality`.
pub fn suggested_quality_xml(quality: Quality) -> String {
    format!(r#"<OMTSettings Quality="{}" />"#, quality.as_str())
}

/// Map a tally state to the exact wire XML constant.
pub fn tally_xml(tally: Tally) -> &'static str {
    match (tally.preview != 0, tally.program != 0) {
        (false, false) => TALLY_NONE,
        (true, false) => TALLY_PREVIEW,
        (false, true) => TALLY_PROGRAM,
        (true, true) => TALLY_PREVIEW_PROGRAM,
    }
}

/// Encode UTF-8 XML metadata with a trailing NUL (as required on the wire).
pub fn encode_metadata_xml(xml: &str) -> Vec<u8> {
    let mut out = xml.as_bytes().to_vec();
    out.push(0);
    out
}

/// Decode NUL-terminated UTF-8 metadata to a string (NUL stripped).
pub fn decode_metadata_xml(bytes: &[u8]) -> Result<String, crate::error::OmtError> {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8(bytes[..end].to_vec())
        .map_err(|e| crate::error::OmtError::Protocol(format!("invalid metadata utf-8: {e}")))
}

/// Classify a metadata command by exact string match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataCommand {
    /// Subscribe video.
    SubscribeVideo,
    /// Subscribe audio.
    SubscribeAudio,
    /// Subscribe metadata.
    SubscribeMetadata,
    /// Preview on.
    PreviewOn,
    /// Preview off.
    PreviewOff,
    /// Tally update.
    Tally(Tally),
}

/// Match a metadata XML string to a known command.
pub fn classify(xml: &str) -> Option<MetadataCommand> {
    if xml == SUBSCRIBE_VIDEO {
        Some(MetadataCommand::SubscribeVideo)
    } else if xml == SUBSCRIBE_AUDIO {
        Some(MetadataCommand::SubscribeAudio)
    } else if xml == SUBSCRIBE_METADATA {
        Some(MetadataCommand::SubscribeMetadata)
    } else if xml == PREVIEW_ON {
        Some(MetadataCommand::PreviewOn)
    } else if xml == PREVIEW_OFF {
        Some(MetadataCommand::PreviewOff)
    } else {
        match xml {
            x if x == TALLY_NONE => Some(MetadataCommand::Tally(Tally {
                preview: 0,
                program: 0,
            })),
            x if x == TALLY_PREVIEW => Some(MetadataCommand::Tally(Tally {
                preview: 1,
                program: 0,
            })),
            x if x == TALLY_PROGRAM => Some(MetadataCommand::Tally(Tally {
                preview: 0,
                program: 1,
            })),
            x if x == TALLY_PREVIEW_PROGRAM => Some(MetadataCommand::Tally(Tally {
                preview: 1,
                program: 1,
            })),
            _ => None,
        }
    }
}

/// Alias used by sender control plane.
pub fn from_tally(tally: Tally) -> &'static str {
    tally_xml(tally)
}

/// Alias for suggested quality XML builder.
pub fn suggested_quality_xml_alias(q: Quality) -> String {
    suggested_quality_xml(q)
}

/// Returns true if `xml` is an exact subscribe command for the given type keyword.
pub fn is_subscribe(xml: &str, kind: &str) -> bool {
    match kind {
        "Video" => xml == SUBSCRIBE_VIDEO,
        "Audio" => xml == SUBSCRIBE_AUDIO,
        "Metadata" => xml == SUBSCRIBE_METADATA,
        _ => false,
    }
}
