//! Load and save a flat training-config document.
//!
//! The rules are recorded in `docs/phase-1/config.md`. This module does not
//! touch the filesystem.

use std::collections::BTreeSet;

use crate::config::{FieldType, TRAIN_CONFIG_FIELD_COUNT, TRAIN_CONFIG_FIELDS, TrainConfig};
use crate::config_bind::{assign_field, canonical_field};
use crate::config_value::{LexemeInteger, lexeme_integer};
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
    /// Reject nonempty closed choices the reader would reject.
    ///
    /// An empty string is the canonical JSON `null` encoding and is writable
    /// for every string field. An explicit empty JSON string is still a read
    /// error unless the field's reference default is empty. `None` optionals
    /// stay valid.
    pub fn validate(&self) -> Result<(), DomainError> {
        crate::config_bind::validate_fields(self)
    }

    /// Current document: `schema_version` 1 and every field in table order.
    ///
    /// Returns the same error the reader would return for an invalid closed choice.
    pub fn to_current_json(&self) -> Result<String, DomainError> {
        self.validate()?;
        Ok(write_current(self, &[]))
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
    /// An unchanged valid config returns the original text. Edited fields that
    /// were absent are appended in table order. Unknown values are copied from
    /// the source text. An invalid closed choice is not written.
    pub fn to_preserved_json(&self) -> Result<String, DomainError> {
        self.config.validate()?;
        if self.config.same_values(&self.baseline) {
            return Ok(self.source.clone());
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
        Ok(out)
    }

    /// Explicit migration text. Does not write a file.
    pub fn to_current_json(&self) -> Result<String, DomainError> {
        let unknown: Vec<(&str, &str)> = self
            .entries
            .iter()
            .filter_map(|slot| match slot {
                RootSlot::Unknown { name, raw } => Some((name.as_str(), raw.as_str())),
                _ => None,
            })
            .collect();
        self.config.validate()?;
        Ok(write_current(&self.config, &unknown))
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
    let JsonKind::Number(JsonNumber::Finite(_)) = value.kind else {
        return Err(shape_error("schema_version", "integer", value.actual()));
    };
    match lexeme_integer(value.raw()) {
        LexemeInteger::Fits(1) => Ok(1),
        LexemeInteger::Fits(_) | LexemeInteger::TooWide => Err(version_error(value.raw())),
        LexemeInteger::NotInteger => Err(shape_error("schema_version", "integer", value.raw())),
    }
}

#[cfg(test)]
mod tests {
    use super::LoadedTrainConfig;
    use crate::config::{TRAIN_CONFIG_FIELDS, TrainConfig};
    use crate::config_bind::{canonical_field, fill_probe};

    #[test]
    fn nondefault_assignments_cover_every_field() {
        let defaults = TrainConfig::default();
        defaults.validate().unwrap();
        let mut config = TrainConfig::default();
        fill_probe(&mut config);
        config.validate().unwrap();
        assert_eq!(TRAIN_CONFIG_FIELDS.len(), 251);
        for spec in TRAIN_CONFIG_FIELDS {
            let now = canonical_field(&config, spec.name).expect(spec.name);
            let before = canonical_field(&defaults, spec.name).expect(spec.name);
            assert_ne!(now, before, "{}", spec.name);
        }
        let text = config.to_current_json().unwrap();
        let loaded = LoadedTrainConfig::parse(&text).unwrap();
        assert!(loaded.config.same_values(&config));
        assert_eq!(loaded.to_preserved_json().unwrap(), text);
        assert_eq!(loaded.to_current_json().unwrap(), text);
    }
}
