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

/// Fallible reserve that never panics on huge counts; enforces parse memory budget.
pub(crate) fn try_reserve_elems<T>(
    vec: &mut Vec<T>,
    additional: usize,
    mem_used: &mut u64,
    field: &str,
) -> Result<(), DomainError> {
    let elem = std::mem::size_of::<T>() as u64;
    let add_bytes = checked_mul_u64(additional as u64, elem.max(1), field)?;
    let next = mem_used.saturating_add(add_bytes);
    if next > MAX_PARSE_MEMORY_BYTES {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            field,
            format!("{field} parse memory {next} exceeds budget {MAX_PARSE_MEMORY_BYTES}"),
        ));
    }
    vec.try_reserve(additional).map_err(|_| {
        err(
            ErrorCode::ValueOutOfRange,
            field,
            format!("{field} reservation of {additional} elements failed"),
        )
    })?;
    *mem_used = next;
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
