//! Isolated temporary directories for filesystem tests and local probes.
//!
//! Each harness owns a directory it created exclusively under the process temp
//! root and removes that directory on drop. A preexisting path is never adopted
//! or deleted.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use spirula_core::{ContextKey, DomainError, ErrorCode};

static HARNESS_SEQ: AtomicU64 = AtomicU64::new(1);

/// A unique temporary directory removed when dropped.
#[derive(Debug)]
pub struct TempHarness {
    root: PathBuf,
}

impl TempHarness {
    /// Allocate a new empty directory with `create_dir`, retrying on
    /// `AlreadyExists`. Never reuses or deletes a preexisting root.
    pub fn new() -> Result<Self, DomainError> {
        for _ in 0..64 {
            let seq = HARNESS_SEQ.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0);
            let name = format!("spirula-io-{}-{seq}-{nanos}", std::process::id());
            let root = std::env::temp_dir().join(name);
            match fs::create_dir(&root) {
                Ok(()) => return Ok(Self { root }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(
                        DomainError::try_new(ErrorCode::WriteFailed, error.to_string())
                            .expect("message")
                            .with_context(ContextKey::Path, root.display().to_string())
                            .expect("path")
                            .with_source(error),
                    );
                }
            }
        }
        Err(DomainError::try_new(
            ErrorCode::WriteFailed,
            "could not allocate a unique temp harness",
        )
        .expect("message"))
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Join a relative child under this harness. Rejects absolute paths and
    /// parent-directory segments so tests stay inside the isolation root.
    pub fn child(&self, relative: impl AsRef<Path>) -> Result<PathBuf, DomainError> {
        let relative = relative.as_ref();
        if relative.is_absolute() {
            return Err(DomainError::try_new(
                ErrorCode::InvalidArgument,
                "temp harness child must be relative",
            )
            .expect("message")
            .with_context(ContextKey::Path, relative.display().to_string())
            .expect("path"));
        }
        for component in relative.components() {
            if matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::RootDir
            ) {
                return Err(DomainError::try_new(
                    ErrorCode::InvalidArgument,
                    "temp harness child must not escape the root",
                )
                .expect("message")
                .with_context(ContextKey::Path, relative.display().to_string())
                .expect("path"));
            }
        }
        Ok(self.root.join(relative))
    }

    /// Create parent directories for `relative` and return the joined path.
    pub fn prepare_file(&self, relative: impl AsRef<Path>) -> Result<PathBuf, DomainError> {
        let path = self.child(relative)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                DomainError::try_new(ErrorCode::WriteFailed, error.to_string())
                    .expect("message")
                    .with_context(ContextKey::Path, parent.display().to_string())
                    .expect("path")
                    .with_source(error)
            })?;
        }
        Ok(path)
    }

    /// Create an empty file at `relative`.
    pub fn create_file(&self, relative: impl AsRef<Path>) -> Result<PathBuf, DomainError> {
        let path = self.prepare_file(relative)?;
        File::create(&path).map_err(|error| {
            DomainError::try_new(ErrorCode::WriteFailed, error.to_string())
                .expect("message")
                .with_context(ContextKey::Path, path.display().to_string())
                .expect("path")
                .with_source(error)
        })?;
        Ok(path)
    }
}

impl Drop for TempHarness {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
