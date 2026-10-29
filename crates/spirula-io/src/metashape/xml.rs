//! Minimal XML DOM for Metashape camera exports (elements, attrs, text).
//! Recursive `iter` / `findall` match C++ `XmlNode` usage.

use spirula_core::ErrorCode;

use crate::colmap::budget::err;

#[derive(Clone, Debug, Default)]
pub struct XmlNode {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<XmlNode>,
    pub text: String,
}

impl XmlNode {
    pub fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn find(&self, name: &str) -> Option<&XmlNode> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn findall(&self, name: &str) -> Vec<&XmlNode> {
        self.children.iter().filter(|c| c.name == name).collect()
    }

    /// Depth-first yield of elements named `name` (including nested).
    pub fn iter<'a>(&'a self, name: &'a str) -> Vec<&'a XmlNode> {
        let mut out = Vec::new();
        self.collect_named(name, &mut out);
        out
    }

    fn collect_named<'a>(&'a self, name: &str, out: &mut Vec<&'a XmlNode>) {
        for c in &self.children {
            if c.name == name {
                out.push(c);
            }
            c.collect_named(name, out);
        }
    }
}

fn decode_entities(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

/// Parse a document into a synthetic root whose `children` are top-level elements.
pub fn parse_xml_bytes(bytes: &[u8]) -> Result<XmlNode, spirula_core::DomainError> {
    let text = std::str::from_utf8(bytes).map_err(|_| {
        err(
            ErrorCode::MalformedRecord,
            "xml",
            "Metashape XML is not valid UTF-8",
        )
    })?;
    let mut i = 0usize;
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();

    fn skip_ws(chars: &[char], i: &mut usize) {
        while *i < chars.len() && chars[*i].is_whitespace() {
            *i += 1;
        }
    }

    fn parse_name(chars: &[char], i: &mut usize) -> Result<String, spirula_core::DomainError> {
        let start = *i;
        while *i < chars.len() {
            let c = chars[*i];
            if c.is_alphanumeric() || c == '_' || c == ':' || c == '-' || c == '.' {
                *i += 1;
            } else {
                break;
            }
        }
        if *i == start {
            return Err(err(
                ErrorCode::MalformedRecord,
                "xml",
                "Metashape XML: expected element name",
            ));
        }
        Ok(chars[start..*i].iter().collect())
    }

    fn parse_attr_value(
        chars: &[char],
        i: &mut usize,
    ) -> Result<String, spirula_core::DomainError> {
        skip_ws(chars, i);
        if *i >= chars.len() || (chars[*i] != '"' && chars[*i] != '\'') {
            return Err(err(
                ErrorCode::MalformedRecord,
                "xml",
                "Metashape XML: expected quoted attribute",
            ));
        }
        let quote = chars[*i];
        *i += 1;
        let start = *i;
        while *i < chars.len() && chars[*i] != quote {
            *i += 1;
        }
        if *i >= chars.len() {
            return Err(err(
                ErrorCode::MalformedRecord,
                "xml",
                "Metashape XML: unterminated attribute",
            ));
        }
        let raw: String = chars[start..*i].iter().collect();
        *i += 1;
        Ok(decode_entities(&raw))
    }

    fn parse_element(chars: &[char], i: &mut usize) -> Result<XmlNode, spirula_core::DomainError> {
        skip_ws(chars, i);
        if *i >= chars.len() || chars[*i] != '<' {
            return Err(err(
                ErrorCode::MalformedRecord,
                "xml",
                "Metashape XML: expected '<'",
            ));
        }
        *i += 1;
        // Skip declarations / comments / doctype.
        if *i < chars.len() && (chars[*i] == '?' || chars[*i] == '!') {
            while *i < chars.len() && chars[*i] != '>' {
                *i += 1;
            }
            if *i < chars.len() {
                *i += 1;
            }
            return parse_element(chars, i);
        }
        let name = parse_name(chars, i)?;
        let mut attrs = Vec::new();
        loop {
            skip_ws(chars, i);
            if *i >= chars.len() {
                return Err(err(
                    ErrorCode::MalformedRecord,
                    "xml",
                    "Metashape XML: truncated tag",
                ));
            }
            if chars[*i] == '/' {
                *i += 1;
                skip_ws(chars, i);
                if *i >= chars.len() || chars[*i] != '>' {
                    return Err(err(
                        ErrorCode::MalformedRecord,
                        "xml",
                        "Metashape XML: bad empty tag",
                    ));
                }
                *i += 1;
                return Ok(XmlNode {
                    name,
                    attrs,
                    children: Vec::new(),
                    text: String::new(),
                });
            }
            if chars[*i] == '>' {
                *i += 1;
                break;
            }
            let an = parse_name(chars, i)?;
            skip_ws(chars, i);
            if *i >= chars.len() || chars[*i] != '=' {
                return Err(err(
                    ErrorCode::MalformedRecord,
                    "xml",
                    "Metashape XML: expected '=' in attribute",
                ));
            }
            *i += 1;
            let av = parse_attr_value(chars, i)?;
            attrs.push((an, av));
        }

        let mut children = Vec::new();
        let mut text = String::new();
        loop {
            if *i >= chars.len() {
                return Err(err(
                    ErrorCode::MalformedRecord,
                    "xml",
                    format!("Metashape XML: unclosed <{name}>"),
                ));
            }
            if chars[*i] == '<' {
                if *i + 1 < chars.len() && chars[*i + 1] == '/' {
                    *i += 2;
                    let end_name = parse_name(chars, i)?;
                    if end_name != name {
                        return Err(err(
                            ErrorCode::MalformedRecord,
                            "xml",
                            format!("Metashape XML: mismatched </{end_name}> for <{name}>"),
                        ));
                    }
                    skip_ws(chars, i);
                    if *i >= chars.len() || chars[*i] != '>' {
                        return Err(err(
                            ErrorCode::MalformedRecord,
                            "xml",
                            "Metashape XML: bad close tag",
                        ));
                    }
                    *i += 1;
                    break;
                }
                // Child element.
                let child = parse_element(chars, i)?;
                children.push(child);
            } else {
                let start = *i;
                while *i < chars.len() && chars[*i] != '<' {
                    *i += 1;
                }
                let chunk: String = chars[start..*i].iter().collect();
                text.push_str(&decode_entities(&chunk));
            }
        }
        Ok(XmlNode {
            name,
            attrs,
            children,
            text,
        })
    }

    skip_ws(&chars, &mut i);
    // Allow leading declarations by looping parse_element which skips them.
    let mut root = XmlNode {
        name: "#document".into(),
        ..Default::default()
    };
    while i < n {
        skip_ws(&chars, &mut i);
        if i >= n {
            break;
        }
        if chars[i] != '<' {
            return Err(err(
                ErrorCode::MalformedRecord,
                "xml",
                "Metashape XML: junk outside elements",
            ));
        }
        // Peek for end — shouldn't happen at top level.
        if i + 1 < n && chars[i + 1] == '/' {
            break;
        }
        // Declaration / comment at top: parse_element skips and continues.
        if i + 1 < n && (chars[i + 1] == '?' || chars[i + 1] == '!') {
            i += 1;
            while i < n && chars[i] != '>' {
                i += 1;
            }
            if i < n {
                i += 1;
            }
            continue;
        }
        root.children.push(parse_element(&chars, &mut i)?);
    }
    if root.children.is_empty() {
        return Err(err(
            ErrorCode::MalformedRecord,
            "xml",
            "Metashape XML: empty document",
        ));
    }
    Ok(root)
}
