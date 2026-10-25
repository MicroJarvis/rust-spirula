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

use crate::atomic::{PublishOptions, PublishOutcome, acquire_publish_lease};

/// Read and parse a config document from disk.
///
/// Invalid UTF-8 is `SP-FMT-002` with `path`. Other parse failures also carry
/// `path` when the core error did not already set it.
pub fn load_train_config(path: &Path) -> Result<LoadedTrainConfig, DomainError> {
    let bytes = fs::read(path).map_err(|error| map_read(path, error))?;
    let text = std::str::from_utf8(&bytes).map_err(|error| {
        DomainError::try_new(ErrorCode::MalformedRecord, "config text is not UTF-8")
            .expect("message")
            .with_context(ContextKey::Path, path.display().to_string())
            .expect("path")
            .with_context(ContextKey::Field, "config")
            .expect("field")
            .with_source(error)
    })?;
    LoadedTrainConfig::parse(text).map_err(|error| attach_path(error, path))
}

/// Atomically rewrite `path` with preserved-document JSON.
///
/// Holds the destination lease across the disk snapshot check and publish so
/// a concurrent rewrite cannot slip between them.
pub fn save_train_config(
    path: &Path,
    loaded: &LoadedTrainConfig,
    options: PublishOptions<'_>,
) -> Result<PublishOutcome, DomainError> {
    let lease = acquire_publish_lease(path)?;
    ensure_loaded_matches_disk(lease.destination(), loaded)?;
    let text = loaded.to_preserved_json()?;
    lease.publish_bytes_validated(text.as_bytes(), options, validate_config_bytes)
}

/// Promote a loaded document to the current form and atomically replace `path`.
///
/// Under the destination lease, plus a separate backup lease for the no-clobber
/// backup publish (never re-locks the same destination):
/// 1. Require `loaded.source_text()` to match the live file (`SP-SESS-001`).
/// 2. Build and validate the migrated document before any backup write.
/// 3. Create `{path}.bak` under its own lease with atomic no-clobber
///    (`hard_link`); an existing backup is left untouched.
/// 4. Publish the migrated document to `path`.
pub fn migrate_train_config(
    path: &Path,
    loaded: &LoadedTrainConfig,
    options: PublishOptions<'_>,
) -> Result<PublishOutcome, DomainError> {
    let lease = acquire_publish_lease(path)?;
    let original = ensure_loaded_matches_disk(lease.destination(), loaded)?;
    let migrated = loaded.to_current_json()?;
    validate_config_bytes(migrated.as_bytes())?;

    let backup = backup_path(lease.destination());
    {
        // Separate destination → separate lock file. Acquire while holding the
        // config lease so snapshot/backup/publish stay one transaction; never
        // call acquire on `path` again here (would deadlock with ourselves).
        let backup_lease = acquire_publish_lease(&backup)?;
        backup_lease.publish_bytes_if_absent(&original, options)?;
    }

    lease.publish_bytes_validated(migrated.as_bytes(), options, validate_config_bytes)
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

fn attach_path(error: DomainError, path: &Path) -> DomainError {
    if error.context_value(ContextKey::Path).is_some() {
        return error;
    }
    error
        .with_context(ContextKey::Path, path.display().to_string())
        .expect("path context was absent")
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
