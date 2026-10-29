//! Minimal XML DOM for Metashape camera exports (elements, attrs, text).
//! Recursive `iter` / `findall` match C++ `XmlNode` usage.

use spirula_core::{CancelToken, ErrorCode};

use crate::colmap::budget::{ParseCancel, ParseMem, err};

/// Nesting depth cap for Metashape camera-export XML.
pub const MAX_XML_DEPTH: u32 = 64;

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

struct ParseCtx<'a> {
    chars: &'a [char],
    mem: &'a mut ParseMem,
    cp: ParseCancel<'a>,
}

impl ParseCtx<'_> {
    fn charge_str(&mut self, s: &str, field: &str) -> Result<(), spirula_core::DomainError> {
        self.mem.charge_str_bytes(s.len(), field)
    }

    fn charge_node(&mut self) -> Result<(), spirula_core::DomainError> {
        self.mem.charge_elems::<XmlNode>(1, "xml_dom")
    }
}

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

fn parse_attr_value(chars: &[char], i: &mut usize) -> Result<String, spirula_core::DomainError> {
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

fn parse_element(
    ctx: &mut ParseCtx<'_>,
    i: &mut usize,
    depth: u32,
) -> Result<XmlNode, spirula_core::DomainError> {
    if depth > MAX_XML_DEPTH {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "xml_depth",
            format!("Metashape XML: nesting exceeds depth {MAX_XML_DEPTH}"),
        ));
    }
    ctx.cp.tick(1)?;
    let chars = ctx.chars;
    skip_ws(chars, i);
    if *i >= chars.len() || chars[*i] != '<' {
        return Err(err(
            ErrorCode::MalformedRecord,
            "xml",
            "Metashape XML: expected '<'",
        ));
    }
    *i += 1;
    if *i < chars.len() && (chars[*i] == '?' || chars[*i] == '!') {
        while *i < chars.len() && chars[*i] != '>' {
            *i += 1;
        }
        if *i < chars.len() {
            *i += 1;
        }
        return parse_element(ctx, i, depth);
    }
    let name = parse_name(chars, i)?;
    ctx.charge_str(&name, "xml_name")?;
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
            ctx.charge_node()?;
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
        ctx.charge_str(&an, "xml_attr")?;
        ctx.charge_str(&av, "xml_attr")?;
        attrs.push((an, av));
    }

    let mut children = Vec::new();
    let mut text = String::new();
    loop {
        ctx.cp.tick(1)?;
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
            let child = parse_element(ctx, i, depth + 1)?;
            children.push(child);
        } else {
            let start = *i;
            while *i < chars.len() && chars[*i] != '<' {
                *i += 1;
            }
            let chunk: String = chars[start..*i].iter().collect();
            let decoded = decode_entities(&chunk);
            ctx.charge_str(&decoded, "xml_text")?;
            text.push_str(&decoded);
        }
    }
    ctx.charge_node()?;
    Ok(XmlNode {
        name,
        attrs,
        children,
        text,
    })
}

/// Parse a document into a synthetic root whose `children` are top-level elements.
pub fn parse_xml_bytes(
    bytes: &[u8],
    mem: &mut ParseMem,
    cancel: Option<&CancelToken>,
) -> Result<XmlNode, spirula_core::DomainError> {
    let text = std::str::from_utf8(bytes).map_err(|_| {
        err(
            ErrorCode::MalformedRecord,
            "xml",
            "Metashape XML is not valid UTF-8",
        )
    })?;
    let chars: Vec<char> = text.chars().collect();
    // Charge the char buffer used for parsing (separate from file-byte charge).
    mem.charge_elems::<char>(chars.len(), "xml_chars")?;
    let n = chars.len();
    let mut i = 0usize;
    let mut ctx = ParseCtx {
        chars: &chars,
        mem,
        cp: ParseCancel::new(cancel),
    };
    ctx.cp.poll_now()?;

    skip_ws(&chars, &mut i);
    let mut root = XmlNode {
        name: "#document".into(),
        ..Default::default()
    };
    ctx.charge_node()?;
    while i < n {
        ctx.cp.tick(1)?;
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
        if i + 1 < n && chars[i + 1] == '/' {
            break;
        }
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
        root.children.push(parse_element(&mut ctx, &mut i, 1)?);
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
