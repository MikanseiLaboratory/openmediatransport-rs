//! Tolerant XML element parser for OMT metadata.
//!
//! OMT control strings are not always well-formed XML (libomtnet tally uses
//! `Program==` instead of `Program=`). This parser extracts element names,
//! attributes, children, and text so callers can look up keys instead of
//! matching entire strings.

use crate::error::OmtError;

/// One XML element with attributes, child elements, and concatenated text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlElement {
    /// Tag name (`OMTTally`, `OMTPTZ`, …).
    pub name: String,
    /// Attributes in document order. Names are stored without extra `=`.
    pub attributes: Vec<(String, String)>,
    /// Direct child elements.
    pub children: Vec<XmlElement>,
    /// Concatenated character data (trimmed).
    pub text: String,
}

impl XmlElement {
    /// Parse `xml` into a single root element.
    ///
    /// Leading BOM, XML declarations, and comments are skipped. `Program=="x"`
    /// is accepted as attribute `Program` = `x`.
    pub fn parse(xml: &str) -> Result<Self, OmtError> {
        let rest = skip_prolog(xml);
        let (el, rest) = parse_element(rest)?;
        let rest = skip_prolog(rest).trim();
        if !rest.is_empty() {
            return Err(OmtError::Protocol(format!(
                "trailing XML after <{}>: {rest}",
                el.name
            )));
        }
        Ok(el)
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

fn skip_prolog(s: &str) -> &str {
    let mut rest = s.trim_start_matches('\u{feff}');
    loop {
        rest = rest.trim_start();
        if rest.starts_with("<?") {
            match rest.find("?>") {
                Some(end) => rest = &rest[end + 2..],
                None => break,
            }
            continue;
        }
        if rest.starts_with("<!--") {
            match rest.find("-->") {
                Some(end) => rest = &rest[end + 3..],
                None => break,
            }
            continue;
        }
        break;
    }
    rest
}

fn parse_element(input: &str) -> Result<(XmlElement, &str), OmtError> {
    let rest = skip_prolog(input);
    let rest = rest
        .strip_prefix('<')
        .ok_or_else(|| OmtError::Protocol("expected '<' to start XML element".into()))?;
    if rest.starts_with('/') {
        return Err(OmtError::Protocol("unexpected closing tag".into()));
    }
    let (name, rest) = parse_name(rest)?;
    if name.is_empty() {
        return Err(OmtError::Protocol("empty XML element name".into()));
    }
    let (attributes, rest) = parse_attributes(rest)?;
    let rest = rest.trim_start();
    if let Some(rest) = rest.strip_prefix("/>") {
        return Ok((
            XmlElement {
                name,
                attributes,
                children: Vec::new(),
                text: String::new(),
            },
            rest,
        ));
    }
    let rest = rest
        .strip_prefix('>')
        .ok_or_else(|| OmtError::Protocol(format!("expected '>' after <{name}")))?;
    let (children, text, rest) = parse_content(&name, rest)?;
    Ok((
        XmlElement {
            name,
            attributes,
            children,
            text,
        },
        rest,
    ))
}

fn parse_content<'a>(
    name: &str,
    mut rest: &'a str,
) -> Result<(Vec<XmlElement>, String, &'a str), OmtError> {
    let mut children = Vec::new();
    let mut text = String::new();
    loop {
        if rest.is_empty() {
            return Err(OmtError::Protocol(format!("unclosed <{name}>")));
        }
        if rest.starts_with("<!--") {
            rest = match rest.find("-->") {
                Some(end) => &rest[end + 3..],
                None => return Err(OmtError::Protocol("unclosed XML comment".into())),
            };
            continue;
        }
        if let Some(after) = rest.strip_prefix("</") {
            let (end_name, after) = parse_name(after)?;
            if end_name != name {
                return Err(OmtError::Protocol(format!(
                    "mismatched close: expected </{name}>, got </{end_name}>"
                )));
            }
            let after = after.trim_start();
            let after = after
                .strip_prefix('>')
                .ok_or_else(|| OmtError::Protocol(format!("expected '>' after </{name}")))?;
            return Ok((children, text.trim().to_string(), after));
        }
        if rest.starts_with('<') {
            let (child, after) = parse_element(rest)?;
            children.push(child);
            rest = after;
            continue;
        }
        match rest.find('<') {
            Some(pos) => {
                text.push_str(&unescape_xml(&rest[..pos]));
                rest = &rest[pos..];
            }
            None => {
                return Err(OmtError::Protocol(format!("unclosed <{name}>")));
            }
        }
    }
}

type AttrList<'a> = Result<(Vec<(String, String)>, &'a str), OmtError>;

fn parse_attributes(mut rest: &str) -> AttrList<'_> {
    let mut attrs = Vec::new();
    loop {
        rest = rest.trim_start();
        if rest.is_empty() || rest.starts_with('/') || rest.starts_with('>') {
            return Ok((attrs, rest));
        }
        let (name, after) = parse_name(rest)?;
        rest = after.trim_start();
        let mut eq = 0usize;
        while let Some(stripped) = rest.strip_prefix('=') {
            eq += 1;
            rest = stripped;
        }
        if eq == 0 {
            return Err(OmtError::Protocol(format!(
                "attribute {name} is missing '='"
            )));
        }
        rest = rest.trim_start();
        let (value, after) = parse_quoted(rest)?;
        attrs.push((name, value));
        rest = after;
    }
}

fn parse_name(s: &str) -> Result<(String, &str), OmtError> {
    let mut chars = s.char_indices();
    let Some((_, first)) = chars.next() else {
        return Err(OmtError::Protocol("expected XML name".into()));
    };
    if !is_name_start(first) {
        return Err(OmtError::Protocol(format!(
            "invalid XML name start {first:?}"
        )));
    }
    let mut end = first.len_utf8();
    for (i, c) in chars {
        if is_name_char(c) {
            end = i + c.len_utf8();
        } else {
            break;
        }
    }
    Ok((s[..end].to_string(), &s[end..]))
}

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c == ':'
}

fn is_name_char(c: char) -> bool {
    is_name_start(c) || c.is_ascii_digit() || c == '-' || c == '.'
}

fn parse_quoted(s: &str) -> Result<(String, &str), OmtError> {
    let quote = s
        .chars()
        .next()
        .filter(|c| *c == '"' || *c == '\'')
        .ok_or_else(|| OmtError::Protocol("expected quoted attribute value".into()))?;
    let inner = &s[quote.len_utf8()..];
    let end = inner
        .find(quote)
        .ok_or_else(|| OmtError::Protocol("unclosed attribute quotes".into()))?;
    Ok((
        unescape_xml(&inner[..end]),
        &inner[end + quote.len_utf8()..],
    ))
}

fn unescape_xml(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Escape text or attribute values for XML output.
pub fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tally_with_double_equals_quirk() {
        let el = XmlElement::parse(r#"<OMTTally Preview="true" Program=="false" />"#).unwrap();
        assert_eq!(el.name, "OMTTally");
        assert_eq!(el.attr("Preview"), Some("true"));
        assert_eq!(el.attr("Program"), Some("false"));
        assert_eq!(el.attr_bool("Preview"), Some(true));
        assert_eq!(el.attr_bool("Program"), Some(false));
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
}
