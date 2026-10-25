//! Isolated temporary directories for filesystem tests and local probes.
//!
//! Each harness owns a directory it created exclusively under the process temp
//! root and removes that directory on drop. A preexisting path is never adopted
//! or deleted. Child creation walks owned directory components and refuses
//! symlink parents so a link inside the root cannot redirect writes outside.

use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};
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

    /// Join a relative child under this harness.
    ///
    /// Rejects absolute paths, parent segments, root directories, and platform
    /// prefixes such as Windows drive-relative `C:foo` (`Component::Prefix`
    /// with `is_absolute() == false`).
    pub fn child(&self, relative: impl AsRef<Path>) -> Result<PathBuf, DomainError> {
        let relative = relative.as_ref();
        if relative.is_absolute() {
            return Err(invalid_child(
                relative,
                "temp harness child must be relative",
            ));
        }
        for component in relative.components() {
            match component {
                Component::Normal(_) | Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(invalid_child(
                        relative,
                        "temp harness child must not escape the root",
                    ));
                }
            }
        }
        Ok(self.root.join(relative))
    }

    /// Create owned parent directories for `relative` and return the joined path.
    ///
    /// Intermediate directories are created one component at a time. Symlink
    /// parents are rejected so a link inside the harness cannot redirect
    /// creation outside the root.
    pub fn prepare_file(&self, relative: impl AsRef<Path>) -> Result<PathBuf, DomainError> {
        let path = self.child(relative)?;
        self.ensure_owned_ancestors(&path)?;
        Ok(path)
    }

    /// Create an empty file at `relative` after owned-ancestor checks.
    pub fn create_file(&self, relative: impl AsRef<Path>) -> Result<PathBuf, DomainError> {
        let path = self.prepare_file(relative)?;
        if let Ok(meta) = fs::symlink_metadata(&path)
            && meta.file_type().is_symlink()
        {
            return Err(invalid_child(
                &path,
                "temp harness refuses symlink file targets",
            ));
        }
        File::create(&path).map_err(|error| {
            DomainError::try_new(ErrorCode::WriteFailed, error.to_string())
                .expect("message")
                .with_context(ContextKey::Path, path.display().to_string())
                .expect("path")
                .with_source(error)
        })?;
        self.ensure_final_path_contained(&path)?;
        Ok(path)
    }

    fn ensure_owned_ancestors(&self, path: &Path) -> Result<(), DomainError> {
        let relative = path
            .strip_prefix(&self.root)
            .map_err(|_| invalid_child(path, "temp harness path escaped the owned root"))?;
        let mut current = self.root.clone();
        let components: Vec<_> = relative.components().collect();
        for (index, component) in components.iter().enumerate() {
            let Component::Normal(name) = component else {
                return Err(invalid_child(
                    path,
                    "temp harness child must not escape the root",
                ));
            };
            current.push(name);
            let is_final = index + 1 == components.len();
            if is_final {
                break;
            }
            match fs::symlink_metadata(&current) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err(invalid_child(
                        &current,
                        "temp harness refuses symlink parents",
                    ));
                }
                Ok(meta) if meta.is_dir() => {}
                Ok(_) => {
                    return Err(invalid_child(
                        &current,
                        "temp harness ancestor is not a directory",
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    fs::create_dir(&current).map_err(|error| {
                        DomainError::try_new(ErrorCode::WriteFailed, error.to_string())
                            .expect("message")
                            .with_context(ContextKey::Path, current.display().to_string())
                            .expect("path")
                            .with_source(error)
                    })?;
                }
                Err(error) => {
                    return Err(
                        DomainError::try_new(ErrorCode::WriteFailed, error.to_string())
                            .expect("message")
                            .with_context(ContextKey::Path, current.display().to_string())
                            .expect("path")
                            .with_source(error),
                    );
                }
            }
        }
        if let Some(parent) = path.parent() {
            self.ensure_final_path_contained(parent)?;
        }
        Ok(())
    }

    fn ensure_final_path_contained(&self, path: &Path) -> Result<(), DomainError> {
        if !path.exists() {
            return Ok(());
        }
        let root = fs::canonicalize(&self.root).map_err(|error| {
            DomainError::try_new(ErrorCode::ReadFailed, error.to_string())
                .expect("message")
                .with_context(ContextKey::Path, self.root.display().to_string())
                .expect("path")
                .with_source(error)
        })?;
        let canonical = fs::canonicalize(path).map_err(|error| {
            DomainError::try_new(ErrorCode::ReadFailed, error.to_string())
                .expect("message")
                .with_context(ContextKey::Path, path.display().to_string())
                .expect("path")
                .with_source(error)
        })?;
        if canonical == root || canonical.starts_with(&root) {
            Ok(())
        } else {
            Err(invalid_child(
                path,
                "temp harness path resolved outside the owned root",
            ))
        }
    }
}

fn invalid_child(path: &Path, message: &str) -> DomainError {
    DomainError::try_new(ErrorCode::InvalidArgument, message)
        .expect("message")
        .with_context(ContextKey::Path, path.display().to_string())
        .expect("path")
}

impl Drop for TempHarness {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
