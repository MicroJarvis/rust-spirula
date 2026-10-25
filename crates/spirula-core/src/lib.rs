//! Stable domain contracts shared by the Spirula Studio rewrite.
//!
//! This crate must remain independent of Slint, `wgpu`, filesystem formats,
//! and application process state. P1-F01 owns the error catalog. P1-F02 owns
//! the training-config table and document API described in
//! `docs/phase-1/config.md`. Writers call [`TrainConfig::validate`] and
//! return [`DomainError`] instead of emitting a closed choice the reader
//! rejects.

mod config;
mod config_bind;
mod config_doc;
mod config_value;
mod error;
mod json_doc;

pub use config::{
    ChoiceKind, FieldSpec, FieldType, TRAIN_CONFIG_FIELD_COUNT, TRAIN_CONFIG_FIELDS,
    TRAIN_CONFIG_SECTIONS, TRAIN_CONFIG_TIERS, TrainConfig, field_spec,
};
pub use config_doc::{
    LoadedTrainConfig, TRAIN_CONFIG_SCHEMA_VERSION, UnknownField, field_catalog_json,
};
pub use error::{
    ContextKey, DomainError, DuplicateContextKey, EXIT_CANCELED, EXIT_FAILURE, EXIT_SUCCESS,
    EXIT_USAGE, ErrorCategory, ErrorCode, InvalidMessage, ProcessStage, UnknownContextKey,
    UnknownErrorCode, UserAction, process_exit_code,
};
pub use json_doc::{JsonKind, JsonNumber, JsonValue, parse_json};
