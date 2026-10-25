//! JSON documents for `config.json`, including Python `Infinity` / `NaN`.
//!
//! Objects keep insertion order. Each value remembers its source text so an
//! unknown nested value can be written back unchanged.

use crate::{ContextKey, DomainError, ErrorCode};

/// Parsed JSON value.
#[derive(Clone, Debug)]
pub struct JsonValue {
    pub kind: JsonKind,
    raw: String,
}

/// JSON value kinds. Numbers retain the parsed numeric value.
#[derive(Clone, Debug)]
pub enum JsonKind {
    Null,
    Bool(bool),
    Number(JsonNumber),
    String(String),
    Array(Vec<JsonValue>),
    Object(Vec<(String, JsonValue)>),
}

/// Numeric value. Non-finite tokens match the reference parser.
#[derive(Clone, Copy, Debug)]
pub enum JsonNumber {
    Finite(f64),
    Infinity,
    NegInfinity,
    NaN,
}

impl JsonValue {
    pub fn raw(&self) -> &str {
        &self.raw
    }

    pub(crate) fn actual(&self) -> &'static str {
        match self.kind {
            JsonKind::Null => "null",
            JsonKind::Bool(_) => "bool",
            JsonKind::Number(_) => "number",
            JsonKind::String(_) => "string",
            JsonKind::Array(_) => "array",
            JsonKind::Object(_) => "object",
        }
    }
}

/// Parse one JSON value. A leading UTF-8 BOM is ignored.
pub fn parse_json(text: &str) -> Result<JsonValue, DomainError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut parser = Parser { text, index: 0 };
    parser.skip_ws()?;
    if parser.done() {
        return Err(parse_error(0, "unexpected end of input"));
    }
    let value = parser.parse_value(0)?;
    parser.skip_ws()?;
    if !parser.done() {
        return Err(parse_error(parser.index, "trailing content"));
    }
    Ok(value)
}

pub(crate) fn parse_error(offset: usize, why: &str) -> DomainError {
    domain(
        ErrorCode::MalformedRecord,
        format!("config JSON parse error at byte {offset}: {why}"),
    )
    .with_context(ContextKey::Format, "config.json")
    .expect("format")
}

pub(crate) fn shape_error(field: &str, expected: &str, actual: &str) -> DomainError {
    domain(
        ErrorCode::MalformedRecord,
        format!("config field {field} must be {expected}, got {actual}"),
    )
    .with_context(ContextKey::Field, field)
    .expect("field")
    .with_context(ContextKey::Expected, expected)
    .expect("expected")
    .with_context(ContextKey::Actual, actual)
    .expect("actual")
    .with_context(ContextKey::Format, "config.json")
    .expect("format")
}

pub(crate) fn range_error(field: &str, actual: &str) -> DomainError {
    domain(
        ErrorCode::ValueOutOfRange,
        format!("config field {field} is outside i32"),
    )
    .with_context(ContextKey::Field, field)
    .expect("field")
    .with_context(ContextKey::Expected, "i32")
    .expect("expected")
    .with_context(ContextKey::Actual, actual)
    .expect("actual")
    .with_context(ContextKey::Format, "config.json")
    .expect("format")
}

pub(crate) fn choice_error(field: &str, actual: &str, choices: &str) -> DomainError {
    domain(
        ErrorCode::UnsupportedVersion,
        format!("config field {field} has unknown value {actual}"),
    )
    .with_context(ContextKey::Field, field)
    .expect("field")
    .with_context(ContextKey::Expected, choices)
    .expect("expected")
    .with_context(ContextKey::Actual, actual)
    .expect("actual")
    .with_context(ContextKey::Format, "config.json")
    .expect("format")
}

pub(crate) fn version_error(actual: &str) -> DomainError {
    domain(
        ErrorCode::UnsupportedVersion,
        format!("unsupported config schema_version {actual}"),
    )
    .with_context(ContextKey::Field, "schema_version")
    .expect("field")
    .with_context(ContextKey::Expected, "1")
    .expect("expected")
    .with_context(ContextKey::Actual, actual)
    .expect("actual")
    .with_context(ContextKey::Format, "config.json")
    .expect("format")
}

fn domain(code: ErrorCode, message: String) -> DomainError {
    DomainError::try_new(code, message).expect("config error message is not empty")
}

pub(crate) fn quote_json(text: &str) -> String {
    let mut out = String::from("\"");
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if control.is_control() => {
                out.push_str(&format!("\\u{value:04x}", value = u32::from(control)));
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

pub(crate) fn format_f32(value: f32) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value.is_sign_positive() {
            "Infinity".to_owned()
        } else {
            "-Infinity".to_owned()
        };
    }
    format!("{value}")
}

struct Parser<'a> {
    text: &'a str,
    index: usize,
}

impl<'a> Parser<'a> {
    fn done(&self) -> bool {
        self.index >= self.text.len()
    }

    fn peek(&self) -> Result<u8, DomainError> {
        self.text
            .as_bytes()
            .get(self.index)
            .copied()
            .ok_or_else(|| parse_error(self.index, "unexpected end of input"))
    }

    fn bump(&mut self) -> Result<u8, DomainError> {
        let byte = self.peek()?;
        self.index += 1;
        Ok(byte)
    }

    fn skip_ws(&mut self) -> Result<(), DomainError> {
        while !self.done() {
            match self.peek()? {
                b' ' | b'\t' | b'\n' | b'\r' => self.index += 1,
                _ => break,
            }
        }
        Ok(())
    }

    fn parse_value(&mut self, depth: usize) -> Result<JsonValue, DomainError> {
        if depth > 256 {
            return Err(parse_error(self.index, "nesting too deep"));
        }
        self.skip_ws()?;
        let start = self.index;
        let byte = self.peek()?;
        let kind = match byte {
            b'{' => self.parse_object(depth)?,
            b'[' => self.parse_array(depth)?,
            b'"' => JsonKind::String(self.parse_string()?),
            b't' => {
                self.consume("true")?;
                JsonKind::Bool(true)
            }
            b'f' => {
                self.consume("false")?;
                JsonKind::Bool(false)
            }
            b'n' if self.starts_with("null") => {
                self.consume("null")?;
                JsonKind::Null
            }
            b'I' => {
                self.consume("Infinity")?;
                self.reject_ident_tail()?;
                JsonKind::Number(JsonNumber::Infinity)
            }
            b'N' => {
                self.consume("NaN")?;
                self.reject_ident_tail()?;
                JsonKind::Number(JsonNumber::NaN)
            }
            b'-' if self.starts_with("-Infinity") => {
                self.consume("-Infinity")?;
                self.reject_ident_tail()?;
                JsonKind::Number(JsonNumber::NegInfinity)
            }
            b'-' | b'0'..=b'9' => JsonKind::Number(self.parse_number()?),
            _ => return Err(parse_error(self.index, "unexpected character")),
        };
        Ok(JsonValue {
            kind,
            raw: self.text[start..self.index].to_owned(),
        })
    }

    fn parse_object(&mut self, depth: usize) -> Result<JsonKind, DomainError> {
        self.bump()?;
        let mut fields = Vec::new();
        self.skip_ws()?;
        if self.peek()? == b'}' {
            self.bump()?;
            return Ok(JsonKind::Object(fields));
        }
        loop {
            self.skip_ws()?;
            if self.peek()? != b'"' {
                return Err(parse_error(self.index, "expected string"));
            }
            let key = self.parse_string()?;
            self.skip_ws()?;
            if self.bump()? != b':' {
                return Err(parse_error(self.index - 1, "expected ':'"));
            }
            fields.push((key, self.parse_value(depth + 1)?));
            self.skip_ws()?;
            match self.bump()? {
                b',' => continue,
                b'}' => return Ok(JsonKind::Object(fields)),
                _ => return Err(parse_error(self.index - 1, "expected ',' or '}'")),
            }
        }
    }

    fn parse_array(&mut self, depth: usize) -> Result<JsonKind, DomainError> {
        self.bump()?;
        let mut items = Vec::new();
        self.skip_ws()?;
        if self.peek()? == b']' {
            self.bump()?;
            return Ok(JsonKind::Array(items));
        }
        loop {
            items.push(self.parse_value(depth + 1)?);
            self.skip_ws()?;
            match self.bump()? {
                b',' => continue,
                b']' => return Ok(JsonKind::Array(items)),
                _ => return Err(parse_error(self.index - 1, "expected ',' or ']'")),
            }
        }
    }

    fn parse_string(&mut self) -> Result<String, DomainError> {
        if self.bump()? != b'"' {
            return Err(parse_error(self.index - 1, "expected string"));
        }
        let mut out = Vec::new();
        loop {
            if self.done() {
                return Err(parse_error(self.index, "unterminated string"));
            }
            let byte = self.bump()?;
            if byte == b'"' {
                return String::from_utf8(out)
                    .map_err(|_| parse_error(self.index, "invalid UTF-8 in string"));
            }
            if byte != b'\\' {
                out.push(byte);
                continue;
            }
            if self.done() {
                return Err(parse_error(self.index, "truncated escape"));
            }
            match self.bump()? {
                b'"' => out.push(b'"'),
                b'\\' => out.push(b'\\'),
                b'/' => out.push(b'/'),
                b'b' => out.push(0x08),
                b'f' => out.push(0x0c),
                b'n' => out.push(b'\n'),
                b'r' => out.push(b'\r'),
                b't' => out.push(b'\t'),
                b'u' => {
                    let mut code = self.parse_hex4()?;
                    if (0xD800..=0xDBFF).contains(&code) {
                        if !self.starts_with("\\u") {
                            return Err(parse_error(self.index, "unpaired surrogate"));
                        }
                        self.index += 2;
                        let low = self.parse_hex4()?;
                        if !(0xDC00..=0xDFFF).contains(&low) {
                            return Err(parse_error(self.index, "bad low surrogate"));
                        }
                        code = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                    }
                    let character = char::from_u32(code)
                        .ok_or_else(|| parse_error(self.index, "invalid unicode scalar"))?;
                    let mut buf = [0; 4];
                    out.extend_from_slice(character.encode_utf8(&mut buf).as_bytes());
                }
                _ => return Err(parse_error(self.index - 1, "unknown escape")),
            }
        }
    }

    fn parse_hex4(&mut self) -> Result<u32, DomainError> {
        if self.text.len() - self.index < 4 {
            return Err(parse_error(self.index, "truncated \\u escape"));
        }
        let mut value = 0u32;
        for _ in 0..4 {
            let byte = self.bump()?;
            value <<= 4;
            value |= match byte {
                b'0'..=b'9' => u32::from(byte - b'0'),
                b'a'..=b'f' => u32::from(byte - b'a' + 10),
                b'A'..=b'F' => u32::from(byte - b'A' + 10),
                _ => return Err(parse_error(self.index - 1, "bad \\u escape")),
            };
        }
        Ok(value)
    }

    fn parse_number(&mut self) -> Result<JsonNumber, DomainError> {
        let start = self.index;
        if self.peek()? == b'-' {
            self.index += 1;
        }
        let digits = self.peek()?;
        if digits == b'0' {
            self.index += 1;
        } else if digits.is_ascii_digit() {
            while !self.done() && self.peek()?.is_ascii_digit() {
                self.index += 1;
            }
        } else {
            return Err(parse_error(self.index, "bad number"));
        }
        if !self.done() && self.peek()? == b'.' {
            self.index += 1;
            if self.done() || !self.peek()?.is_ascii_digit() {
                return Err(parse_error(self.index, "bad number"));
            }
            while !self.done() && self.peek()?.is_ascii_digit() {
                self.index += 1;
            }
        }
        if !self.done() && matches!(self.peek()?, b'e' | b'E') {
            self.index += 1;
            if !self.done() && matches!(self.peek()?, b'+' | b'-') {
                self.index += 1;
            }
            if self.done() || !self.peek()?.is_ascii_digit() {
                return Err(parse_error(self.index, "bad number"));
            }
            while !self.done() && self.peek()?.is_ascii_digit() {
                self.index += 1;
            }
        }
        let raw = &self.text[start..self.index];
        let value = raw
            .parse::<f64>()
            .map_err(|_| parse_error(start, "bad number"))?;
        Ok(JsonNumber::Finite(value))
    }

    fn consume(&mut self, token: &str) -> Result<(), DomainError> {
        if self.starts_with(token) {
            self.index += token.len();
            Ok(())
        } else {
            Err(parse_error(self.index, "unexpected character"))
        }
    }

    fn starts_with(&self, token: &str) -> bool {
        self.text[self.index..].starts_with(token)
    }

    fn reject_ident_tail(&self) -> Result<(), DomainError> {
        if self.done() {
            return Ok(());
        }
        let byte = self.text.as_bytes()[self.index];
        if byte.is_ascii_alphanumeric() || byte == b'_' {
            Err(parse_error(self.index, "unexpected character"))
        } else {
            Ok(())
        }
    }
}
