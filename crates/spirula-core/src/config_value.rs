//! Typed reads and canonical spellings for one config field.

use crate::DomainError;
use crate::config::{ChoiceKind, FieldSpec, field_spec};
use crate::json_doc::{
    JsonKind, JsonNumber, JsonValue, choice_error, format_f32, quote_json, range_error, shape_error,
};

pub(crate) enum Take<T> {
    Keep,
    Set(T),
}

pub(crate) fn take_string(name: &str, value: &JsonValue) -> Result<String, DomainError> {
    let spec = field_spec(name).expect("known config field");
    match &value.kind {
        JsonKind::Null => Ok(String::new()),
        JsonKind::String(text) => {
            check_choice(spec, text)?;
            Ok(text.clone())
        }
        _ => Err(shape_error(name, "string or null", value.actual())),
    }
}

pub(crate) fn take_bool(name: &str, value: &JsonValue) -> Result<Take<bool>, DomainError> {
    match value.kind {
        JsonKind::Null => Ok(Take::Keep),
        JsonKind::Bool(flag) => Ok(Take::Set(flag)),
        _ => Err(shape_error(name, "bool or null", value.actual())),
    }
}

pub(crate) fn take_i32(name: &str, value: &JsonValue) -> Result<Take<i32>, DomainError> {
    match value.kind {
        JsonKind::Null => Ok(Take::Keep),
        JsonKind::Number(number) => Ok(Take::Set(exact_i32(name, value.raw(), number)?)),
        _ => Err(shape_error(name, "integer or null", value.actual())),
    }
}

pub(crate) fn take_f32(name: &str, value: &JsonValue) -> Result<Take<f32>, DomainError> {
    match value.kind {
        JsonKind::Null => Ok(Take::Keep),
        JsonKind::Number(number) => Ok(Take::Set(as_f32(number))),
        _ => Err(shape_error(name, "number or null", value.actual())),
    }
}

pub(crate) fn take_opt_bool(name: &str, value: &JsonValue) -> Result<Option<bool>, DomainError> {
    match value.kind {
        JsonKind::Null => Ok(None),
        JsonKind::Bool(flag) => Ok(Some(flag)),
        _ => Err(shape_error(name, "bool or null", value.actual())),
    }
}

pub(crate) fn take_opt_i32(name: &str, value: &JsonValue) -> Result<Option<i32>, DomainError> {
    match value.kind {
        JsonKind::Null => Ok(None),
        JsonKind::Number(number) => Ok(Some(exact_i32(name, value.raw(), number)?)),
        _ => Err(shape_error(name, "integer or null", value.actual())),
    }
}

pub(crate) fn take_opt_f32(name: &str, value: &JsonValue) -> Result<Option<f32>, DomainError> {
    match value.kind {
        JsonKind::Null => Ok(None),
        JsonKind::Number(number) => Ok(Some(as_f32(number))),
        _ => Err(shape_error(name, "number or null", value.actual())),
    }
}

pub(crate) fn take_vec3_i(name: &str, value: &JsonValue) -> Result<Take<[i32; 3]>, DomainError> {
    let JsonKind::Array(items) = &value.kind else {
        return match value.kind {
            JsonKind::Null => Ok(Take::Keep),
            _ => Err(shape_error(
                name,
                "array of 3 integers or null",
                value.actual(),
            )),
        };
    };
    if items.len() != 3 {
        return Err(shape_error(name, "array of 3 integers", value.actual()));
    }
    let mut out = [0; 3];
    for (index, item) in items.iter().enumerate() {
        match take_i32(&format!("{name}[{index}]"), item)? {
            Take::Keep => {
                return Err(shape_error(&format!("{name}[{index}]"), "integer", "null"));
            }
            Take::Set(number) => out[index] = number,
        }
    }
    Ok(Take::Set(out))
}

pub(crate) fn take_vec3_f(name: &str, value: &JsonValue) -> Result<Take<[f32; 3]>, DomainError> {
    let JsonKind::Array(items) = &value.kind else {
        return match value.kind {
            JsonKind::Null => Ok(Take::Keep),
            _ => Err(shape_error(
                name,
                "array of 3 numbers or null",
                value.actual(),
            )),
        };
    };
    if items.len() != 3 {
        return Err(shape_error(name, "array of 3 numbers", value.actual()));
    }
    let mut out = [0.0; 3];
    for (index, item) in items.iter().enumerate() {
        match take_f32(&format!("{name}[{index}]"), item)? {
            Take::Keep => {
                return Err(shape_error(&format!("{name}[{index}]"), "number", "null"));
            }
            Take::Set(number) => out[index] = number,
        }
    }
    Ok(Take::Set(out))
}

pub(crate) fn canonical_string(value: &str) -> String {
    if value.is_empty() {
        "null".to_owned()
    } else {
        quote_json(value)
    }
}

pub(crate) fn canonical_bool(value: bool) -> String {
    if value { "true" } else { "false" }.to_owned()
}

pub(crate) fn canonical_i32(value: i32) -> String {
    value.to_string()
}

pub(crate) fn canonical_f32(value: f32) -> String {
    format_f32(value)
}

pub(crate) fn canonical_opt_bool(value: Option<bool>) -> String {
    value
        .map(canonical_bool)
        .unwrap_or_else(|| "null".to_owned())
}

pub(crate) fn canonical_opt_i32(value: Option<i32>) -> String {
    value
        .map(canonical_i32)
        .unwrap_or_else(|| "null".to_owned())
}

pub(crate) fn canonical_opt_f32(value: Option<f32>) -> String {
    value
        .map(canonical_f32)
        .unwrap_or_else(|| "null".to_owned())
}

pub(crate) fn canonical_vec3_i(value: &[i32; 3]) -> String {
    format!("[{},{},{}]", value[0], value[1], value[2])
}

pub(crate) fn canonical_vec3_f(value: &[f32; 3]) -> String {
    format!(
        "[{},{},{}]",
        format_f32(value[0]),
        format_f32(value[1]),
        format_f32(value[2])
    )
}

pub(crate) fn check_choice(spec: &FieldSpec, value: &str) -> Result<(), DomainError> {
    let ChoiceKind::Closed(choices) = spec.choice_kind else {
        return Ok(());
    };
    if value.is_empty() && spec.empty_is_unset {
        return Ok(());
    }
    if choices.contains(&value) {
        return Ok(());
    }
    Err(choice_error(spec.name, value, spec.choices))
}

fn as_f32(number: JsonNumber) -> f32 {
    match number {
        JsonNumber::Finite(value) => value as f32,
        JsonNumber::Infinity => f32::INFINITY,
        JsonNumber::NegInfinity => f32::NEG_INFINITY,
        JsonNumber::NaN => f32::NAN,
    }
}

fn exact_i32(name: &str, raw: &str, number: JsonNumber) -> Result<i32, DomainError> {
    if !matches!(number, JsonNumber::Finite(_)) {
        return Err(shape_error(name, "integer", raw));
    }
    match lexeme_integer(raw) {
        LexemeInteger::Fits(value) if (i32::MIN as i128..=i32::MAX as i128).contains(&value) => {
            Ok(value as i32)
        }
        LexemeInteger::Fits(_) | LexemeInteger::TooWide => Err(range_error(name, raw)),
        LexemeInteger::NotInteger => Err(shape_error(name, "integer", raw)),
    }
}

/// Exact integer carried by a JSON number lexeme.
///
/// `f64` rounds `1.0000000000000001` to `1`, so integrality is decided from
/// the decimal digits and exponent, not from [`f64::fract`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LexemeInteger {
    NotInteger,
    Fits(i128),
    TooWide,
}

pub(crate) fn lexeme_integer(raw: &str) -> LexemeInteger {
    let Some((negative, mut digits, mut scale)) = decimal_parts(raw) else {
        return LexemeInteger::NotInteger;
    };
    while digits.len() > 1 && digits.ends_with('0') && scale > i64::MIN {
        digits.pop();
        scale -= 1;
    }
    let trimmed = digits.trim_start_matches('0');
    if trimmed.is_empty() {
        return LexemeInteger::Fits(0);
    }
    if scale > 0 {
        return LexemeInteger::NotInteger;
    }
    if scale == i64::MIN {
        return LexemeInteger::TooWide;
    }
    let extra = (-scale) as usize;
    if trimmed.len().saturating_add(extra) > 40 {
        return LexemeInteger::TooWide;
    }
    let mut text = String::with_capacity(trimmed.len() + extra);
    text.push_str(trimmed);
    text.extend(std::iter::repeat_n('0', extra));
    let Ok(magnitude) = text.parse::<i128>() else {
        return LexemeInteger::TooWide;
    };
    let signed = if negative { -magnitude } else { magnitude };
    LexemeInteger::Fits(signed)
}

enum Exponent {
    Value(i64),
    PositiveHuge,
    NegativeHuge,
}

fn decimal_parts(raw: &str) -> Option<(bool, String, i64)> {
    if raw.is_empty() {
        return None;
    }
    let (negative, rest) = match raw.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, raw),
    };
    if rest.is_empty() {
        return None;
    }
    let (body, exponent) = match rest.find(['e', 'E']) {
        Some(index) => (&rest[..index], Some(&rest[index + 1..])),
        None => (rest, None),
    };
    if body.is_empty() {
        return None;
    }
    let (int_part, frac_part) = match body.split_once('.') {
        Some((int_part, frac_part)) => (int_part, frac_part),
        None => (body, ""),
    };
    if int_part.is_empty()
        || (frac_part.is_empty() && body.contains('.'))
        || !int_part.bytes().all(|byte| byte.is_ascii_digit())
        || !frac_part.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let exponent = match exponent {
        None => Exponent::Value(0),
        Some(text) => parse_exponent(text)?,
    };
    let mut digits = String::with_capacity(int_part.len() + frac_part.len());
    digits.push_str(int_part);
    digits.push_str(frac_part);
    Some((
        negative,
        digits,
        scale_from(frac_part.len() as i64, exponent),
    ))
}

fn parse_exponent(text: &str) -> Option<Exponent> {
    if text.is_empty() {
        return None;
    }
    let (negative, digits) = if let Some(rest) = text.strip_prefix('-') {
        (true, rest)
    } else if let Some(rest) = text.strip_prefix('+') {
        (false, rest)
    } else {
        (false, text)
    };
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        return Some(Exponent::Value(0));
    }
    if digits.len() > 18 {
        return Some(if negative {
            Exponent::NegativeHuge
        } else {
            Exponent::PositiveHuge
        });
    }
    let value: i64 = digits.parse().ok()?;
    Some(if negative {
        Exponent::Value(-value)
    } else {
        Exponent::Value(value)
    })
}

fn scale_from(fraction_len: i64, exponent: Exponent) -> i64 {
    match exponent {
        Exponent::PositiveHuge => i64::MIN,
        Exponent::NegativeHuge => i64::MAX,
        Exponent::Value(exponent) => match fraction_len.checked_sub(exponent) {
            Some(scale) => scale,
            None if exponent > 0 => i64::MIN,
            None => i64::MAX,
        },
    }
}
