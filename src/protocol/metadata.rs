//! Metadata XML templates, typed parse, and helpers.
//!
//! **Sending** still emits the exact libomtnet control strings (including the
//! tally `Program==` quirk) so official stacks that compare whole strings keep
//! working.
//!
//! **Receiving** matches the four libomtnet tally constants by exact string
//! (those documents are not well-formed XML). Everything else is parsed with
//! `roxmltree` by element/attribute keys, including well-formed `<OMTTally>`.

use crate::error::OmtError;
use crate::protocol::xml::{XmlElement, escape_xml, parse_bool};
use crate::types::{MetadataFrame, Quality, SenderInfo, Tally};

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
/// NOTE: `Program==` (double equals) is intentional on the wire.
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

/// Parsed OMT metadata document (control + [recommended application types](https://github.com/openmediatransport/Metadata)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Metadata {
    /// `<OMTSubscribe … />`
    Subscribe {
        /// Video subscription requested.
        video: bool,
        /// Audio subscription requested.
        audio: bool,
        /// Metadata subscription requested.
        metadata: bool,
    },
    /// `<OMTSettings Preview="…" Quality="…" />` (either attribute may be absent).
    Settings {
        /// Preview mode when the `Preview` attribute is present.
        preview: Option<bool>,
        /// Suggested quality when the `Quality` attribute is present.
        quality: Option<Quality>,
    },
    /// `<OMTTally Preview="…" Program="…" />` (wire form may use `Program==`).
    Tally(Tally),
    /// `<OMTInfo … />`
    SenderInfo(SenderInfo),
    /// `<OMTRedirect NewAddress="…" />`
    Redirect {
        /// New source URL / name.
        new_address: String,
    },
    /// `<OMTAddress>` discovery registration.
    Address {
        /// Instance name (`MACHINE (Source)`).
        name: String,
        /// TCP port.
        port: u16,
        /// Whether this entry was removed.
        removed: bool,
        /// Advertised IP addresses.
        addresses: Vec<String>,
    },
    /// `<OMTWeb URL="…" />`
    Web {
        /// Management URL.
        url: String,
    },
    /// `<OMTPTZ … />`
    Ptz(PtzMetadata),
    /// `<AncillaryData>…</AncillaryData>`
    Ancillary(AncillaryMetadata),
    /// Unrecognized element (original tree retained).
    Unknown(XmlElement),
}

/// PTZ metadata ([spec](https://github.com/openmediatransport/Metadata#ptz-control)).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PtzMetadata {
    /// `VISCAoverIP` or `VISCA`.
    pub protocol: String,
    /// `visca://host:port` for out-of-band VISCA.
    pub url: Option<String>,
    /// In-band VISCA sequence number.
    pub sequence: Option<String>,
    /// Hex command payload (controller → camera).
    pub command: Option<String>,
    /// Hex reply payload (camera → controller).
    pub reply: Option<String>,
}

impl PtzMetadata {
    /// VISCA-over-IP advertisement XML.
    pub fn visca_over_ip_xml(url: &str) -> String {
        format!(
            r#"<OMTPTZ Protocol="VISCAoverIP" URL="{}" />"#,
            escape_xml(url)
        )
    }

    /// In-band VISCA command XML.
    pub fn visca_command_xml(sequence: &str, command_hex: &str) -> String {
        format!(
            r#"<OMTPTZ Protocol="VISCA" Sequence="{}" Command="{}" />"#,
            escape_xml(sequence),
            escape_xml(command_hex)
        )
    }

    /// In-band VISCA reply XML.
    pub fn visca_reply_xml(sequence: &str, reply_hex: &str) -> String {
        format!(
            r#"<OMTPTZ Protocol="VISCA" Sequence="{}" Reply="{}" />"#,
            escape_xml(sequence),
            escape_xml(reply_hex)
        )
    }
}

/// SDI ancillary metadata ([spec](https://github.com/openmediatransport/Metadata#ancillary-data)).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AncillaryMetadata {
    /// `xmns` / `xmlns` attribute when present.
    pub xmlns: Option<String>,
    /// `<Packet>` children.
    pub packets: Vec<AncillaryPacket>,
}

/// One ancillary packet.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AncillaryPacket {
    /// Packet attributes (`did`, `sdid`, `line`, …).
    pub attributes: Vec<(String, String)>,
    /// Hex payload from `<Payload>`.
    pub payload: String,
}

impl AncillaryPacket {
    /// Attribute value for `name`.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// Web-management advertisement XML.
pub fn web_xml(url: &str) -> String {
    format!(r#"<OMTWeb URL="{}" />"#, escape_xml(url))
}

/// Wrap `inner` documents in `<OMTGroup>`.
pub fn group_xml(inner: &[&str]) -> String {
    let mut out = String::from("<OMTGroup>\n");
    for xml in inner {
        out.push_str(xml.trim());
        out.push('\n');
    }
    out.push_str("</OMTGroup>");
    out
}

/// Redirect metadata XML (`OMTRedirect.ToXML`).
pub fn redirect_xml(new_address: &str) -> String {
    format!(
        r#"<OMTRedirect NewAddress="{}" />"#,
        escape_xml(new_address)
    )
}

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

/// Encode UTF-8 XML metadata for a metadata-frame Data payload.
///
/// Matches libomtnet `OMTBuffer.FromMetadata`: UTF-8 bytes **without** a trailing NUL.
/// (Per-frame metadata attached to video/audio still uses `MetadataLength` including NUL.)
pub fn encode_metadata_xml(xml: &str) -> Vec<u8> {
    xml.as_bytes().to_vec()
}

/// Decode UTF-8 metadata to a string (trailing NUL stripped if present).
pub fn decode_metadata_xml(bytes: &[u8]) -> Result<String, OmtError> {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8(bytes[..end].to_vec())
        .map_err(|e| OmtError::Protocol(format!("invalid metadata utf-8: {e}")))
}

/// Parse XML into typed documents. `<OMTGroup>` children are flattened.
///
/// Tally uses exact string match against the libomtnet constants first, because
/// those strings contain `Program==` and are not valid XML. Well-formed
/// `<OMTTally>` still goes through the XML parser.
pub fn parse_metadata(xml: &str) -> Vec<Metadata> {
    if let Some(tally) = tally_from_libomtnet_constant(xml) {
        return vec![Metadata::Tally(tally)];
    }
    match XmlElement::parse(xml) {
        Ok(root) => flatten(element_to_metadata(&root)),
        Err(_) => Vec::new(),
    }
}

fn tally_from_libomtnet_constant(xml: &str) -> Option<Tally> {
    match xml {
        TALLY_PREVIEW => Some(Tally::new(1, 0)),
        TALLY_PROGRAM => Some(Tally::new(0, 1)),
        TALLY_PREVIEW_PROGRAM => Some(Tally::new(1, 1)),
        TALLY_NONE => Some(Tally::new(0, 0)),
        _ => None,
    }
}

fn flatten(item: Metadata) -> Vec<Metadata> {
    match item {
        Metadata::Unknown(el) if el.name == "OMTGroup" => el
            .children
            .iter()
            .flat_map(|c| flatten(element_to_metadata(c)))
            .collect(),
        other => vec![other],
    }
}

fn element_to_metadata(el: &XmlElement) -> Metadata {
    match el.name.as_str() {
        "OMTSubscribe" => Metadata::Subscribe {
            video: el.attr_bool("Video").unwrap_or(false),
            audio: el.attr_bool("Audio").unwrap_or(false),
            metadata: el.attr_bool("Metadata").unwrap_or(false),
        },
        "OMTSettings" => Metadata::Settings {
            preview: el.attr_bool("Preview"),
            quality: el.attr("Quality").and_then(quality_from_name),
        },
        "OMTTally" => Metadata::Tally(Tally::new(
            i32::from(el.attr_bool("Preview").unwrap_or(false)),
            i32::from(el.attr_bool("Program").unwrap_or(false)),
        )),
        "OMTInfo" => Metadata::SenderInfo(SenderInfo::new(
            el.attr("ProductName").unwrap_or_default(),
            el.attr("Manufacturer").unwrap_or_default(),
            el.attr("Version").unwrap_or_default(),
        )),
        "OMTRedirect" => Metadata::Redirect {
            new_address: el.attr("NewAddress").unwrap_or_default().to_string(),
        },
        "OMTAddress" => {
            let name = el.child_text("Name").unwrap_or("").to_string();
            let port = el
                .child_text("Port")
                .and_then(|p| p.parse().ok())
                .unwrap_or(0);
            let removed = el
                .child_text("Removed")
                .and_then(parse_bool)
                .unwrap_or(false);
            let addresses = el
                .child("Addresses")
                .map(|a| {
                    a.children_named("IPAddress")
                        .chain(a.children_named("Address"))
                        .map(|c| c.text.clone())
                        .collect()
                })
                .unwrap_or_default();
            Metadata::Address {
                name,
                port,
                removed,
                addresses,
            }
        }
        "OMTWeb" => Metadata::Web {
            url: el.attr("URL").unwrap_or_default().to_string(),
        },
        "OMTPTZ" => Metadata::Ptz(PtzMetadata {
            protocol: el.attr("Protocol").unwrap_or_default().to_string(),
            url: el.attr("URL").map(str::to_string),
            sequence: el.attr("Sequence").map(str::to_string),
            command: el.attr("Command").map(str::to_string),
            reply: el.attr("Reply").map(str::to_string),
        }),
        "AncillaryData" => {
            let xmlns = el
                .attr("xmlns")
                .or_else(|| el.attr("xmns"))
                .map(str::to_string);
            let packets = el
                .children_named("Packet")
                .map(|p| AncillaryPacket {
                    attributes: p.attributes.clone(),
                    payload: p.child_text("Payload").unwrap_or("").to_string(),
                })
                .collect();
            Metadata::Ancillary(AncillaryMetadata { xmlns, packets })
        }
        "OMTGroup" => Metadata::Unknown(el.clone()),
        _ => Metadata::Unknown(el.clone()),
    }
}

fn quality_from_name(name: &str) -> Option<Quality> {
    match name {
        "Default" => Some(Quality::Default),
        "Low" => Some(Quality::Low),
        "Medium" => Some(Quality::Medium),
        "High" => Some(Quality::High),
        _ => None,
    }
}

/// Classify a metadata command by XML keys (not whole-string equality).
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
    /// Suggested quality.
    Quality(Quality),
    /// Tally update.
    Tally(Tally),
}

/// Match a metadata XML string to known control commands (flattened).
pub fn classify(xml: &str) -> Vec<MetadataCommand> {
    parse_metadata(xml)
        .into_iter()
        .flat_map(metadata_to_commands)
        .collect()
}

fn metadata_to_commands(item: Metadata) -> Vec<MetadataCommand> {
    match item {
        Metadata::Subscribe {
            video,
            audio,
            metadata,
        } => {
            let mut out = Vec::new();
            if video {
                out.push(MetadataCommand::SubscribeVideo);
            }
            if audio {
                out.push(MetadataCommand::SubscribeAudio);
            }
            if metadata {
                out.push(MetadataCommand::SubscribeMetadata);
            }
            out
        }
        Metadata::Settings { preview, quality } => {
            let mut out = Vec::new();
            if let Some(on) = preview {
                out.push(if on {
                    MetadataCommand::PreviewOn
                } else {
                    MetadataCommand::PreviewOff
                });
            }
            if let Some(q) = quality {
                out.push(MetadataCommand::Quality(q));
            }
            out
        }
        Metadata::Tally(t) => vec![MetadataCommand::Tally(t)],
        _ => Vec::new(),
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

/// Returns true if `xml` requests a subscribe for the given type keyword.
pub fn is_subscribe(xml: &str, kind: &str) -> bool {
    parse_metadata(xml).iter().any(|m| {
        matches!(
            (kind, m),
            ("Video", Metadata::Subscribe { video: true, .. })
                | ("Audio", Metadata::Subscribe { audio: true, .. })
                | ("Metadata", Metadata::Subscribe { metadata: true, .. })
        )
    })
}

impl MetadataFrame {
    /// Parse this frame's XML into typed documents (`OMTGroup` is flattened).
    pub fn parse(&self) -> Vec<Metadata> {
        parse_metadata(&self.xml)
    }
}

impl SenderInfo {
    /// Parse `<OMTInfo … />` (attribute lookup, not string equality).
    pub fn from_xml(xml: &str) -> Option<Self> {
        parse_metadata(xml).into_iter().find_map(|m| match m {
            Metadata::SenderInfo(info) => Some(info),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_exact_libomtnet_tally_constants() {
        assert_eq!(
            parse_metadata(TALLY_PREVIEW),
            vec![Metadata::Tally(Tally::new(1, 0))]
        );
        assert_eq!(
            parse_metadata(TALLY_PROGRAM),
            vec![Metadata::Tally(Tally::new(0, 1))]
        );
        assert_eq!(
            parse_metadata(TALLY_PREVIEW_PROGRAM),
            vec![Metadata::Tally(Tally::new(1, 1))]
        );
        assert_eq!(
            parse_metadata(TALLY_NONE),
            vec![Metadata::Tally(Tally::new(0, 0))]
        );
    }

    #[test]
    fn parse_well_formed_tally_and_whitespace() {
        let xml = "  <OMTTally  Preview=\"true\"  Program=\"true\" />\n";
        assert_eq!(parse_metadata(xml), vec![Metadata::Tally(Tally::new(1, 1))]);
    }

    #[test]
    fn non_constant_program_double_equals_is_not_xml() {
        let xml = r#"<OMTTally Preview="true"  Program=="false" />"#;
        assert!(parse_metadata(xml).is_empty());
    }

    #[test]
    fn parse_combined_subscribe() {
        let items = parse_metadata(r#"<OMTSubscribe Video="true" Audio="true" />"#);
        assert_eq!(
            items,
            vec![Metadata::Subscribe {
                video: true,
                audio: true,
                metadata: false,
            }]
        );
        assert!(is_subscribe(
            r#"<OMTSubscribe Video="true" Audio="true" />"#,
            "Video"
        ));
        assert!(is_subscribe(
            r#"<OMTSubscribe Video="true" Audio="true" />"#,
            "Audio"
        ));
    }

    #[test]
    fn parse_settings_quality_attribute() {
        assert_eq!(
            parse_metadata(r#"<OMTSettings Quality="High" />"#),
            vec![Metadata::Settings {
                preview: None,
                quality: Some(Quality::High),
            }]
        );
    }

    #[test]
    fn parse_group_flattens_web_and_ptz() {
        let xml = group_xml(&[
            &web_xml("http://10.0.0.5/"),
            &PtzMetadata::visca_command_xml("22", "8101040700FF"),
        ]);
        let items = parse_metadata(&xml);
        assert_eq!(
            items,
            vec![
                Metadata::Web {
                    url: "http://10.0.0.5/".into(),
                },
                Metadata::Ptz(PtzMetadata {
                    protocol: "VISCA".into(),
                    url: None,
                    sequence: Some("22".into()),
                    command: Some("8101040700FF".into()),
                    reply: None,
                }),
            ]
        );
    }

    #[test]
    fn parse_ancillary_packet() {
        let xml = r#"<AncillaryData xmns="urn:anc:1.0">
<Packet did="45" sdid="01" line="21">
<Payload>81010A011E0000</Payload>
</Packet>
</AncillaryData>"#;
        let items = parse_metadata(xml);
        match &items[0] {
            Metadata::Ancillary(anc) => {
                assert_eq!(anc.xmlns.as_deref(), Some("urn:anc:1.0"));
                assert_eq!(anc.packets[0].attr("did"), Some("45"));
                assert_eq!(anc.packets[0].payload, "81010A011E0000");
            }
            other => panic!("expected ancillary, got {other:?}"),
        }
    }

    #[test]
    fn sender_info_roundtrip_attributes() {
        let info = SenderInfo::new("Studio", "Mikansei", "1.0");
        let parsed = SenderInfo::from_xml(&info.to_xml()).unwrap();
        assert_eq!(parsed, info);
    }
}
