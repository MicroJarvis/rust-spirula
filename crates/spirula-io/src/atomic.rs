//! Temporary sibling files and atomic destination replace.
//!
//! Pinned reference: `src/core/AtomicFile.h` uses POSIX `rename` and Windows
//! `MoveFileExW(MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)`. Config
//! and checkpoint writers in the reference often open the destination directly
//! (`TrainerCore.cpp:759-768`, checkpoint gaps in the inventory); this module
//! is the rewrite path those call sites should use. Checkpoint bundle layout
//! and completion markers stay in P5-F05.
//!
//! There is no delete-then-rename window: the destination is replaced only by
//! a same-directory rename / `MoveFileExW`.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use spirula_core::{CancelToken, ContextKey, DomainError, ErrorCode};

const TEMP_MARKER: &str = ".spirula-tmp.";
static TEMP_SEQ: AtomicU64 = AtomicU64::new(1);

/// Options for one atomic publish.
#[derive(Clone, Copy, Debug)]
pub struct PublishOptions<'a> {
    /// Cooperative cancel checked before create, before replace, and by callers
    /// that stream through [`publish_writer`].
    pub cancel: Option<&'a CancelToken>,
    /// After a successful replace, `fsync` the parent directory when the
    /// platform supports it. Defaults to `true` on Unix and `false` on
    /// Windows, where directory handles do not provide the same durability
    /// contract. A failure here means the destination already holds the new
    /// bytes; the error carries `actual=postpublish_directory_sync`.
    pub sync_directory: bool,
}

impl Default for PublishOptions<'static> {
    fn default() -> Self {
        Self {
            cancel: None,
            sync_directory: cfg!(unix),
        }
    }
}

impl<'a> PublishOptions<'a> {
    pub fn with_cancel(mut self, cancel: &'a CancelToken) -> Self {
        self.cancel = Some(cancel);
        self
    }

    pub fn with_sync_directory(mut self, sync_directory: bool) -> Self {
        self.sync_directory = sync_directory;
        self
    }
}

/// Result of a successful publish.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishOutcome {
    pub destination: PathBuf,
    pub bytes_written: u64,
}

/// Write `bytes` to a unique sibling temp, validate length, then replace
/// `destination`. On any pre-replace failure the previous destination bytes
/// stay intact and the temp is removed.
pub fn publish_bytes(
    destination: &Path,
    bytes: &[u8],
    options: PublishOptions<'_>,
) -> Result<PublishOutcome, DomainError> {
    publish_bytes_validated(destination, bytes, options, |_| Ok(()))
}

/// Like [`publish_bytes`], but run `validate` on the payload before the
/// destination is touched. A validation error leaves the destination unchanged.
pub fn publish_bytes_validated<F>(
    destination: &Path,
    bytes: &[u8],
    options: PublishOptions<'_>,
    validate: F,
) -> Result<PublishOutcome, DomainError>
where
    F: FnOnce(&[u8]) -> Result<(), DomainError>,
{
    validate(bytes)?;
    publish_writer(destination, options, |file| {
        file.write_all(bytes)
            .map_err(|error| map_io(ErrorCode::WriteFailed, destination, error))?;
        Ok(bytes.len() as u64)
    })
}

/// Create a unique sibling with `create_new`, let `write` fill it, sync, then
/// atomically replace `destination`. `write` must return the number of bytes
/// it believes it wrote; a mismatch is `SP-FMT-004` and aborts before replace.
pub fn publish_writer<F>(
    destination: &Path,
    options: PublishOptions<'_>,
    write: F,
) -> Result<PublishOutcome, DomainError>
where
    F: FnOnce(&mut File) -> Result<u64, DomainError>,
{
    check_cancel(options.cancel)?;
    require_parent(destination)?;

    let temp = create_temp_sibling(destination)?;
    let publish = (|| {
        check_cancel(options.cancel)?;
        let mut file = OpenOptions::new()
            .write(true)
            .open(&temp)
            .map_err(|error| map_io(ErrorCode::WriteFailed, &temp, error))?;
        let expected = write(&mut file)?;
        file.flush()
            .map_err(|error| map_io(ErrorCode::WriteFailed, &temp, error))?;
        file.sync_all()
            .map_err(|error| map_io(ErrorCode::WriteFailed, &temp, error))?;
        let metadata = file
            .metadata()
            .map_err(|error| map_io(ErrorCode::ReadFailed, &temp, error))?;
        drop(file);
        if metadata.len() != expected {
            return Err(integrity_error(
                destination,
                format!("expected {expected} bytes, temp has {}", metadata.len()),
            ));
        }
        check_cancel(options.cancel)?;
        replace_file(&temp, destination)?;
        if options.sync_directory {
            sync_parent_directory(destination)?;
        }
        Ok(PublishOutcome {
            destination: destination.to_path_buf(),
            bytes_written: expected,
        })
    })();

    match publish {
        Ok(outcome) => Ok(outcome),
        Err(error) => {
            let _ = fs::remove_file(&temp);
            Err(error)
        }
    }
}

/// Remove unfinished sibling temps for `destination` left by crashes or
/// killed writers. Returns the paths that were deleted. The live destination
/// is never touched.
pub fn recover_publish_orphans(destination: &Path) -> Result<Vec<PathBuf>, DomainError> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let prefix = temp_prefix(destination)?;
    let entries = fs::read_dir(parent).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            map_io(ErrorCode::NotFound, parent, error)
        } else {
            map_io(ErrorCode::ReadFailed, parent, error)
        }
    })?;
    let mut removed = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| map_io(ErrorCode::ReadFailed, parent, error))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(&prefix) {
            continue;
        }
        let path = entry.path();
        fs::remove_file(&path).map_err(|error| map_io(ErrorCode::WriteFailed, &path, error))?;
        removed.push(path);
    }
    Ok(removed)
}

/// Whether `path` looks like a publish temp for `destination`.
pub fn is_publish_temp(destination: &Path, path: &Path) -> bool {
    match temp_prefix(destination) {
        Ok(prefix) => path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(&prefix)),
        Err(_) => false,
    }
}

fn temp_prefix(destination: &Path) -> Result<String, DomainError> {
    let file_name = destination.file_name().ok_or_else(|| {
        DomainError::try_new(
            ErrorCode::InvalidArgument,
            "destination path has no file name",
        )
        .expect("message")
        .with_context(ContextKey::Path, destination.display().to_string())
        .expect("path")
    })?;
    Ok(format!("{}{TEMP_MARKER}", file_name.to_string_lossy()))
}

fn create_temp_sibling(destination: &Path) -> Result<PathBuf, DomainError> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let prefix = temp_prefix(destination)?;
    for _ in 0..32 {
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let name = format!("{prefix}{}-{seq}", std::process::id());
        let path = parent.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => {
                drop(file);
                return Ok(path);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(map_io(ErrorCode::WriteFailed, &path, error)),
        }
    }
    Err(DomainError::try_new(
        ErrorCode::WriteFailed,
        "could not allocate a unique publish temp",
    )
    .expect("message")
    .with_context(ContextKey::Path, destination.display().to_string())
    .expect("path"))
}

fn require_parent(destination: &Path) -> Result<(), DomainError> {
    let Some(parent) = destination.parent() else {
        return Ok(());
    };
    if parent.as_os_str().is_empty() {
        return Ok(());
    }
    match fs::metadata(parent) {
        Ok(meta) if meta.is_dir() => Ok(()),
        Ok(_) => Err(DomainError::try_new(
            ErrorCode::InvalidArgument,
            "destination parent is not a directory",
        )
        .expect("message")
        .with_context(ContextKey::Path, parent.display().to_string())
        .expect("path")),
        Err(error) => Err(map_io(ErrorCode::NotFound, parent, error)),
    }
}

fn check_cancel(cancel: Option<&CancelToken>) -> Result<(), DomainError> {
    match cancel {
        Some(token) => token.check(),
        None => Ok(()),
    }
}

fn integrity_error(path: &Path, actual: impl Into<String>) -> DomainError {
    DomainError::try_new(
        ErrorCode::IntegrityMismatch,
        "publish temp failed completeness validation",
    )
    .expect("message")
    .with_context(ContextKey::Path, path.display().to_string())
    .expect("path")
    .with_context(ContextKey::Expected, "complete temp payload")
    .expect("expected")
    .with_context(ContextKey::Actual, actual)
    .expect("actual")
}

fn map_io(code: ErrorCode, path: &Path, error: io::Error) -> DomainError {
    let code = match error.kind() {
        io::ErrorKind::NotFound => ErrorCode::NotFound,
        io::ErrorKind::PermissionDenied => ErrorCode::PermissionDenied,
        _ => code,
    };
    DomainError::try_new(code, error.to_string())
        .expect("io message")
        .with_context(ContextKey::Path, path.display().to_string())
        .expect("path")
        .with_source(error)
}

fn replace_file(temporary: &Path, destination: &Path) -> Result<(), DomainError> {
    replace_file_platform(temporary, destination)
        .map_err(|error| map_io(ErrorCode::WriteFailed, destination, error))
}

#[cfg(unix)]
fn replace_file_platform(temporary: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(temporary, destination)
}

#[cfg(windows)]
fn replace_file_platform(temporary: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
        fn GetLastError() -> u32;
    }

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

    let from: Vec<u16> = temporary
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let to: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // Matches AtomicFile.h: replace in place without a delete window.
    let ok = unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        Err(io::Error::from_raw_os_error(
            unsafe { GetLastError() } as i32
        ))
    } else {
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
fn replace_file_platform(temporary: &Path, destination: &Path) -> io::Result<()> {
    // Last-resort platforms: std rename with no promised replace semantics.
    fs::rename(temporary, destination)
}

fn sync_parent_directory(destination: &Path) -> Result<(), DomainError> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    match File::open(parent).and_then(|file| file.sync_all()) {
        Ok(()) => Ok(()),
        Err(error) => Err(DomainError::try_new(
            ErrorCode::WriteFailed,
            "atomic replace succeeded; directory sync failed",
        )
        .expect("message")
        .with_context(ContextKey::Path, destination.display().to_string())
        .expect("path")
        .with_context(ContextKey::Actual, "postpublish_directory_sync")
        .expect("actual")
        .with_source(error)),
    }
}
