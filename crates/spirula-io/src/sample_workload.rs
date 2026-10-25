//! CPU sample workload for P1-F08.
//!
//! The hashed fixture bytes are the measured input. The hashed config JSON
//! defines chunk size, iteration count, and the scoped resident resource
//! budget. Accounted bytes cover **only** the per-iteration work buffer
//! (`chunk_bytes`); fixture/config/report payloads and allocator metadata are
//! outside this counter, so the budget is not process-wide RSS enforcement.
//! Release builds must keep a real allocation + byte-consuming fold; opaque
//! `black_box` input and an observable checksum prevent the optimizer from
//! deleting the work.

use std::hint::black_box;
use std::io::{Cursor, Read};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;
use spirula_core::{AccountedBytesTracker, ContextKey, DomainError, ErrorCode};

use crate::hash::sha256_hex;

/// Process-wide sink so release builds cannot prove the checksum unused.
static CHECKSUM_SINK: AtomicU64 = AtomicU64::new(0);

/// Default measured fixture length / per-iteration chunk (bytes).
pub const SAMPLE_CHUNK_BYTES: usize = 8192;

/// Default iteration count from the sample config.
pub const SAMPLE_ITERATIONS: u64 = 8;

/// Accounted-memory `source` for this sample: only the work buffer is counted.
pub const SAMPLE_ACCOUNTED_SOURCE: &str = "explicit_counter:per_iteration_work_buffer_only";

/// Parsed sample configuration. Values come from the hashed config document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SampleConfig {
    pub feature: String,
    pub chunk_bytes: u64,
    pub iterations: u64,
    pub resource_budget_bytes: u64,
    pub phase_budget_ns: u64,
}

impl SampleConfig {
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.feature.trim().is_empty() {
            return Err(DomainError::try_new(
                ErrorCode::InvalidArgument,
                "sample config feature must be non-empty",
            )
            .expect("message")
            .with_context(ContextKey::Field, "feature")
            .expect("field"));
        }
        if self.chunk_bytes == 0 {
            return Err(DomainError::try_new(
                ErrorCode::InvalidArgument,
                "sample config chunk_bytes must be non-zero",
            )
            .expect("message")
            .with_context(ContextKey::Field, "chunk_bytes")
            .expect("field"));
        }
        if self.iterations == 0 {
            return Err(DomainError::try_new(
                ErrorCode::InvalidArgument,
                "sample config iterations must be non-zero",
            )
            .expect("message")
            .with_context(ContextKey::Field, "iterations")
            .expect("field"));
        }
        if self.resource_budget_bytes < self.chunk_bytes {
            return Err(DomainError::try_new(
                ErrorCode::InvalidArgument,
                "sample config resource_budget_bytes must cover one chunk",
            )
            .expect("message")
            .with_context(ContextKey::Field, "resource_budget_bytes")
            .expect("field")
            .with_context(ContextKey::Expected, self.chunk_bytes.to_string())
            .expect("expected")
            .with_context(ContextKey::Actual, self.resource_budget_bytes.to_string())
            .expect("actual"));
        }
        Ok(())
    }
}

/// Build the canonical sample fixture: `SAMPLE_CHUNK_BYTES` of non-zero pattern.
pub fn canonical_sample_fixture() -> Vec<u8> {
    (0..SAMPLE_CHUNK_BYTES)
        .map(|i| ((i.wrapping_mul(17)).wrapping_add(3)) as u8)
        .collect()
}

/// Canonical sample config JSON (also the hashed config payload).
pub fn canonical_sample_config_json() -> String {
    format!(
        "{{\n  \"feature\": \"P1-F08\",\n  \"chunk_bytes\": {SAMPLE_CHUNK_BYTES},\n  \"iterations\": {SAMPLE_ITERATIONS},\n  \"resource_budget_bytes\": {SAMPLE_CHUNK_BYTES},\n  \"phase_budget_ns\": 60000000000\n}}\n"
    )
}

/// Parse sample config from the same bytes that are hashed into provenance.
pub fn parse_sample_config(bytes: &[u8]) -> Result<SampleConfig, DomainError> {
    let text = std::str::from_utf8(bytes).map_err(|error| {
        DomainError::try_new(ErrorCode::MalformedRecord, "sample config is not UTF-8")
            .expect("message")
            .with_context(ContextKey::Format, "spirula-perf-sample-config")
            .expect("format")
            .with_source(error)
    })?;
    let value: Value = serde_json::from_str(text).map_err(|error| {
        DomainError::try_new(
            ErrorCode::MalformedRecord,
            "sample config JSON parse failed",
        )
        .expect("message")
        .with_context(ContextKey::Format, "spirula-perf-sample-config")
        .expect("format")
        .with_source(error)
    })?;
    let obj = value.as_object().ok_or_else(|| {
        DomainError::try_new(
            ErrorCode::MalformedRecord,
            "sample config must be a JSON object",
        )
        .expect("message")
        .with_context(ContextKey::Format, "spirula-perf-sample-config")
        .expect("format")
    })?;
    let feature = obj
        .get("feature")
        .and_then(Value::as_str)
        .ok_or_else(|| field_err("feature", "string"))?
        .to_owned();
    let chunk_bytes = obj
        .get("chunk_bytes")
        .and_then(Value::as_u64)
        .ok_or_else(|| field_err("chunk_bytes", "unsigned integer"))?;
    let iterations = obj
        .get("iterations")
        .and_then(Value::as_u64)
        .ok_or_else(|| field_err("iterations", "unsigned integer"))?;
    let resource_budget_bytes = obj
        .get("resource_budget_bytes")
        .and_then(Value::as_u64)
        .ok_or_else(|| field_err("resource_budget_bytes", "unsigned integer"))?;
    let phase_budget_ns = obj
        .get("phase_budget_ns")
        .and_then(Value::as_u64)
        .ok_or_else(|| field_err("phase_budget_ns", "unsigned integer"))?;
    let config = SampleConfig {
        feature,
        chunk_bytes,
        iterations,
        resource_budget_bytes,
        phase_budget_ns,
    };
    config.validate()?;
    Ok(config)
}

fn field_err(field: &str, expected: &str) -> DomainError {
    DomainError::try_new(
        ErrorCode::MalformedRecord,
        format!("sample config `{field}` must be {expected}"),
    )
    .expect("message")
    .with_context(ContextKey::Field, field)
    .expect("field")
}

/// Result of executing the measured allocate/read/free loop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkloadResult {
    pub wall_time: Duration,
    pub checksum: u64,
    pub expected_checksum: u64,
    pub accounted: spirula_core::AccountedMemory,
    pub fixture_sha256: String,
    pub config_sha256: String,
    pub config: SampleConfig,
}

/// Deterministic checksum for `iterations` full passes over `fixture`.
pub fn expected_workload_checksum(fixture: &[u8], iterations: u64) -> u64 {
    let one = fold_bytes(fixture);
    let mut total = 0u64;
    for _ in 0..iterations {
        total = total.wrapping_add(one);
    }
    total
}

/// Fold buffer bytes into a position-weighted checksum.
///
/// `black_box` on the slice and each byte keeps release builds from deleting
/// loads when the result feeds [`CHECKSUM_SINK`] and a returned value.
#[inline(never)]
fn fold_bytes(buf: &[u8]) -> u64 {
    let buf = black_box(buf);
    let mut acc = 0u64;
    for (i, b) in buf.iter().enumerate() {
        let byte = black_box(*b) as u64;
        acc = acc.wrapping_add(byte.wrapping_mul((i as u64).wrapping_add(1)));
    }
    black_box(acc)
}

/// Allocate a buffer and copy opaque fixture bytes through `Read` so the
/// release allocator/`memcpy` path remains.
#[inline(never)]
fn allocate_fixture_chunk(src: &[u8]) -> Vec<u8> {
    let src = black_box(src);
    let mut reader = Cursor::new(src);
    let mut buf = Vec::with_capacity(src.len());
    reader
        .read_to_end(&mut buf)
        .expect("in-memory Cursor read cannot fail");
    assert_eq!(black_box(buf.len()), src.len());
    black_box(buf)
}

/// Run the measured workload: hashed fixture is the input; hashed config
/// defines iterations/chunk/budget. Returns an observable checksum that must
/// match [`expected_workload_checksum`].
pub fn run_measured_workload(
    fixture_bytes: &[u8],
    config_bytes: &[u8],
) -> Result<WorkloadResult, DomainError> {
    let config = parse_sample_config(config_bytes)?;
    if fixture_bytes.len() as u64 != config.chunk_bytes {
        return Err(DomainError::try_new(
            ErrorCode::InvalidArgument,
            "fixture byte length must equal config chunk_bytes",
        )
        .expect("message")
        .with_context(ContextKey::Field, "fixture")
        .expect("field")
        .with_context(ContextKey::Expected, config.chunk_bytes.to_string())
        .expect("expected")
        .with_context(ContextKey::Actual, fixture_bytes.len().to_string())
        .expect("actual"));
    }

    let fixture_sha256 = sha256_hex(fixture_bytes);
    let config_sha256 = sha256_hex(config_bytes);
    let expected_checksum = expected_workload_checksum(fixture_bytes, config.iterations);

    let mut tracker = AccountedBytesTracker::new();
    let mut checksum = 0u64;
    let started = Instant::now();
    for _ in 0..config.iterations {
        tracker.preflight(config.chunk_bytes, config.resource_budget_bytes)?;
        let buf = allocate_fixture_chunk(fixture_bytes);
        tracker.allocate(config.chunk_bytes)?;
        let pass = fold_bytes(buf.as_slice());
        checksum = checksum.wrapping_add(pass);
        // Drop through black_box so the buffer is not proven dead before free.
        drop(black_box(buf));
        tracker.free(config.chunk_bytes)?;
    }
    let wall_time = started.elapsed();
    // Observable sinks: atomic store + returned compare keep the fold live.
    let checksum = black_box(checksum);
    CHECKSUM_SINK.fetch_xor(checksum, Ordering::SeqCst);
    if checksum != expected_checksum {
        return Err(DomainError::try_new(
            ErrorCode::IntegrityMismatch,
            format!("workload checksum {checksum} does not match expected {expected_checksum}"),
        )
        .expect("message")
        .with_context(ContextKey::Expected, expected_checksum.to_string())
        .expect("expected")
        .with_context(ContextKey::Actual, checksum.to_string())
        .expect("actual"));
    }

    let accounted = tracker.snapshot(SAMPLE_ACCOUNTED_SOURCE)?;
    accounted.check_budget(config.resource_budget_bytes)?;

    Ok(WorkloadResult {
        wall_time,
        checksum,
        expected_checksum,
        accounted,
        fixture_sha256,
        config_sha256,
        config,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defines_8192_by_8_under_resident_cap() {
        let config = parse_sample_config(canonical_sample_config_json().as_bytes()).unwrap();
        assert_eq!(config.chunk_bytes, 8192);
        assert_eq!(config.iterations, 8);
        assert_eq!(config.resource_budget_bytes, 8192);
    }

    #[test]
    fn fixture_hash_matches_executed_bytes() {
        let fixture = canonical_sample_fixture();
        let config_bytes = canonical_sample_config_json().into_bytes();
        let result = run_measured_workload(&fixture, &config_bytes).unwrap();
        assert_eq!(result.fixture_sha256, sha256_hex(&fixture));
        assert_eq!(result.config_sha256, sha256_hex(&config_bytes));
        assert_eq!(result.accounted.peak_bytes, 8192);
        assert_eq!(result.accounted.current_bytes, 0);
        assert_eq!(result.accounted.allocated_bytes, 8192 * 8);
        assert_eq!(result.accounted.source(), SAMPLE_ACCOUNTED_SOURCE);
        assert_eq!(result.checksum, result.expected_checksum);
        assert_ne!(result.checksum, 0);
    }

    #[test]
    fn resident_budget_allows_eight_reuse_cycles() {
        let fixture = canonical_sample_fixture();
        let config_bytes = canonical_sample_config_json().into_bytes();
        // resource_budget_bytes == chunk_bytes; eight allocate/free cycles must pass.
        run_measured_workload(&fixture, &config_bytes).unwrap();
    }

    #[test]
    fn mismatched_fixture_length_rejected() {
        let err =
            run_measured_workload(b"short", canonical_sample_config_json().as_bytes()).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidArgument);
    }
}
