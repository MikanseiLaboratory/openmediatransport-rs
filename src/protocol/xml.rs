//! XML element parser for OMT metadata, backed by `roxmltree`.
//!
//! Control tally from libomtnet is **not** parsed here: those constants use
//! invalid `Program==` and are matched as exact strings in [`super::metadata`].

use crate::error::OmtError;

/// One XML element with attributes, child elements, and concatenated text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlElement {
    /// Tag name (`OMTTally`, `OMTPTZ`, …).
    pub name: String,
    /// Attributes in document order.
    pub attributes: Vec<(String, String)>,
    /// Direct child elements.
    pub children: Vec<XmlElement>,
    /// Concatenated character data (trimmed).
    pub text: String,
}

impl XmlElement {
    /// Parse well-formed `xml` into a single root element.
    pub fn parse(xml: &str) -> Result<Self, OmtError> {
        let xml = xml.trim_start_matches('\u{feff}');
        let doc = roxmltree::Document::parse(xml)
            .map_err(|e| OmtError::Protocol(format!("invalid metadata XML: {e}")))?;
        Ok(from_node(doc.root_element()))
    }

    /// Attribute value for `name`, or `None` if missing.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// Parse an attribute as a boolean (`true`/`1` vs `false`/`0`).
    pub fn attr_bool(&self, name: &str) -> Option<bool> {
        parse_bool(self.attr(name)?)
    }

    /// First direct child named `name`.
    pub fn child(&self, name: &str) -> Option<&XmlElement> {
        self.children.iter().find(|c| c.name == name)
    }

    /// All direct children named `name`.
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a XmlElement> {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// Text of the first child named `name`, if present.
    pub fn child_text(&self, name: &str) -> Option<&str> {
        let c = self.child(name)?;
        if c.text.is_empty() {
            None
        } else {
            Some(c.text.as_str())
        }
    }
}

/// Parse a boolean attribute/text value.
pub fn parse_bool(value: &str) -> Option<bool> {
    match value.trim() {
        s if s.eq_ignore_ascii_case("true") || s == "1" => Some(true),
        s if s.eq_ignore_ascii_case("false") || s == "0" => Some(false),
        _ => None,
    }
}

/// Escape text or attribute values for XML output.
pub fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn from_node(node: roxmltree::Node<'_, '_>) -> XmlElement {
    let attributes = node
        .attributes()
        .map(|a| (a.name().to_string(), a.value().to_string()))
        .collect();
    let mut children = Vec::new();
    let mut text = String::new();
    for child in node.children() {
        if child.is_element() {
            children.push(from_node(child));
        } else if child.is_text() {
            text.push_str(child.text().unwrap_or(""));
        }
    }
    XmlElement {
        name: node.tag_name().name().to_string(),
        attributes,
        children,
        text: text.trim().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_libomtnet_tally_quirk() {
        assert!(XmlElement::parse(r#"<OMTTally Preview="true" Program=="false" />"#).is_err());
    }

    #[test]
    fn parses_well_formed_tally() {
        let el = XmlElement::parse(r#"<OMTTally Preview="false" Program="true" />"#).unwrap();
        assert_eq!(el.attr_bool("Program"), Some(true));
    }

    #[test]
    fn parses_nested_children_and_text() {
        let el = XmlElement::parse(
            r#"<OMTAddress>
  <Name>HOST (Cam)</Name>
  <Port>6400</Port>
  <Addresses>
    <IPAddress>10.0.0.1</IPAddress>
    <IPAddress>127.0.0.1</IPAddress>
  </Addresses>
</OMTAddress>"#,
        )
        .unwrap();
        assert_eq!(el.child_text("Name"), Some("HOST (Cam)"));
        assert_eq!(el.child_text("Port"), Some("6400"));
        let ips: Vec<_> = el
            .child("Addresses")
            .unwrap()
            .children_named("IPAddress")
            .map(|c| c.text.as_str())
            .collect();
        assert_eq!(ips, vec!["10.0.0.1", "127.0.0.1"]);
    }

    #[test]
    fn parses_group_children() {
        let el = XmlElement::parse(
            r#"<OMTGroup>
  <OMTWeb URL="http://10.0.0.5/" />
  <OMTPTZ Protocol="VISCA" Sequence="22" Command="8101040700FF" />
</OMTGroup>"#,
        )
        .unwrap();
        assert_eq!(el.children.len(), 2);
        assert_eq!(
            el.child("OMTWeb").unwrap().attr("URL"),
            Some("http://10.0.0.5/")
        );
        assert_eq!(el.child("OMTPTZ").unwrap().attr("Protocol"), Some("VISCA"));
    }

    #[test]
    fn rejects_malformed_xml() {
        assert!(XmlElement::parse("<OMTTally Preview=").is_err());
    }
}
