//! Memory / count budgets and typed error helpers for COLMAP IO.

use spirula_core::{CancelToken, ContextKey, DomainError, ErrorCode};

use crate::cancel_checkpoint::CancelCheckpoint;

/// Cap on a single COLMAP table file (cameras / images / points3D).
pub const MAX_COLMAP_FILE_BYTES: u64 = 512 * 1024 * 1024;
/// Maximum camera records.
pub const MAX_CAMERAS: u64 = 1_000_000;
/// Maximum image / frame records.
pub const MAX_IMAGES: u64 = 2_000_000;
/// Maximum 3D points.
pub const MAX_POINTS: u64 = 50_000_000;
/// Maximum 2D observations across all images.
pub const MAX_OBSERVATIONS: u64 = 200_000_000;
/// Maximum track elements across all points.
pub const MAX_TRACK_ELEMS: u64 = 200_000_000;
/// Cumulative in-memory parse budget for a single table (Vec element bytes).
pub const MAX_PARSE_MEMORY_BYTES: u64 = 512 * 1024 * 1024;
/// Cumulative live-structure budget across the whole load pipeline (tables +
/// builds / names / frames / splits / BTree sets). Independent of per-table
/// [`MAX_PARSE_MEMORY_BYTES`] so three maxed tables cannot silently sum past
/// this ceiling.
pub const MAX_LOAD_MEMORY_BYTES: u64 = 512 * 1024 * 1024;
/// Conservative per-entry BTreeMap/BTreeSet overhead on 64-bit (node pointers +
/// layout). Upper-bounds typical leaf fanout accounting; charged in addition
/// to `size_of::<(K,V)>()` / key bytes so clone/map/seen costs are not free.
pub const BTREE_ENTRY_OVERHEAD: u64 = 64;
/// Minimum bytes for one `points3D.bin` record excluding track payload.
pub const MIN_POINT3D_RECORD_BYTES: u64 = 8 + 24 + 3 + 8 + 8;
/// Bytes per binary observation (`x`/`y` f64 + point3D_id u64).
pub const OBSERVATION_RECORD_BYTES: u64 = 24;
/// Bytes per binary track element (2×i32).
pub const TRACK_ELEM_BYTES: u64 = 8;

pub(crate) fn err(code: ErrorCode, field: &str, msg: impl Into<String>) -> DomainError {
    DomainError::try_new(code, msg)
        .expect("msg")
        .with_context(ContextKey::Field, field)
        .expect("field")
}

pub(crate) fn err_path(code: ErrorCode, path: &str, msg: impl Into<String>) -> DomainError {
    DomainError::try_new(code, msg)
        .expect("msg")
        .with_context(ContextKey::Path, path)
        .expect("path")
}

pub(crate) fn ensure_count(count: u64, max: u64, field: &str) -> Result<(), DomainError> {
    if count > max {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            field,
            format!("{field} count {count} exceeds budget {max}"),
        )
        .with_context(ContextKey::Expected, format!("<= {max}"))
        .expect("expected")
        .with_context(ContextKey::Actual, count.to_string())
        .expect("actual"));
    }
    Ok(())
}

pub(crate) fn checked_mul_u64(a: u64, b: u64, field: &str) -> Result<u64, DomainError> {
    a.checked_mul(b).ok_or_else(|| {
        err(
            ErrorCode::ValueOutOfRange,
            field,
            format!("{field} size overflow ({a} * {b})"),
        )
    })
}

pub(crate) fn ensure_remaining(remaining: u64, need: u64, field: &str) -> Result<(), DomainError> {
    if remaining < need {
        return Err(err(
            ErrorCode::MalformedRecord,
            field,
            format!("{field} needs {need} bytes, only {remaining} remain"),
        ));
    }
    Ok(())
}

/// Cumulative in-memory budget (file bytes + Vec/BTree/clone/string growth).
#[derive(Debug)]
pub(crate) struct ParseMem {
    used: u64,
    cap: u64,
}

impl Default for ParseMem {
    fn default() -> Self {
        Self::new()
    }
}

impl ParseMem {
    /// Per-table parse budget ([`MAX_PARSE_MEMORY_BYTES`]).
    pub fn new() -> Self {
        Self::with_cap(MAX_PARSE_MEMORY_BYTES)
    }

    /// Explicit cap (tests inject small budgets for cross-table / emit OOM).
    pub fn with_cap(cap: u64) -> Self {
        Self { used: 0, cap }
    }

    pub fn used(&self) -> u64 {
        self.used
    }

    /// Charge `bytes` against the cumulative budget.
    pub fn charge(&mut self, bytes: u64, field: &str) -> Result<(), DomainError> {
        let next = self.used.saturating_add(bytes);
        if next > self.cap {
            return Err(err(
                ErrorCode::ValueOutOfRange,
                field,
                format!("{field} parse memory {next} exceeds budget {}", self.cap),
            ));
        }
        self.used = next;
        Ok(())
    }

    pub fn charge_elems<T>(&mut self, n: usize, field: &str) -> Result<(), DomainError> {
        let elem = (std::mem::size_of::<T>() as u64).max(1);
        let add = checked_mul_u64(n as u64, elem, field)?;
        self.charge(add, field)
    }

    pub fn charge_str_bytes(&mut self, len: usize, field: &str) -> Result<(), DomainError> {
        self.charge(len as u64, field)
    }

    /// Charge `n` BTreeMap/BTreeSet entries: key/value (or key) bytes plus
    /// [`BTREE_ENTRY_OVERHEAD`] per entry.
    pub fn charge_btree_entries<T>(&mut self, n: usize, field: &str) -> Result<(), DomainError> {
        self.charge_elems::<T>(n, field)?;
        let overhead = checked_mul_u64(n as u64, BTREE_ENTRY_OVERHEAD, field)?;
        self.charge(overhead, field)
    }
}

/// Fallible reserve that never panics on huge counts; enforces parse memory budget.
pub(crate) fn try_reserve_elems<T>(
    vec: &mut Vec<T>,
    additional: usize,
    mem: &mut ParseMem,
    field: &str,
) -> Result<(), DomainError> {
    mem.charge_elems::<T>(additional, field)?;
    vec.try_reserve(additional).map_err(|_| {
        err(
            ErrorCode::ValueOutOfRange,
            field,
            format!("{field} reservation of {additional} elements failed"),
        )
    })?;
    Ok(())
}

/// Charge amortized capacity growth **before** a `push` that would reallocate.
///
/// Std `Vec` grows empty→≥4 then ≈2×. Charging only `1` elem per push under-bounds
/// the live allocation (capacity slots). Call immediately before every `push`.
pub(crate) fn prepare_vec_push<T>(
    vec: &mut Vec<T>,
    mem: &mut ParseMem,
    field: &str,
) -> Result<(), DomainError> {
    if vec.len() < vec.capacity() {
        return Ok(());
    }
    let old = vec.capacity();
    // Conservative match for std amortized growth on large `T` (XmlNode, PlyElement, …).
    let additional = if old == 0 { 4 } else { old };
    try_reserve_elems(vec, additional, mem, field)?;
    // If the allocator rounded higher than our doubling guess, pay the delta
    // before the subsequent push observes the larger capacity.
    let grew = vec.capacity().saturating_sub(old);
    if grew > additional {
        mem.charge_elems::<T>(grew - additional, field)?;
    }
    Ok(())
}

/// Shared cancel checkpoint wrapper for parse loops.
pub(crate) struct ParseCancel<'a> {
    pub cp: CancelCheckpoint<'a>,
}

impl<'a> ParseCancel<'a> {
    pub fn new(cancel: Option<&'a CancelToken>) -> Self {
        Self {
            cp: CancelCheckpoint::new(cancel),
        }
    }

    pub fn tick(&mut self, units: usize) -> Result<(), DomainError> {
        self.cp.tick(units)
    }

    pub fn poll_now(&mut self) -> Result<(), DomainError> {
        self.cp.poll_now()
    }
}
