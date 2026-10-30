//! Minimal XML DOM for Metashape camera exports (elements, attrs, text).
//! Recursive `iter` / `findall` match C++ `XmlNode` usage.
//!
//! Memory: every `String` / `Vec` grow is charged **before** allocate. Long
//! attribute / text scans tick cancel. Comments (`<!-- … -->`) and PIs are
//! skipped as sibling markup (including trailing comments under a parent).

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

fn decode_named_entity(name: &str) -> Option<char> {
    match name {
        "lt" => Some('<'),
        "gt" => Some('>'),
        "amp" => Some('&'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => None,
    }
}

fn decode_numeric_entity(body: &str) -> Option<char> {
    let (radix, digits) =
        if let Some(hex) = body.strip_prefix('x').or_else(|| body.strip_prefix('X')) {
            (16, hex)
        } else {
            (10, body)
        };
    if digits.is_empty() {
        return None;
    }
    let code = u32::from_str_radix(digits, radix).ok()?;
    char::from_u32(code)
}

/// Decode `&lt;` / `&amp;` / `&#NNN;` / `&#xHH;` entities. Output length is
/// always ≤ input length for the entity forms we accept.
fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let Some(semi) = rest.find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..semi];
        let decoded = if let Some(num) = ent.strip_prefix('#') {
            decode_numeric_entity(num)
        } else {
            decode_named_entity(ent)
        };
        if let Some(ch) = decoded {
            out.push(ch);
            rest = &rest[semi + 1..];
        } else {
            out.push('&');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    out
}

struct ParseCtx<'a> {
    chars: &'a [char],
    mem: &'a mut ParseMem,
    cp: ParseCancel<'a>,
}

impl ParseCtx<'_> {
    fn charge_str_bytes(
        &mut self,
        len: usize,
        field: &str,
    ) -> Result<(), spirula_core::DomainError> {
        self.mem.charge_str_bytes(len, field)
    }

    fn charge_node(&mut self) -> Result<(), spirula_core::DomainError> {
        self.mem.charge_elems::<XmlNode>(1, "xml_dom")
    }

    fn collect_str(
        &mut self,
        slice: &[char],
        field: &str,
    ) -> Result<String, spirula_core::DomainError> {
        let nbytes: usize = slice.iter().map(|c| c.len_utf8()).sum();
        self.charge_str_bytes(nbytes, field)?;
        Ok(slice.iter().collect())
    }
}

fn skip_ws(chars: &[char], i: &mut usize) {
    while *i < chars.len() && chars[*i].is_whitespace() {
        *i += 1;
    }
}

/// Skip `<? … ?>`, `<!-- … -->`, or other `<! … >` markup. Ticks cancel on long
/// scans. Caller has already consumed `'<'`; `chars[*i]` is `'?'` or `'!'`.
fn skip_pi_or_comment(
    ctx: &mut ParseCtx<'_>,
    i: &mut usize,
) -> Result<(), spirula_core::DomainError> {
    let chars = ctx.chars;
    if *i >= chars.len() {
        return Err(err(
            ErrorCode::MalformedRecord,
            "xml",
            "Metashape XML: truncated markup",
        ));
    }
    if chars[*i] == '?' {
        *i += 1;
        while *i + 1 < chars.len() {
            ctx.cp.tick(1)?;
            if chars[*i] == '?' && chars[*i + 1] == '>' {
                *i += 2;
                return Ok(());
            }
            *i += 1;
        }
        return Err(err(
            ErrorCode::MalformedRecord,
            "xml",
            "Metashape XML: unterminated processing instruction",
        ));
    }
    // '!'
    *i += 1;
    if *i + 1 < chars.len() && chars[*i] == '-' && chars[*i + 1] == '-' {
        *i += 2;
        while *i + 2 < chars.len() {
            ctx.cp.tick(1)?;
            if chars[*i] == '-' && chars[*i + 1] == '-' && chars[*i + 2] == '>' {
                *i += 3;
                return Ok(());
            }
            *i += 1;
        }
        return Err(err(
            ErrorCode::MalformedRecord,
            "xml",
            "Metashape XML: unterminated comment",
        ));
    }
    while *i < chars.len() {
        ctx.cp.tick(1)?;
        if chars[*i] == '>' {
            *i += 1;
            return Ok(());
        }
        *i += 1;
    }
    Err(err(
        ErrorCode::MalformedRecord,
        "xml",
        "Metashape XML: unterminated declaration",
    ))
}

fn parse_name(ctx: &mut ParseCtx<'_>, i: &mut usize) -> Result<String, spirula_core::DomainError> {
    let chars = ctx.chars;
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
    ctx.collect_str(&chars[start..*i], "xml_name")
}

fn parse_attr_value(
    ctx: &mut ParseCtx<'_>,
    i: &mut usize,
) -> Result<String, spirula_core::DomainError> {
    let chars = ctx.chars;
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
        ctx.cp.tick(1)?;
        *i += 1;
    }
    if *i >= chars.len() {
        return Err(err(
            ErrorCode::MalformedRecord,
            "xml",
            "Metashape XML: unterminated attribute",
        ));
    }
    let raw = ctx.collect_str(&chars[start..*i], "xml_attr")?;
    *i += 1;
    // Decoded form is ≤ raw UTF-8 length for supported entities; charge that
    // upper bound before allocating the decoded String.
    ctx.charge_str_bytes(raw.len(), "xml_attr")?;
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
        skip_pi_or_comment(ctx, i)?;
        return parse_element(ctx, i, depth);
    }
    let name = parse_name(ctx, i)?;
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
        let an = parse_name(ctx, i)?;
        skip_ws(chars, i);
        if *i >= chars.len() || chars[*i] != '=' {
            return Err(err(
                ErrorCode::MalformedRecord,
                "xml",
                "Metashape XML: expected '=' in attribute",
            ));
        }
        *i += 1;
        let av = parse_attr_value(ctx, i)?;
        ctx.mem.charge_elems::<(String, String)>(1, "xml_attr")?;
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
                let end_name = parse_name(ctx, i)?;
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
            if *i + 1 < chars.len() && (chars[*i + 1] == '?' || chars[*i + 1] == '!') {
                *i += 1;
                skip_pi_or_comment(ctx, i)?;
                continue;
            }
            let child = parse_element(ctx, i, depth + 1)?;
            children.push(child);
        } else {
            let start = *i;
            while *i < chars.len() && chars[*i] != '<' {
                ctx.cp.tick(1)?;
                *i += 1;
            }
            let chunk = ctx.collect_str(&chars[start..*i], "xml_text")?;
            // Decoded temp + text push_str growth: each ≤ chunk.len(); charge both
            // before either allocation/grow.
            ctx.charge_str_bytes(chunk.len().saturating_mul(2), "xml_text")?;
            text.push_str(&decode_entities(&chunk));
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
    // Count chars without allocating, then charge + reserve before Vec<char> grow.
    let n_chars = text.chars().count();
    mem.charge_elems::<char>(n_chars, "xml_chars")?;
    let mut chars = Vec::new();
    chars.try_reserve(n_chars).map_err(|_| {
        err(
            ErrorCode::ValueOutOfRange,
            "xml_chars",
            format!("xml_chars reservation of {n_chars} elements failed"),
        )
    })?;
    chars.extend(text.chars());
    let n = chars.len();
    let mut i = 0usize;
    let mut ctx = ParseCtx {
        chars: &chars,
        mem,
        cp: ParseCancel::new(cancel),
    };
    ctx.cp.poll_now()?;

    skip_ws(&chars, &mut i);
    ctx.charge_str_bytes("#document".len(), "xml_name")?;
    ctx.charge_node()?;
    let mut root = XmlNode {
        name: "#document".into(),
        ..Default::default()
    };
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
            skip_pi_or_comment(&mut ctx, &mut i)?;
            continue;
        }
        let child = parse_element(&mut ctx, &mut i, 1)?;
        root.children.push(child);
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

#[cfg(test)]
mod tests {
    use super::*;
    use spirula_core::{CancelReason, CancelToken, ErrorCode};

    #[test]
    fn trailing_comment_and_numeric_entities() {
        let mut mem = ParseMem::with_cap(1 << 20);
        let xml = br#"<?xml version="1.0"?>
<root label="A&#65;&#x42;&amp;"><child>&#66;</child><!-- trailing --></root>
"#;
        let doc = parse_xml_bytes(xml, &mut mem, None).expect("parse");
        let root = doc.find("root").expect("root");
        assert_eq!(root.attr("label"), Some("AAB&"));
        assert_eq!(root.find("child").map(|c| c.text.as_str()), Some("B"));
        assert_eq!(root.children.len(), 1);
    }

    #[test]
    fn xml_chars_budget_rejects_before_vec_char() {
        let xml = br#"<document><chunk/></document>"#;
        // Cap below char-buffer charge (file not pre-charged here).
        let mut mem = ParseMem::with_cap(8);
        let e = parse_xml_bytes(xml, &mut mem, None).unwrap_err();
        assert_eq!(e.code(), ErrorCode::ValueOutOfRange);
        assert_eq!(
            e.context_value(spirula_core::ContextKey::Field),
            Some("xml_chars")
        );
    }

    #[test]
    fn after_start_cancel_during_long_attribute() {
        let mut body = String::from("<root label=\"");
        body.extend(std::iter::repeat_n('x', 300_000));
        body.push_str("\"/>");
        let cancel = CancelToken::new();
        let c2 = cancel.clone();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(2));
            assert!(c2.request(CancelReason::User));
        });
        let mut mem = ParseMem::with_cap(1 << 30);
        let e = parse_xml_bytes(body.as_bytes(), &mut mem, Some(&cancel)).unwrap_err();
        handle.join().unwrap();
        assert_eq!(e.code(), ErrorCode::Canceled);
    }
}
