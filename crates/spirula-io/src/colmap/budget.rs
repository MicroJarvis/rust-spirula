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

/// Upper bound on `Vec` capacity after a growth that makes room for one more
/// element when `len == capacity`.
///
/// Covers std amortized doubling (`max(2×old, len+1)`, empty→≥8 to match the
/// largest `MIN_NON_ZERO_CAP` for 1-byte `T`) plus ≤2× size-class slack from the
/// allocator. All of this is charged **before** `try_reserve` so we never pay
/// an unexpected capacity delta after the allocation has already happened.
pub(crate) fn max_vec_grow_capacity(old_cap: usize, len: usize) -> usize {
    let needed = len.saturating_add(1);
    let amortized = if old_cap == 0 {
        needed.max(8)
    } else {
        old_cap.saturating_mul(2).max(needed)
    };
    amortized.saturating_mul(2)
}

/// Charge amortized capacity growth **before** a `push` that would reallocate.
///
/// Charges [`max_vec_grow_capacity`] − current capacity up front, then
/// `try_reserve(1)`. No post-allocation top-up. Call immediately before every
/// `push` that may grow.
pub(crate) fn prepare_vec_push<T>(
    vec: &mut Vec<T>,
    mem: &mut ParseMem,
    field: &str,
) -> Result<(), DomainError> {
    if vec.len() < vec.capacity() {
        return Ok(());
    }
    let old = vec.capacity();
    let max_cap = max_vec_grow_capacity(old, vec.len());
    let charge_n = max_cap.saturating_sub(old).max(1);
    mem.charge_elems::<T>(charge_n, field)?;
    vec.try_reserve(1).map_err(|_| {
        err(
            ErrorCode::ValueOutOfRange,
            field,
            format!("{field} reservation for push failed"),
        )
    })?;
    debug_assert!(
        vec.capacity() <= max_cap || vec.capacity() <= old.saturating_add(charge_n),
        "prepare_vec_push: capacity {} exceeded pre-charged max {}",
        vec.capacity(),
        max_cap
    );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_vec_push_charges_max_growth_before_reserve() {
        let mut mem = ParseMem::with_cap(1 << 20);
        let mut v: Vec<u64> = Vec::new();
        // Empty→growth: must succeed without any post-alloc charge path.
        prepare_vec_push(&mut v, &mut mem, "t").unwrap();
        v.push(1);
        assert!(v.capacity() >= 1);
        assert!(v.capacity() <= max_vec_grow_capacity(0, 0));

        // Fill to capacity so the next prepare grows again.
        while v.len() < v.capacity() {
            v.push(v.len() as u64);
        }
        let old = v.capacity();
        let used_before = mem.used();
        prepare_vec_push(&mut v, &mut mem, "t").unwrap();
        let charged = mem.used() - used_before;
        let max_cap = max_vec_grow_capacity(old, v.len());
        let expected = (max_cap.saturating_sub(old).max(1) as u64)
            * (std::mem::size_of::<u64>() as u64).max(1);
        assert_eq!(charged, expected, "charge equals max pre-reserve bound");
        v.push(99);
        assert!(v.capacity() <= max_cap);
    }

    #[test]
    fn prepare_vec_push_rejects_when_max_growth_exceeds_budget() {
        let mut v: Vec<u32> = Vec::new();
        // Cap below empty-grow charge (8 × size_of::<u32> = 32).
        let mut mem = ParseMem::with_cap(16);
        let e = prepare_vec_push(&mut v, &mut mem, "t").unwrap_err();
        assert_eq!(e.code(), ErrorCode::ValueOutOfRange);
        assert!(v.capacity() == 0, "must not allocate after budget reject");
    }
}
