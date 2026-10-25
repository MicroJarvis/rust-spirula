//! Filesystem load, save, and migration for training config documents.
//!
//! Document text rules stay in `spirula-core` / `docs/phase-1/config.md`.
//! Publish rules are in `docs/phase-1/atomic.md`. This module finishes the
//! P1-F02 filesystem contract with atomic replace and an explicit migration
//! backup. The reference still writes `config.json` directly
//! (`TrainerCore.cpp:759-768`).

use std::fs;
use std::path::{Path, PathBuf};

use spirula_core::{ContextKey, DomainError, ErrorCode, LoadedTrainConfig};

use crate::atomic::{PublishOptions, PublishOutcome, publish_bytes_validated};

/// Read and parse a config document from disk.
pub fn load_train_config(path: &Path) -> Result<LoadedTrainConfig, DomainError> {
    let text = read_text(path)?;
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
    ensure_loaded_matches_disk(path, loaded)?;
    let text = loaded.to_preserved_json()?;
    publish_bytes_validated(path, text.as_bytes(), options, validate_config_bytes)
}

/// Promote a loaded document to the current form and atomically replace `path`.
///
/// Order of side effects:
/// 1. Require `loaded.source_text()` to match the live file (stale snapshots
///    are `SP-SESS-001`).
/// 2. Build and validate the migrated document before any backup write.
/// 3. Create `{path}.bak` only when it does not already exist (no-clobber). An
///    existing backup is left untouched so retries keep the original rollback
///    bytes.
/// 4. Atomically publish the migrated document to `path`.
pub fn migrate_train_config(
    path: &Path,
    loaded: &LoadedTrainConfig,
    options: PublishOptions<'_>,
) -> Result<PublishOutcome, DomainError> {
    let original = ensure_loaded_matches_disk(path, loaded)?;
    let migrated = loaded.to_current_json()?;
    validate_config_bytes(migrated.as_bytes())?;

    let backup = backup_path(path);
    if !backup.exists() {
        publish_bytes_validated(&backup, &original, options, |_| Ok(()))?;
    }

    publish_bytes_validated(path, migrated.as_bytes(), options, validate_config_bytes)
}

fn ensure_loaded_matches_disk(
    path: &Path,
    loaded: &LoadedTrainConfig,
) -> Result<Vec<u8>, DomainError> {
    let disk = fs::read(path).map_err(|error| map_read(path, error))?;
    if disk.as_slice() != loaded.source_text().as_bytes() {
        return Err(DomainError::try_new(
            ErrorCode::InvalidState,
            "loaded config snapshot does not match the file on disk",
        )
        .expect("message")
        .with_context(ContextKey::Path, path.display().to_string())
        .expect("path")
        .with_context(ContextKey::Expected, "loaded.source_text()")
        .expect("expected")
        .with_context(ContextKey::Actual, "disk contents changed")
        .expect("actual"));
    }
    Ok(disk)
}

fn backup_path(path: &Path) -> PathBuf {
    let mut backup = path.as_os_str().to_owned();
    backup.push(".bak");
    PathBuf::from(backup)
}

fn read_text(path: &Path) -> Result<String, DomainError> {
    fs::read_to_string(path).map_err(|error| map_read(path, error))
}

fn map_read(path: &Path, error: std::io::Error) -> DomainError {
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
