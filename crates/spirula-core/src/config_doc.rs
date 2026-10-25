//! Load and save a flat training-config document.
//!
//! The rules are recorded in `docs/phase-1/config.md`. This module does not
//! touch the filesystem.

use std::collections::BTreeSet;

use crate::config::{FieldType, TRAIN_CONFIG_FIELD_COUNT, TRAIN_CONFIG_FIELDS, TrainConfig};
use crate::config_bind::{assign_field, canonical_field};
use crate::json_doc::{
    JsonKind, JsonNumber, JsonValue, parse_json, quote_json, shape_error, version_error,
};
use crate::{ChoiceKind, DomainError};

/// Current `schema_version` written by documents this crate originates.
pub const TRAIN_CONFIG_SCHEMA_VERSION: u64 = 1;

/// A config document loaded from text, plus the unknown values it carried.
#[derive(Clone, Debug)]
pub struct LoadedTrainConfig {
    /// Interpreted flags. Missing keys keep their defaults.
    pub config: TrainConfig,
    source: String,
    baseline: TrainConfig,
    version: Option<u64>,
    entries: Vec<RootSlot>,
}

#[derive(Clone, Debug)]
enum RootSlot {
    Version { raw: String },
    Known(&'static str),
    Unknown { name: String, raw: String },
}

/// One unknown root key and the JSON text of its value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownField {
    pub name: String,
    pub raw: String,
}

impl TrainConfig {
    /// Current document: `schema_version` 1 and every field in table order.
    pub fn to_current_json(&self) -> String {
        write_current(self, &[])
    }
}

impl LoadedTrainConfig {
    /// Parse a config document. Legacy files omit `schema_version`.
    pub fn parse(text: &str) -> Result<Self, DomainError> {
        let root = parse_json(text)?;
        let JsonKind::Object(pairs) = root.kind else {
            return Err(shape_error("document", "object", root.actual()));
        };
        let mut seen = BTreeSet::new();
        for (name, _) in &pairs {
            if !seen.insert(name.clone()) {
                return Err(shape_error(name, "unique key", "duplicate key"));
            }
        }

        let mut config = TrainConfig::default();
        let mut version = None;
        let mut entries = Vec::new();
        for (name, value) in &pairs {
            if name == "schema_version" {
                version = Some(read_version(value)?);
                entries.push(RootSlot::Version {
                    raw: value.raw().to_owned(),
                });
                continue;
            }
            if assign_field(&mut config, name, value)? {
                let spec_name = crate::field_spec(name)
                    .map(|spec| spec.name)
                    .expect("assigned field");
                entries.push(RootSlot::Known(spec_name));
            } else {
                entries.push(RootSlot::Unknown {
                    name: name.clone(),
                    raw: value.raw().to_owned(),
                });
            }
        }

        Ok(Self {
            baseline: config.clone(),
            config,
            source: text.to_owned(),
            version,
            entries,
        })
    }

    /// `Some(1)` for a current document. `None` means an unversioned legacy file.
    pub const fn schema_version(&self) -> Option<u64> {
        self.version
    }

    /// Unknown root keys, in document order. Nested values stay in [`UnknownField::raw`].
    pub fn unknown_fields(&self) -> Vec<UnknownField> {
        self.entries
            .iter()
            .filter_map(|slot| match slot {
                RootSlot::Unknown { name, raw } => Some(UnknownField {
                    name: name.clone(),
                    raw: raw.clone(),
                }),
                _ => None,
            })
            .collect()
    }

    /// Save without inserting untouched defaults or a missing `schema_version`.
    ///
    /// An unchanged config returns the original text. Edited fields that were
    /// absent are appended in table order. Unknown values are copied from the
    /// source text.
    pub fn to_preserved_json(&self) -> String {
        if self.config.same_values(&self.baseline) {
            return self.source.clone();
        }
        let mut out = String::from("{");
        let mut first = true;
        let mut seen = BTreeSet::new();
        for slot in &self.entries {
            match slot {
                RootSlot::Version { raw } => {
                    push_entry(&mut out, &mut first, "schema_version", raw);
                    seen.insert("schema_version");
                }
                RootSlot::Known(name) => {
                    let raw = canonical_field(&self.config, name).expect("known field");
                    push_entry(&mut out, &mut first, name, &raw);
                    seen.insert(name);
                }
                RootSlot::Unknown { name, raw } => {
                    push_entry(&mut out, &mut first, name, raw);
                }
            }
        }
        for spec in TRAIN_CONFIG_FIELDS {
            if seen.contains(spec.name) {
                continue;
            }
            let now = canonical_field(&self.config, spec.name).expect("known field");
            let before = canonical_field(&self.baseline, spec.name).expect("known field");
            if now != before {
                push_entry(&mut out, &mut first, spec.name, &now);
            }
        }
        out.push('}');
        out.push('\n');
        out
    }

    /// Explicit migration text. Does not write a file.
    pub fn to_current_json(&self) -> String {
        let unknown: Vec<(&str, &str)> = self
            .entries
            .iter()
            .filter_map(|slot| match slot {
                RootSlot::Unknown { name, raw } => Some((name.as_str(), raw.as_str())),
                _ => None,
            })
            .collect();
        write_current(&self.config, &unknown)
    }
}

/// Golden catalog of every field, its metadata and the canonical default spelling.
pub fn field_catalog_json() -> String {
    let mut out = format!(
        "{{\"schema\":\"spirula-train-config-catalog\",\"version\":1,\"field_count\":{TRAIN_CONFIG_FIELD_COUNT},\"fields\":["
    );
    let defaults = TrainConfig::default();
    for (index, spec) in TRAIN_CONFIG_FIELDS.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        let default_json = canonical_field(&defaults, spec.name).expect("known field");
        let value_type = match spec.value_type {
            FieldType::String => "string",
            FieldType::Int => "int",
            FieldType::Bool => "bool",
            FieldType::Float => "float",
            FieldType::OptionalBool => "optional_bool",
            FieldType::OptionalFloat => "optional_float",
            FieldType::OptionalInt => "optional_int",
            FieldType::Vec3Int => "vec3_int",
            FieldType::Vec3Float => "vec3_float",
        };
        let choice_kind = match spec.choice_kind {
            ChoiceKind::Free => "free",
            ChoiceKind::FreeForm => "free_form",
            ChoiceKind::Path => "path",
            ChoiceKind::Closed(_) => "closed",
        };
        out.push_str(&format!(
            "{{\"name\":{},\"type\":{},\"section\":{},\"tier\":{},\"choices\":{},\"choice_kind\":{},\"empty_is_unset\":{},\"default_json\":{}}}",
            quote_json(spec.name),
            quote_json(value_type),
            quote_json(spec.section),
            quote_json(spec.tier),
            quote_json(spec.choices),
            quote_json(choice_kind),
            if spec.empty_is_unset { "true" } else { "false" },
            quote_json(&default_json),
        ));
    }
    out.push_str("]}\n");
    out
}

fn write_current(config: &TrainConfig, unknown: &[(&str, &str)]) -> String {
    let mut out = format!("{{\"schema_version\":{TRAIN_CONFIG_SCHEMA_VERSION}");
    for spec in TRAIN_CONFIG_FIELDS {
        let raw = canonical_field(config, spec.name).expect("known field");
        out.push(',');
        out.push_str(&quote_json(spec.name));
        out.push(':');
        out.push_str(&raw);
    }
    for (name, raw) in unknown {
        out.push(',');
        out.push_str(&quote_json(name));
        out.push(':');
        out.push_str(raw);
    }
    out.push('}');
    out.push('\n');
    out
}

fn push_entry(out: &mut String, first: &mut bool, key: &str, raw: &str) {
    if !*first {
        out.push(',');
    }
    *first = false;
    out.push_str(&quote_json(key));
    out.push(':');
    out.push_str(raw);
}

fn read_version(value: &JsonValue) -> Result<u64, DomainError> {
    match value.kind {
        JsonKind::Number(JsonNumber::Finite(number))
            if number.is_finite() && number.fract() == 0.0 && number == 1.0 =>
        {
            Ok(1)
        }
        JsonKind::Number(JsonNumber::Finite(number))
            if number.is_finite() && number.fract() == 0.0 =>
        {
            Err(version_error(value.raw()))
        }
        _ => Err(shape_error("schema_version", "integer", value.actual())),
    }
}
