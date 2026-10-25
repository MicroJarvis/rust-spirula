//! Versioned resource / performance report contracts (P1-F08).
//!
//! Accounted allocation counters are distinct from OS process peaks and from
//! device-wide GPU memory. Metrics that are not measured must carry an explicit
//! unavailable reason — never invent a numeric stand-in.

use crate::{ContextKey, DomainError, ErrorCode};

/// Document schema id written into every report JSON object.
pub const PERF_REPORT_SCHEMA: &str = "spirula-perf-report";

/// Current schema version. Unsupported versions fail closed on load.
pub const PERF_REPORT_VERSION: u32 = 1;

/// A metric that was deliberately not measured.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnavailableMetric {
    reason: String,
}

impl UnavailableMetric {
    /// Reject blank reasons so callers cannot hide a missing measurement.
    pub fn try_new(reason: impl Into<String>) -> Result<Self, DomainError> {
        let reason = reason.into();
        require_non_empty(&reason, "unavailable.reason")?;
        Ok(Self { reason })
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        Self::try_new(self.reason.clone()).map(|_| ())
    }
}

/// Application-accounted host bytes. Values come from an explicit counter the
/// caller updates on allocate/free — not from OS RSS or device queries.
///
/// The `source` string must describe **what** is counted. A sample may count
/// only the per-iteration work buffer while fixture/config/report payloads and
/// allocator metadata remain outside the counter; [`Self::check_budget`] then
/// caps that scoped live/peak only and must not be read as process-wide RSS
/// enforcement.
///
/// Invariant: `current_bytes <= peak_bytes <= allocated_bytes`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountedMemory {
    /// Cumulative bytes requested through the accounted path (volume counter).
    pub allocated_bytes: u64,
    /// Bytes still held by the accounted path.
    pub current_bytes: u64,
    /// High-water mark of `current_bytes`.
    pub peak_bytes: u64,
    /// Scope + method (for example
    /// `explicit_counter:per_iteration_work_buffer_only`).
    source: String,
}

impl AccountedMemory {
    pub fn try_new(
        allocated_bytes: u64,
        current_bytes: u64,
        peak_bytes: u64,
        source: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let source = source.into();
        require_non_empty(&source, "memory.accounted.source")?;
        // current <= peak <= cumulative allocated
        if current_bytes > peak_bytes {
            return Err(range_err(
                "memory.accounted.current_bytes",
                "accounted current_bytes cannot exceed peak_bytes",
                peak_bytes,
                current_bytes,
            ));
        }
        if peak_bytes > allocated_bytes {
            return Err(range_err(
                "memory.accounted.peak_bytes",
                "accounted peak_bytes cannot exceed allocated_bytes",
                allocated_bytes,
                peak_bytes,
            ));
        }
        Ok(Self {
            allocated_bytes,
            current_bytes,
            peak_bytes,
            source,
        })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// Re-check invariants after public field mutation.
    pub fn validate(&self) -> Result<(), DomainError> {
        Self::try_new(
            self.allocated_bytes,
            self.current_bytes,
            self.peak_bytes,
            self.source.clone(),
        )
        .map(|_| ())
    }

    /// Fail when scoped live current/peak exceeds a resident-memory byte budget.
    ///
    /// Cumulative `allocated_bytes` is a volume counter and is **not** compared
    /// to this limit (repeated allocate/free under the same live cap is allowed).
    /// This is not process-wide RSS enforcement — only the bytes described by
    /// [`Self::source`] are checked.
    pub fn check_budget(&self, limit_bytes: u64) -> Result<(), DomainError> {
        if self.current_bytes > limit_bytes {
            return Err(allocation_budget_err(
                "memory.accounted.current_bytes",
                format!(
                    "accounted current_bytes {} exceeds scoped resource budget_bytes {limit_bytes}",
                    self.current_bytes
                ),
                limit_bytes,
                self.current_bytes,
            ));
        }
        if self.peak_bytes > limit_bytes {
            return Err(allocation_budget_err(
                "memory.accounted.peak_bytes",
                format!(
                    "accounted peak_bytes {} exceeds scoped resource budget_bytes {limit_bytes}",
                    self.peak_bytes
                ),
                limit_bytes,
                self.peak_bytes,
            ));
        }
        Ok(())
    }
}

/// Mutable counter used by samples to produce [`AccountedMemory`].
///
/// Updates use checked arithmetic and leave the counter unchanged on error.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccountedBytesTracker {
    allocated_bytes: u64,
    current_bytes: u64,
    peak_bytes: u64,
}

impl AccountedBytesTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn allocated_bytes(&self) -> u64 {
        self.allocated_bytes
    }

    pub fn current_bytes(&self) -> u64 {
        self.current_bytes
    }

    pub fn peak_bytes(&self) -> u64 {
        self.peak_bytes
    }

    /// Reject an allocation that would push **live** current/peak past
    /// `limit_bytes` without mutating the counter.
    ///
    /// Cumulative allocated volume is not a capacity check: allocate/free
    /// reuse under the same resident cap is allowed.
    pub fn preflight(&self, bytes: u64, limit_bytes: u64) -> Result<(), DomainError> {
        let next_current = self.current_bytes.checked_add(bytes).ok_or_else(|| {
            DomainError::try_new(
                ErrorCode::ValueOutOfRange,
                "accounted current_bytes would overflow u64",
            )
            .expect("message")
            .with_context(ContextKey::Field, "memory.accounted.current_bytes")
            .expect("field")
        })?;
        let next_peak = self.peak_bytes.max(next_current);
        if next_current > limit_bytes || next_peak > limit_bytes {
            return Err(allocation_budget_err(
                "memory.resource_budget_bytes",
                format!(
                    "preflight accounted allocation of {bytes} bytes would exceed scoped resident budget {limit_bytes}"
                ),
                limit_bytes,
                next_peak.max(next_current),
            ));
        }
        // Still require cumulative volume not to overflow u64, but that is not
        // the resident budget.
        let _ = self.allocated_bytes.checked_add(bytes).ok_or_else(|| {
            DomainError::try_new(
                ErrorCode::ValueOutOfRange,
                "accounted allocated_bytes would overflow u64",
            )
            .expect("message")
            .with_context(ContextKey::Field, "memory.accounted.allocated_bytes")
            .expect("field")
        })?;
        Ok(())
    }

    /// Record an allocation of `bytes`. On overflow the counter is unchanged.
    pub fn allocate(&mut self, bytes: u64) -> Result<(), DomainError> {
        let new_allocated = self.allocated_bytes.checked_add(bytes).ok_or_else(|| {
            DomainError::try_new(
                ErrorCode::ValueOutOfRange,
                "accounted allocated_bytes overflow",
            )
            .expect("message")
            .with_context(ContextKey::Field, "memory.accounted.allocated_bytes")
            .expect("field")
            .with_context(ContextKey::Actual, bytes.to_string())
            .expect("actual")
        })?;
        let new_current = self.current_bytes.checked_add(bytes).ok_or_else(|| {
            DomainError::try_new(
                ErrorCode::ValueOutOfRange,
                "accounted current_bytes overflow",
            )
            .expect("message")
            .with_context(ContextKey::Field, "memory.accounted.current_bytes")
            .expect("field")
            .with_context(ContextKey::Actual, bytes.to_string())
            .expect("actual")
        })?;
        self.allocated_bytes = new_allocated;
        self.current_bytes = new_current;
        self.peak_bytes = self.peak_bytes.max(new_current);
        Ok(())
    }

    /// Record a free of `bytes`. On underflow the counter is unchanged.
    pub fn free(&mut self, bytes: u64) -> Result<(), DomainError> {
        let new_current = self.current_bytes.checked_sub(bytes).ok_or_else(|| {
            DomainError::try_new(
                ErrorCode::ValueOutOfRange,
                "accounted free exceeds current_bytes",
            )
            .expect("message")
            .with_context(ContextKey::Field, "memory.accounted.current_bytes")
            .expect("field")
            .with_context(ContextKey::Expected, self.current_bytes.to_string())
            .expect("expected")
            .with_context(ContextKey::Actual, bytes.to_string())
            .expect("actual")
        })?;
        self.current_bytes = new_current;
        Ok(())
    }

    pub fn snapshot(&self, source: impl Into<String>) -> Result<AccountedMemory, DomainError> {
        AccountedMemory::try_new(
            self.allocated_bytes,
            self.current_bytes,
            self.peak_bytes,
            source,
        )
    }
}

/// One timed phase with an optional wall-time budget.
///
/// Durations use nanoseconds so sub-millisecond CPU samples are not truncated
/// to zero. Optional `work_count` documents batching when many units are timed
/// together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhaseTiming {
    name: String,
    pub wall_time_ns: u64,
    source: String,
    pub budget_ns: Option<u64>,
    /// Number of work units covered by this sample, when batched.
    pub work_count: Option<u64>,
}

impl PhaseTiming {
    pub fn try_new(
        name: impl Into<String>,
        wall_time_ns: u64,
        source: impl Into<String>,
        budget_ns: Option<u64>,
        work_count: Option<u64>,
    ) -> Result<Self, DomainError> {
        let name = name.into();
        let source = source.into();
        require_non_empty(&name, "phases.name")?;
        require_non_empty(&source, "phases.source")?;
        if let Some(0) = work_count {
            return Err(DomainError::try_new(
                ErrorCode::InvalidArgument,
                "phases.work_count must be non-zero when present",
            )
            .expect("message")
            .with_context(ContextKey::Field, "phases.work_count")
            .expect("field"));
        }
        Ok(Self {
            name,
            wall_time_ns,
            source,
            budget_ns,
            work_count,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        Self::try_new(
            self.name.clone(),
            self.wall_time_ns,
            self.source.clone(),
            self.budget_ns,
            self.work_count,
        )
        .map(|_| ())
    }

    /// Fail when a budget is set and wall time exceeds it.
    pub fn check_budget(&self) -> Result<(), DomainError> {
        if let Some(budget) = self.budget_ns
            && self.wall_time_ns > budget
        {
            return Err(range_err(
                self.name.clone(),
                format!(
                    "phase `{}` wall_time_ns {} exceeds budget_ns {budget}",
                    self.name, self.wall_time_ns
                ),
                budget,
                self.wall_time_ns,
            ));
        }
        Ok(())
    }
}

/// Host / tool provenance that is safe to record without claiming measurement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvironmentInfo {
    pub os: String,
    pub arch: String,
    /// Rustc version string when known; otherwise an explicit unavailable note.
    pub rustc: String,
}

impl EnvironmentInfo {
    pub fn try_new(
        os: impl Into<String>,
        arch: impl Into<String>,
        rustc: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let os = os.into();
        let arch = arch.into();
        let rustc = rustc.into();
        require_non_empty(&os, "environment.os")?;
        require_non_empty(&arch, "environment.arch")?;
        require_non_empty(&rustc, "environment.rustc")?;
        Ok(Self { os, arch, rustc })
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        Self::try_new(self.os.clone(), self.arch.clone(), self.rustc.clone()).map(|_| ())
    }

    /// Capture host OS/arch and an optional rustc string from the sample binary.
    pub fn from_host(rustc: impl Into<String>) -> Result<Self, DomainError> {
        Self::try_new(std::env::consts::OS, std::env::consts::ARCH, rustc)
    }
}

/// Source-tree revision / digest identity for the measured binary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceRevision {
    /// Lowercase/uppercase hex SHA-256 of the revision payload.
    Digest { sha256: String },
    /// Revision was not measured; reason must be non-empty.
    Unavailable { reason: String },
}

impl SourceRevision {
    pub fn digest(sha256: impl Into<String>) -> Result<Self, DomainError> {
        let sha256 = sha256.into();
        validate_sha256_hex(&sha256, "provenance.source_revision.sha256")?;
        Ok(Self::Digest { sha256 })
    }

    pub fn unavailable(reason: impl Into<String>) -> Result<Self, DomainError> {
        let reason = reason.into();
        require_non_empty(&reason, "provenance.source_revision.reason")?;
        Ok(Self::Unavailable { reason })
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        match self {
            Self::Digest { sha256 } => {
                validate_sha256_hex(sha256, "provenance.source_revision.sha256")
            }
            Self::Unavailable { reason } => {
                require_non_empty(reason, "provenance.source_revision.reason")
            }
        }
    }
}

/// Working-tree dirty status relative to [`SourceRevision`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DirtyStatus {
    Clean,
    Dirty,
    Unavailable { reason: String },
}

impl DirtyStatus {
    pub fn unavailable(reason: impl Into<String>) -> Result<Self, DomainError> {
        let reason = reason.into();
        require_non_empty(&reason, "provenance.dirty.reason")?;
        Ok(Self::Unavailable { reason })
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        match self {
            Self::Clean | Self::Dirty => Ok(()),
            Self::Unavailable { reason } => require_non_empty(reason, "provenance.dirty.reason"),
        }
    }
}

/// Fixture / config content hashes, source identity, and environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PerfProvenance {
    /// Lowercase hex SHA-256 of the fixture bytes used for the sample.
    fixture_sha256: String,
    /// Lowercase hex SHA-256 of the config bytes used for the sample.
    config_sha256: String,
    source_revision: SourceRevision,
    dirty: DirtyStatus,
    pub environment: EnvironmentInfo,
}

impl PerfProvenance {
    pub fn try_new(
        fixture_sha256: impl Into<String>,
        config_sha256: impl Into<String>,
        source_revision: SourceRevision,
        dirty: DirtyStatus,
        environment: EnvironmentInfo,
    ) -> Result<Self, DomainError> {
        let fixture_sha256 = fixture_sha256.into();
        let config_sha256 = config_sha256.into();
        validate_sha256_hex(&fixture_sha256, "provenance.fixture_sha256")?;
        validate_sha256_hex(&config_sha256, "provenance.config_sha256")?;
        source_revision.validate()?;
        dirty.validate()?;
        environment.validate()?;
        Ok(Self {
            fixture_sha256,
            config_sha256,
            source_revision,
            dirty,
            environment,
        })
    }

    pub fn fixture_sha256(&self) -> &str {
        &self.fixture_sha256
    }

    pub fn config_sha256(&self) -> &str {
        &self.config_sha256
    }

    pub fn source_revision(&self) -> &SourceRevision {
        &self.source_revision
    }

    pub fn dirty(&self) -> &DirtyStatus {
        &self.dirty
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        validate_sha256_hex(&self.fixture_sha256, "provenance.fixture_sha256")?;
        validate_sha256_hex(&self.config_sha256, "provenance.config_sha256")?;
        self.source_revision.validate()?;
        self.dirty.validate()?;
        self.environment.validate()?;
        Ok(())
    }
}

fn validate_sha256_hex(value: &str, field: &str) -> Result<(), DomainError> {
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(DomainError::try_new(
            ErrorCode::MalformedRecord,
            format!("{field} must be 64 lowercase/uppercase hex digits"),
        )
        .expect("message")
        .with_context(ContextKey::Field, field)
        .expect("field")
        .with_context(ContextKey::Actual, value.to_owned())
        .expect("actual"));
    }
    Ok(())
}

fn require_non_empty(value: &str, field: &str) -> Result<(), DomainError> {
    if value.trim().is_empty() {
        return Err(DomainError::try_new(
            ErrorCode::InvalidArgument,
            format!("{field} must be non-empty"),
        )
        .expect("message")
        .with_context(ContextKey::Field, field)
        .expect("field"));
    }
    Ok(())
}

fn range_err(
    field: impl Into<String>,
    message: impl Into<String>,
    expected: u64,
    actual: u64,
) -> DomainError {
    DomainError::try_new(ErrorCode::ValueOutOfRange, message.into())
        .expect("message")
        .with_context(ContextKey::Field, field.into())
        .expect("field")
        .with_context(ContextKey::Expected, expected.to_string())
        .expect("expected")
        .with_context(ContextKey::Actual, actual.to_string())
        .expect("actual")
}

/// Resident accounted-budget failure uses the catalog resource-allocation code
/// (`SP-GPU-004` / [`ErrorCode::AllocationFailed`]) with a scoped host message.
fn allocation_budget_err(
    field: impl Into<String>,
    message: impl Into<String>,
    expected: u64,
    actual: u64,
) -> DomainError {
    DomainError::try_new(ErrorCode::AllocationFailed, message.into())
        .expect("message")
        .with_context(ContextKey::Field, field.into())
        .expect("field")
        .with_context(ContextKey::Resource, "accounted_work_buffer")
        .expect("resource")
        .with_context(ContextKey::Expected, expected.to_string())
        .expect("expected")
        .with_context(ContextKey::Actual, actual.to_string())
        .expect("actual")
}

fn unsupported_version(actual: u64) -> DomainError {
    DomainError::try_new(
        ErrorCode::UnsupportedVersion,
        format!("perf report version {actual} is unsupported (expected {PERF_REPORT_VERSION})"),
    )
    .expect("message")
    .with_context(ContextKey::Field, "version")
    .expect("field")
    .with_context(ContextKey::Expected, PERF_REPORT_VERSION.to_string())
    .expect("expected")
    .with_context(ContextKey::Actual, actual.to_string())
    .expect("actual")
}

/// Parse a JSON integer as the exact schema version without wrapping `u64→u32`.
pub fn parse_schema_version(raw: u64) -> Result<u32, DomainError> {
    let version = u32::try_from(raw).map_err(|_| unsupported_version(raw))?;
    if version != PERF_REPORT_VERSION {
        return Err(unsupported_version(u64::from(version)));
    }
    Ok(version)
}

/// Versioned performance / resource report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PerfReport {
    pub version: u32,
    feature_id: String,
    pub provenance: PerfProvenance,
    pub accounted: AccountedMemory,
    /// Optional resident (live/peak) host byte budget for [`Self::check_budgets`].
    /// Cumulative `allocated_bytes` is not constrained by this field.
    pub resource_budget_bytes: Option<u64>,
    /// OS-wide or process RSS peak — never filled with accounted numbers.
    pub os_process_peak: UnavailableMetric,
    /// Device-wide GPU memory peak — unavailable until a real GPU path measures it.
    pub device_memory: UnavailableMetric,
    /// General GPU counters / timings other than timestamp queries.
    pub gpu: UnavailableMetric,
    /// GPU timestamp-query based timing (distinct from absolute wall-clock).
    pub gpu_timestamp_query: UnavailableMetric,
    /// Absolute wall-clock timestamps — not claimed; phase durations only.
    pub wall_clock_timestamp: UnavailableMetric,
    pub phases: Vec<PhaseTiming>,
}

impl PerfReport {
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        version: u32,
        feature_id: impl Into<String>,
        provenance: PerfProvenance,
        accounted: AccountedMemory,
        resource_budget_bytes: Option<u64>,
        os_process_peak: UnavailableMetric,
        device_memory: UnavailableMetric,
        gpu: UnavailableMetric,
        gpu_timestamp_query: UnavailableMetric,
        wall_clock_timestamp: UnavailableMetric,
        phases: Vec<PhaseTiming>,
    ) -> Result<Self, DomainError> {
        let feature_id = feature_id.into();
        let report = Self {
            version,
            feature_id,
            provenance,
            accounted,
            resource_budget_bytes,
            os_process_peak,
            device_memory,
            gpu,
            gpu_timestamp_query,
            wall_clock_timestamp,
            phases,
        };
        report.validate()?;
        Ok(report)
    }

    pub fn feature_id(&self) -> &str {
        &self.feature_id
    }

    /// Revalidate every publicly mutable nested field (safe before save).
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.version != PERF_REPORT_VERSION {
            return Err(unsupported_version(u64::from(self.version)));
        }
        require_non_empty(&self.feature_id, "feature_id")?;
        self.provenance.validate()?;
        self.accounted.validate()?;
        self.os_process_peak.validate()?;
        self.device_memory.validate()?;
        self.gpu.validate()?;
        self.gpu_timestamp_query.validate()?;
        self.wall_clock_timestamp.validate()?;
        if self.phases.is_empty() {
            return Err(DomainError::try_new(
                ErrorCode::InvalidArgument,
                "perf report requires at least one phase",
            )
            .expect("message")
            .with_context(ContextKey::Field, "phases")
            .expect("field"));
        }
        for phase in &self.phases {
            phase.validate()?;
        }
        Ok(())
    }

    /// Fail closed on phase time budgets and optional resource byte budget.
    pub fn check_budgets(&self) -> Result<(), DomainError> {
        self.validate()?;
        if let Some(limit) = self.resource_budget_bytes {
            self.accounted.check_budget(limit)?;
        }
        for phase in &self.phases {
            phase.check_budget()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex64(ch: char) -> String {
        std::iter::repeat_n(ch, 64).collect()
    }

    fn sample_report(budget_ns: Option<u64>, wall_ns: u64) -> PerfReport {
        let env = EnvironmentInfo::try_new("testos", "testarch", "rustc-test").unwrap();
        let provenance = PerfProvenance::try_new(
            hex64('a'),
            hex64('b'),
            SourceRevision::unavailable("unit test has no source digest").unwrap(),
            DirtyStatus::unavailable("unit test does not inspect a worktree").unwrap(),
            env,
        )
        .unwrap();
        let accounted = AccountedMemory::try_new(100, 40, 80, "explicit_counter").unwrap();
        PerfReport::try_new(
            PERF_REPORT_VERSION,
            "P1-F08",
            provenance,
            accounted,
            Some(1_000),
            UnavailableMetric::try_new("os process peak not sampled in unit test").unwrap(),
            UnavailableMetric::try_new("no GPU device path in P1-F08").unwrap(),
            UnavailableMetric::try_new("GPU counters not implemented in P1-F08").unwrap(),
            UnavailableMetric::try_new(
                "GPU timestamp-query timing unavailable: P1-F08 has no GPU runtime path",
            )
            .unwrap(),
            UnavailableMetric::try_new(
                "absolute wall-clock timestamps are not claimed; only phase durations are recorded",
            )
            .unwrap(),
            vec![
                PhaseTiming::try_new("work", wall_ns, "Instant::elapsed", budget_ns, Some(1))
                    .unwrap(),
            ],
        )
        .unwrap()
    }

    #[test]
    fn rejects_unsupported_version() {
        let err = parse_schema_version(99).unwrap_err();
        assert_eq!(err.code(), ErrorCode::UnsupportedVersion);
    }

    #[test]
    fn version_u64_wrap_rejected() {
        // 2^32 + 1 would wrap to 1 if cast with `as u32`.
        let err = parse_schema_version(u64::from(u32::MAX) + 2).unwrap_err();
        assert_eq!(err.code(), ErrorCode::UnsupportedVersion);
        assert_ne!(parse_schema_version(1).unwrap(), 0);
    }

    #[test]
    fn accounted_requires_current_le_peak_le_allocated() {
        let err = AccountedMemory::try_new(10, 5, 20, "explicit_counter").unwrap_err();
        assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
        let err = AccountedMemory::try_new(10, 8, 5, "explicit_counter").unwrap_err();
        assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
    }

    #[test]
    fn validate_rejects_mutated_environment_and_memory() {
        let mut report = sample_report(Some(50), 10);
        report.provenance.environment.os = "   ".into();
        assert_eq!(
            report.validate().unwrap_err().code(),
            ErrorCode::InvalidArgument
        );
        report.provenance.environment.os = "testos".into();
        report.accounted.peak_bytes = 10_000;
        assert_eq!(
            report.validate().unwrap_err().code(),
            ErrorCode::ValueOutOfRange
        );
    }

    #[test]
    fn phase_time_budget_overflow_fails() {
        let report = sample_report(Some(5), 10);
        let err = report.check_budgets().unwrap_err();
        assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
    }

    #[test]
    fn resource_byte_budget_overflow_fails_on_peak_not_volume() {
        let mut report = sample_report(Some(50), 10);
        report.resource_budget_bytes = Some(50);
        // Cumulative volume 200 with peak 40 is fine under a 50-byte resident cap.
        report.accounted = AccountedMemory::try_new(200, 0, 40, "explicit_counter").unwrap();
        report.check_budgets().unwrap();
        report.accounted = AccountedMemory::try_new(200, 0, 80, "explicit_counter").unwrap();
        let err = report.check_budgets().unwrap_err();
        assert_eq!(err.code(), ErrorCode::AllocationFailed);
    }

    #[test]
    fn resident_budget_allows_allocate_free_reuse() {
        let mut t = AccountedBytesTracker::new();
        t.preflight(64, 64).unwrap();
        t.allocate(64).unwrap();
        t.free(64).unwrap();
        // Second cycle under the same live cap must succeed even though volume=128.
        t.preflight(64, 64).unwrap();
        t.allocate(64).unwrap();
        t.free(64).unwrap();
        assert_eq!(t.allocated_bytes(), 128);
        assert_eq!(t.peak_bytes(), 64);
        assert_eq!(t.current_bytes(), 0);
        t.snapshot("explicit_counter:per_iteration_work_buffer_only")
            .unwrap()
            .check_budget(64)
            .unwrap();
    }

    #[test]
    fn tracker_checked_overflow_and_underflow_leave_state() {
        let mut t = AccountedBytesTracker::new();
        t.allocate(10).unwrap();
        let before = (t.allocated_bytes(), t.current_bytes(), t.peak_bytes());
        let err = t.allocate(u64::MAX).unwrap_err();
        assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
        assert_eq!(
            (t.allocated_bytes(), t.current_bytes(), t.peak_bytes()),
            before
        );
        let err = t.free(11).unwrap_err();
        assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
        assert_eq!(t.current_bytes(), 10);
    }

    #[test]
    fn tracker_preflight_rejects_over_budget() {
        let t = AccountedBytesTracker::new();
        let err = t.preflight(64, 32).unwrap_err();
        assert_eq!(err.code(), ErrorCode::AllocationFailed);
        assert_eq!(
            err.context_value(ContextKey::Resource),
            Some("accounted_work_buffer")
        );
    }

    #[test]
    fn tracker_peak_follows_allocate_free() {
        let mut t = AccountedBytesTracker::new();
        t.allocate(100).unwrap();
        t.allocate(50).unwrap();
        t.free(100).unwrap();
        let snap = t.snapshot("explicit_counter").unwrap();
        assert_eq!(snap.allocated_bytes, 150);
        assert_eq!(snap.current_bytes, 50);
        assert_eq!(snap.peak_bytes, 150);
    }

    #[test]
    fn blank_unavailable_reason_rejected() {
        let err = UnavailableMetric::try_new("   ").unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidArgument);
    }

    #[test]
    fn invalid_hash_rejected() {
        let env = EnvironmentInfo::try_new("os", "arch", "rustc").unwrap();
        let err = PerfProvenance::try_new(
            "not-a-hash",
            hex64('c'),
            SourceRevision::unavailable("n/a").unwrap(),
            DirtyStatus::Clean,
            env,
        )
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::MalformedRecord);
    }
}
