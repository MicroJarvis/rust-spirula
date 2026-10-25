//! Isolated temporary directories for filesystem tests and local probes.
//!
//! Each harness owns a unique directory under the process temp root and removes
//! it on drop. Tests should not share paths across harnesses.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use spirula_core::{ContextKey, DomainError, ErrorCode};

static HARNESS_SEQ: AtomicU64 = AtomicU64::new(1);

/// A unique temporary directory removed when dropped.
#[derive(Debug)]
pub struct TempHarness {
    root: PathBuf,
}

impl TempHarness {
    /// Create `spirula-io-<pid>-<seq>` under the system temporary directory.
    pub fn new() -> Result<Self, DomainError> {
        let seq = HARNESS_SEQ.fetch_add(1, Ordering::Relaxed);
        let name = format!("spirula-io-{}-{seq}", std::process::id());
        let root = std::env::temp_dir().join(name);
        fs::create_dir_all(&root).map_err(|error| {
            DomainError::try_new(ErrorCode::WriteFailed, error.to_string())
                .expect("message")
                .with_context(ContextKey::Path, root.display().to_string())
                .expect("path")
                .with_source(error)
        })?;
        Ok(Self { root })
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
