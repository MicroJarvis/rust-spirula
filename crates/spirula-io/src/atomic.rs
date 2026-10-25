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
const LOCK_SUFFIX: &str = ".spirula-lock";
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

/// Exclusive publish/recovery lease for one destination basename.
///
/// Writers and [`recover_publish_orphans`] take this lease with `create_new`.
/// A live writer therefore blocks orphan recovery from deleting its temp, and
/// recovery blocks a concurrent writer from starting.
struct PublishLease {
    path: PathBuf,
    _file: File,
}

impl Drop for PublishLease {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
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

/// Create a unique sibling with `create_new`, let `write` fill the retained
/// handle, sync, then atomically replace `destination`. `write` must return
/// the number of bytes it believes it wrote; a mismatch is `SP-FMT-004` and
/// aborts before replace.
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
    let _lease = acquire_publish_lease(destination)?;

    let (temp, mut file) = create_temp_sibling(destination)?;
    let publish = (|| {
        check_cancel(options.cancel)?;
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

/// Remove unfinished sibling temps for `destination` left by crashed writers.
///
/// Acquires the same exclusive lease writers use. If a publisher still holds
/// the lease, returns `SP-SESS-003` and deletes nothing. Only names that match
/// the strict `{file}.spirula-tmp.{pid}.{seq}` pattern are removed; the live
/// destination and arbitrary similarly-prefixed files are left alone.
pub fn recover_publish_orphans(destination: &Path) -> Result<Vec<PathBuf>, DomainError> {
    let parent = parent_dir(destination);
    let _lease = acquire_publish_lease(destination)?;
    let file_name = destination_file_name(destination)?;
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
        if !is_strict_publish_temp_name(file_name, name) {
            continue;
        }
        let path = entry.path();
        fs::remove_file(&path).map_err(|error| map_io(ErrorCode::WriteFailed, &path, error))?;
        removed.push(path);
    }
    Ok(removed)
}

/// Whether `path` is a strict publish temp for `destination`.
pub fn is_publish_temp(destination: &Path, path: &Path) -> bool {
    match (
        destination_file_name(destination),
        path.file_name().and_then(|name| name.to_str()),
    ) {
        (Ok(file_name), Some(name)) => is_strict_publish_temp_name(file_name, name),
        _ => false,
    }
}

/// Parent directory used for sibling temps, locks, and directory sync.
///
/// A bare relative file such as `config.json` reports `parent() == Some("")`;
/// that empty path is normalized to `.`.
pub fn parent_dir(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

fn destination_file_name(destination: &Path) -> Result<&str, DomainError> {
    destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            DomainError::try_new(
                ErrorCode::InvalidArgument,
                "destination path has no UTF-8 file name",
            )
            .expect("message")
            .with_context(ContextKey::Path, destination.display().to_string())
            .expect("path")
        })
}

fn is_strict_publish_temp_name(destination_file_name: &str, candidate: &str) -> bool {
    let Some(rest) = candidate.strip_prefix(destination_file_name) else {
        return false;
    };
    let Some(rest) = rest.strip_prefix(TEMP_MARKER) else {
        return false;
    };
    let mut parts = rest.split('.');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(pid), Some(seq), None) => is_decimal_token(pid) && is_decimal_token(seq),
        _ => false,
    }
}

fn is_decimal_token(token: &str) -> bool {
    !token.is_empty() && token.chars().all(|c| c.is_ascii_digit())
}

fn lock_path(destination: &Path) -> Result<PathBuf, DomainError> {
    let file_name = destination_file_name(destination)?;
    Ok(parent_dir(destination).join(format!("{file_name}{LOCK_SUFFIX}")))
}

fn acquire_publish_lease(destination: &Path) -> Result<PublishLease, DomainError> {
    let path = lock_path(destination)?;
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(file) => Ok(PublishLease { path, _file: file }),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Err(DomainError::try_new(
            ErrorCode::CommandInFlight,
            "another publish or orphan recovery holds the destination lease",
        )
        .expect("message")
        .with_context(ContextKey::Path, destination.display().to_string())
        .expect("path")
        .with_context(ContextKey::Actual, "publish_lease_held")
        .expect("actual")),
        Err(error) => Err(map_io(ErrorCode::WriteFailed, &path, error)),
    }
}

fn create_temp_sibling(destination: &Path) -> Result<(PathBuf, File), DomainError> {
    let parent = parent_dir(destination);
    let file_name = destination_file_name(destination)?;
    for _ in 0..32 {
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let name = format!("{file_name}{TEMP_MARKER}{}.{seq}", std::process::id());
        let path = parent.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
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
    let parent = parent_dir(destination);
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
    let parent = parent_dir(destination);
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
