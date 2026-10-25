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

fn check_choice(spec: &FieldSpec, value: &str) -> Result<(), DomainError> {
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
    let JsonNumber::Finite(value) = number else {
        return Err(shape_error(name, "integer", raw));
    };
    if !value.is_finite() || value.fract() != 0.0 {
        return Err(shape_error(name, "integer", raw));
    }
    if value < f64::from(i32::MIN) || value > f64::from(i32::MAX) {
        return Err(range_error(name, raw));
    }
    Ok(value as i32)
}
