//! Filesystem load, save, and migration for training config documents.
//!
//! Document text rules stay in `spirula-core` / `docs/phase-1/config.md`.
//! Publish rules are in `docs/phase-1/atomic.md`. This module finishes the
//! P1-F02 filesystem contract with atomic replace and an explicit migration
//! backup. The reference still writes `config.json` directly
//! (`TrainerCore.cpp:759-768`).

use std::fs;
use std::path::Path;

use spirula_core::{ContextKey, DomainError, ErrorCode, LoadedTrainConfig};

use crate::atomic::{PublishOptions, PublishOutcome, publish_bytes, publish_bytes_validated};

/// Read and parse a config document from disk.
pub fn load_train_config(path: &Path) -> Result<LoadedTrainConfig, DomainError> {
    let text = fs::read_to_string(path).map_err(|error| {
        let code = match error.kind() {
            std::io::ErrorKind::NotFound => ErrorCode::NotFound,
            std::io::ErrorKind::PermissionDenied => ErrorCode::PermissionDenied,
            _ => ErrorCode::ReadFailed,
        };
        DomainError::try_new(code, error.to_string())
            .expect("message")
            .with_context(ContextKey::Path, path.display().to_string())
            .expect("path")
            .with_source(error)
    })?;
    LoadedTrainConfig::parse(&text)
}

/// Atomically rewrite `path` with preserved-document JSON.
///
/// Does not insert a missing `schema_version`. Leaves the previous file
/// intact when validation or the publish fails.
pub fn save_train_config(
    path: &Path,
    loaded: &LoadedTrainConfig,
    options: PublishOptions<'_>,
) -> Result<PublishOutcome, DomainError> {
    let text = loaded.to_preserved_json()?;
    publish_bytes_validated(path, text.as_bytes(), options, validate_config_bytes)
}

/// Promote a loaded document to the current form and atomically replace `path`.
///
/// Writes `{path}.bak` with the pre-migration bytes first (atomic replace of
/// any previous backup), then publishes the migrated document. A failure while
/// writing the destination leaves both the original destination and the backup
/// in place when the backup step already succeeded.
pub fn migrate_train_config(
    path: &Path,
    loaded: &LoadedTrainConfig,
    options: PublishOptions<'_>,
) -> Result<PublishOutcome, DomainError> {
    let original = fs::read(path).map_err(|error| {
        let code = match error.kind() {
            std::io::ErrorKind::NotFound => ErrorCode::NotFound,
            std::io::ErrorKind::PermissionDenied => ErrorCode::PermissionDenied,
            _ => ErrorCode::ReadFailed,
        };
        DomainError::try_new(code, error.to_string())
            .expect("message")
            .with_context(ContextKey::Path, path.display().to_string())
            .expect("path")
            .with_source(error)
    })?;
    let backup = backup_path(path);
    publish_bytes(&backup, &original, options)?;
    let migrated = loaded.to_current_json()?;
    publish_bytes_validated(path, migrated.as_bytes(), options, validate_config_bytes)
}

fn backup_path(path: &Path) -> std::path::PathBuf {
    let mut backup = path.as_os_str().to_owned();
    backup.push(".bak");
    std::path::PathBuf::from(backup)
}

fn validate_config_bytes(bytes: &[u8]) -> Result<(), DomainError> {
    let text = std::str::from_utf8(bytes).map_err(|error| {
        DomainError::try_new(ErrorCode::MalformedRecord, "config text is not UTF-8")
            .expect("message")
            .with_context(ContextKey::Field, "config")
            .expect("field")
            .with_source(error)
    })?;
    let loaded = LoadedTrainConfig::parse(text)?;
    loaded.config.validate()?;
    Ok(())
}
