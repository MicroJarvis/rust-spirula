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

use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions, TryLockError};
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

/// Exclusive publish/recovery lease for one canonical destination.
///
/// The lease is bound to the destination identity captured at acquire time
/// (canonical parent + file name). Publication APIs on this type always write
/// that destination; free `*_on_lease` helpers reject a mismatched path.
///
/// The lock file inode is created once and left on disk. Coordination uses
/// [`File::try_lock`] (advisory). Closing the handle releases the lock even if
/// the process is killed, so recovery is not wedged by a stale lock file.
/// The lock file is never unlinked while the lease is held.
pub struct PublishLease {
    destination: PathBuf,
    file: File,
}

impl Drop for PublishLease {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

impl PublishLease {
    /// Canonical destination this lease authorizes.
    pub fn destination(&self) -> &Path {
        &self.destination
    }

    /// Replace this lease's destination via a streaming writer.
    pub fn publish_writer<F>(
        &self,
        options: PublishOptions<'_>,
        write: F,
    ) -> Result<PublishOutcome, DomainError>
    where
        F: FnOnce(&mut File) -> Result<u64, DomainError>,
    {
        publish_writer_bound(self, options, write)
    }

    /// Validate then replace this lease's destination.
    pub fn publish_bytes_validated<F>(
        &self,
        bytes: &[u8],
        options: PublishOptions<'_>,
        validate: F,
    ) -> Result<PublishOutcome, DomainError>
    where
        F: FnOnce(&[u8]) -> Result<(), DomainError>,
    {
        validate(bytes)?;
        self.publish_writer(options, |file| {
            file.write_all(bytes)
                .map_err(|error| map_io(ErrorCode::WriteFailed, &self.destination, error))?;
            Ok(bytes.len() as u64)
        })
    }

    /// Atomically create this lease's destination only if absent (`hard_link`).
    ///
    /// Holds this destination's lease for the entire temp/write/link lifecycle
    /// so orphan recovery cannot delete a live no-clobber temp.
    pub fn publish_bytes_if_absent(
        &self,
        bytes: &[u8],
        options: PublishOptions<'_>,
    ) -> Result<Option<PublishOutcome>, DomainError> {
        publish_bytes_if_absent_bound(self, bytes, options)
    }

    fn require_destination(&self, destination: &Path) -> Result<(), DomainError> {
        let expected = canonical_destination(destination)?;
        if expected != self.destination {
            return Err(DomainError::try_new(
                ErrorCode::InvalidArgument,
                "publish lease does not authorize this destination",
            )
            .expect("message")
            .with_context(ContextKey::Path, destination.display().to_string())
            .expect("path")
            .with_context(ContextKey::Expected, self.destination.display().to_string())
            .expect("expected")
            .with_context(ContextKey::Actual, "lease_destination_mismatch")
            .expect("actual"));
        }
        Ok(())
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
    let lease = acquire_publish_lease(destination)?;
    lease.publish_bytes_validated(bytes, options, validate)
}

/// Validate and publish while `lease` for `destination` is already held.
///
/// Rejects a lease bound to a different destination (`SP-INP-001` /
/// `lease_destination_mismatch`). Prefer [`PublishLease::publish_bytes_validated`].
pub fn publish_bytes_validated_on_lease<F>(
    destination: &Path,
    bytes: &[u8],
    options: PublishOptions<'_>,
    lease: &PublishLease,
    validate: F,
) -> Result<PublishOutcome, DomainError>
where
    F: FnOnce(&[u8]) -> Result<(), DomainError>,
{
    lease.require_destination(destination)?;
    lease.publish_bytes_validated(bytes, options, validate)
}

/// Atomically create `destination` only if it does not already exist.
///
/// Acquires the destination lease for the full temp/write/`hard_link`
/// lifecycle. An existing destination yields `Ok(None)` without replacing
/// bytes. Prefer holding an already-acquired [`PublishLease`] and calling
/// [`PublishLease::publish_bytes_if_absent`] when composing multi-destination
/// transactions (for example config + `.bak`) so lock order stays explicit.
pub fn publish_bytes_if_absent(
    destination: &Path,
    bytes: &[u8],
    options: PublishOptions<'_>,
) -> Result<Option<PublishOutcome>, DomainError> {
    let lease = acquire_publish_lease(destination)?;
    lease.publish_bytes_if_absent(bytes, options)
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
    let lease = acquire_publish_lease(destination)?;
    lease.publish_writer(options, write)
}

/// Like [`publish_writer`], but uses an already-held destination lease.
///
/// Rejects a lease bound to a different destination (`SP-INP-001` /
/// `lease_destination_mismatch`). Prefer [`PublishLease::publish_writer`].
pub fn publish_writer_on_lease<F>(
    destination: &Path,
    options: PublishOptions<'_>,
    lease: &PublishLease,
    write: F,
) -> Result<PublishOutcome, DomainError>
where
    F: FnOnce(&mut File) -> Result<u64, DomainError>,
{
    lease.require_destination(destination)?;
    lease.publish_writer(options, write)
}

/// Remove unfinished sibling temps for `destination` left by crashed writers.
///
/// Acquires the same advisory lease writers use. A live writer blocks recovery
/// (`SP-SESS-003`). After a killed writer the OS releases the advisory lock
/// with the process, so recovery can proceed even though the lock inode still
/// exists. Only names that match the strict
/// `{file}.spirula-tmp.{pid}.{seq}` pattern with the destination's native
/// [`OsStr`] identity are removed.
pub fn recover_publish_orphans(destination: &Path) -> Result<Vec<PathBuf>, DomainError> {
    let lease = acquire_publish_lease(destination)?;
    let parent = parent_dir(lease.destination());
    let file_name = destination_file_name(lease.destination())?;
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
        if !is_strict_publish_temp_name(file_name, &name) {
            continue;
        }
        let path = entry.path();
        fs::remove_file(&path).map_err(|error| map_io(ErrorCode::WriteFailed, &path, error))?;
        removed.push(path);
    }
    Ok(removed)
}

/// Acquire the exclusive advisory lease for `destination`.
///
/// The returned lease is bound to the canonical destination identity
/// (canonicalized parent + file name). Publication through that lease always
/// targets that identity.
pub fn acquire_publish_lease(destination: &Path) -> Result<PublishLease, DomainError> {
    require_parent(destination)?;
    let destination = canonical_destination(destination)?;
    let path = lock_path_for(&destination)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| map_io(ErrorCode::WriteFailed, &path, error))?;
    match file.try_lock() {
        Ok(()) => Ok(PublishLease { destination, file }),
        Err(TryLockError::WouldBlock) => Err(DomainError::try_new(
            ErrorCode::CommandInFlight,
            "another publish or orphan recovery holds the destination lease",
        )
        .expect("message")
        .with_context(ContextKey::Path, destination.display().to_string())
        .expect("path")
        .with_context(ContextKey::Actual, "publish_lease_held")
        .expect("actual")),
        Err(TryLockError::Error(error)) => Err(map_io(ErrorCode::WriteFailed, &path, error)),
    }
}

/// Whether `path` is a strict publish temp for `destination`.
pub fn is_publish_temp(destination: &Path, path: &Path) -> bool {
    match (destination_file_name(destination), path.file_name()) {
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

fn publish_writer_bound<F>(
    lease: &PublishLease,
    options: PublishOptions<'_>,
    write: F,
) -> Result<PublishOutcome, DomainError>
where
    F: FnOnce(&mut File) -> Result<u64, DomainError>,
{
    let destination = lease.destination();
    check_cancel(options.cancel)?;
    require_parent(destination)?;

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

fn publish_bytes_if_absent_bound(
    lease: &PublishLease,
    bytes: &[u8],
    options: PublishOptions<'_>,
) -> Result<Option<PublishOutcome>, DomainError> {
    let destination = lease.destination();
    check_cancel(options.cancel)?;
    require_parent(destination)?;
    let (temp, mut file) = create_temp_sibling(destination)?;
    let publish = (|| {
        check_cancel(options.cancel)?;
        file.write_all(bytes)
            .map_err(|error| map_io(ErrorCode::WriteFailed, destination, error))?;
        file.flush()
            .map_err(|error| map_io(ErrorCode::WriteFailed, &temp, error))?;
        file.sync_all()
            .map_err(|error| map_io(ErrorCode::WriteFailed, &temp, error))?;
        let metadata = file
            .metadata()
            .map_err(|error| map_io(ErrorCode::ReadFailed, &temp, error))?;
        drop(file);
        if metadata.len() != bytes.len() as u64 {
            return Err(integrity_error(
                destination,
                format!(
                    "expected {} bytes, temp has {}",
                    bytes.len(),
                    metadata.len()
                ),
            ));
        }
        check_cancel(options.cancel)?;
        match fs::hard_link(&temp, destination) {
            Ok(()) => {
                let _ = fs::remove_file(&temp);
                if options.sync_directory {
                    sync_parent_directory(destination)?;
                }
                Ok(Some(PublishOutcome {
                    destination: destination.to_path_buf(),
                    bytes_written: bytes.len() as u64,
                }))
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let _ = fs::remove_file(&temp);
                Ok(None)
            }
            Err(error) => {
                let _ = fs::remove_file(&temp);
                Err(map_io(ErrorCode::WriteFailed, destination, error))
            }
        }
    })();

    match publish {
        Ok(outcome) => Ok(outcome),
        Err(error) => {
            let _ = fs::remove_file(&temp);
            Err(error)
        }
    }
}

fn canonical_destination(destination: &Path) -> Result<PathBuf, DomainError> {
    let file_name = destination_file_name(destination)?;
    let parent = parent_dir(destination);
    let parent = fs::canonicalize(parent).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            map_io(ErrorCode::NotFound, parent, error)
        } else {
            map_io(ErrorCode::ReadFailed, parent, error)
        }
    })?;
    Ok(parent.join(file_name))
}

fn destination_file_name(destination: &Path) -> Result<&OsStr, DomainError> {
    destination.file_name().ok_or_else(|| {
        DomainError::try_new(
            ErrorCode::InvalidArgument,
            "destination path has no file name",
        )
        .expect("message")
        .with_context(ContextKey::Path, destination.display().to_string())
        .expect("path")
    })
}

fn publish_temp_name(file_name: &OsStr, pid: u32, seq: u64) -> OsString {
    let mut name = OsString::from(file_name);
    name.push(TEMP_MARKER);
    name.push(pid.to_string());
    name.push(".");
    name.push(seq.to_string());
    name
}

fn publish_lock_name(file_name: &OsStr) -> OsString {
    let mut name = OsString::from(file_name);
    name.push(LOCK_SUFFIX);
    name
}

#[allow(unsafe_code)] // remainder is a suffix of a valid encoded OsStr after a whole-OsStr prefix
fn strip_os_prefix<'a>(value: &'a OsStr, prefix: &OsStr) -> Option<&'a OsStr> {
    let value_bytes = value.as_encoded_bytes();
    let prefix_bytes = prefix.as_encoded_bytes();
    if !value_bytes.starts_with(prefix_bytes) {
        return None;
    }
    // Prefixes are whole `OsStr` values we concatenated, so the remainder is a
    // valid encoded `OsStr` on every supported platform.
    Some(unsafe { OsStr::from_encoded_bytes_unchecked(&value_bytes[prefix_bytes.len()..]) })
}

fn is_strict_publish_temp_name(destination_file_name: &OsStr, candidate: &OsStr) -> bool {
    let Some(rest) = strip_os_prefix(candidate, destination_file_name) else {
        return false;
    };
    let Some(rest) = strip_os_prefix(rest, OsStr::new(TEMP_MARKER)) else {
        return false;
    };
    let Some(rest) = rest.to_str() else {
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

fn lock_path_for(canonical_destination: &Path) -> Result<PathBuf, DomainError> {
    let file_name = destination_file_name(canonical_destination)?;
    Ok(parent_dir(canonical_destination).join(publish_lock_name(file_name)))
}

fn create_temp_sibling(destination: &Path) -> Result<(PathBuf, File), DomainError> {
    let parent = parent_dir(destination);
    let file_name = destination_file_name(destination)?;
    for _ in 0..32 {
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let name = publish_temp_name(file_name, std::process::id(), seq);
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
#[allow(unsafe_code)] // Windows MoveFileExW FFI; matches AtomicFile.h replace flags
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
