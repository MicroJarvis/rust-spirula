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
        if reason.trim().is_empty() {
            return Err(DomainError::try_new(
                ErrorCode::InvalidArgument,
                "unavailable metric reason must be non-empty",
            )
            .expect("message")
            .with_context(ContextKey::Field, "unavailable.reason")
            .expect("field"));
        }
        Ok(Self { reason })
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// Application-accounted host bytes. Values come from an explicit counter the
/// caller updates on allocate/free — not from OS RSS or device queries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountedMemory {
    /// Cumulative bytes requested through the accounted path.
    pub allocated_bytes: u64,
    /// Bytes still held by the accounted path.
    pub current_bytes: u64,
    /// High-water mark of `current_bytes`.
    pub peak_bytes: u64,
    /// How the numbers were produced (for example `explicit_counter`).
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
        if source.trim().is_empty() {
            return Err(DomainError::try_new(
                ErrorCode::InvalidArgument,
                "accounted memory source must be non-empty",
            )
            .expect("message")
            .with_context(ContextKey::Field, "memory.accounted.source")
            .expect("field"));
        }
        if current_bytes > allocated_bytes {
            return Err(DomainError::try_new(
                ErrorCode::InvalidArgument,
                "accounted current_bytes cannot exceed allocated_bytes",
            )
            .expect("message")
            .with_context(ContextKey::Field, "memory.accounted.current_bytes")
            .expect("field")
            .with_context(ContextKey::Expected, allocated_bytes.to_string())
            .expect("expected")
            .with_context(ContextKey::Actual, current_bytes.to_string())
            .expect("actual"));
        }
        if peak_bytes < current_bytes {
            return Err(DomainError::try_new(
                ErrorCode::InvalidArgument,
                "accounted peak_bytes cannot be below current_bytes",
            )
            .expect("message")
            .with_context(ContextKey::Field, "memory.accounted.peak_bytes")
            .expect("field"));
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
}

/// Mutable counter used by samples to produce [`AccountedMemory`].
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

    /// Record an allocation of `bytes`.
    pub fn allocate(&mut self, bytes: u64) {
        self.allocated_bytes = self.allocated_bytes.saturating_add(bytes);
        self.current_bytes = self.current_bytes.saturating_add(bytes);
        self.peak_bytes = self.peak_bytes.max(self.current_bytes);
    }

    /// Record a free of `bytes` (clamped to current).
    pub fn free(&mut self, bytes: u64) {
        self.current_bytes = self.current_bytes.saturating_sub(bytes);
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhaseTiming {
    name: String,
    pub wall_time_ms: u64,
    source: String,
    pub budget_ms: Option<u64>,
}

impl PhaseTiming {
    pub fn try_new(
        name: impl Into<String>,
        wall_time_ms: u64,
        source: impl Into<String>,
        budget_ms: Option<u64>,
    ) -> Result<Self, DomainError> {
        let name = name.into();
        let source = source.into();
        if name.trim().is_empty() {
            return Err(DomainError::try_new(
                ErrorCode::InvalidArgument,
                "phase name must be non-empty",
            )
            .expect("message")
            .with_context(ContextKey::Field, "phases.name")
            .expect("field"));
        }
        if source.trim().is_empty() {
            return Err(DomainError::try_new(
                ErrorCode::InvalidArgument,
                "phase timing source must be non-empty",
            )
            .expect("message")
            .with_context(ContextKey::Field, "phases.source")
            .expect("field"));
        }
        Ok(Self {
            name,
            wall_time_ms,
            source,
            budget_ms,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// Fail when a budget is set and wall time exceeds it.
    pub fn check_budget(&self) -> Result<(), DomainError> {
        if let Some(budget) = self.budget_ms
            && self.wall_time_ms > budget
        {
            return Err(DomainError::try_new(
                ErrorCode::ValueOutOfRange,
                format!(
                    "phase `{}` wall_time_ms {} exceeds budget_ms {budget}",
                    self.name, self.wall_time_ms
                ),
            )
            .expect("message")
            .with_context(ContextKey::Field, self.name.clone())
            .expect("field")
            .with_context(ContextKey::Expected, budget.to_string())
            .expect("expected")
            .with_context(ContextKey::Actual, self.wall_time_ms.to_string())
            .expect("actual"));
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
        for (field, value) in [
            ("environment.os", &os),
            ("environment.arch", &arch),
            ("environment.rustc", &rustc),
        ] {
            if value.trim().is_empty() {
                return Err(DomainError::try_new(
                    ErrorCode::InvalidArgument,
                    format!("{field} must be non-empty"),
                )
                .expect("message")
                .with_context(ContextKey::Field, field)
                .expect("field"));
            }
        }
        Ok(Self { os, arch, rustc })
    }

    /// Capture host OS/arch and an optional rustc string from the sample binary.
    pub fn from_host(rustc: impl Into<String>) -> Result<Self, DomainError> {
        Self::try_new(std::env::consts::OS, std::env::consts::ARCH, rustc)
    }
}

/// Fixture / config content hashes and environment for reproducibility.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PerfProvenance {
    /// Lowercase hex SHA-256 of the fixture bytes used for the sample.
    fixture_sha256: String,
    /// Lowercase hex SHA-256 of the config bytes used for the sample.
    config_sha256: String,
    pub environment: EnvironmentInfo,
}

impl PerfProvenance {
    pub fn try_new(
        fixture_sha256: impl Into<String>,
        config_sha256: impl Into<String>,
        environment: EnvironmentInfo,
    ) -> Result<Self, DomainError> {
        let fixture_sha256 = fixture_sha256.into();
        let config_sha256 = config_sha256.into();
        validate_sha256_hex(&fixture_sha256, "provenance.fixture_sha256")?;
        validate_sha256_hex(&config_sha256, "provenance.config_sha256")?;
        Ok(Self {
            fixture_sha256,
            config_sha256,
            environment,
        })
    }

    pub fn fixture_sha256(&self) -> &str {
        &self.fixture_sha256
    }

    pub fn config_sha256(&self) -> &str {
        &self.config_sha256
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

/// Versioned performance / resource report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PerfReport {
    pub version: u32,
    feature_id: String,
    pub provenance: PerfProvenance,
    pub accounted: AccountedMemory,
    /// OS-wide or process RSS peak — never filled with accounted numbers.
    pub os_process_peak: UnavailableMetric,
    /// Device-wide GPU memory peak — unavailable until a real GPU path measures it.
    pub device_memory: UnavailableMetric,
    /// GPU timing / counters — unavailable in P1-F08.
    pub gpu: UnavailableMetric,
    /// Absolute wall-clock timestamps — not claimed; phase durations only.
    pub timestamp: UnavailableMetric,
    pub phases: Vec<PhaseTiming>,
}

impl PerfReport {
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        version: u32,
        feature_id: impl Into<String>,
        provenance: PerfProvenance,
        accounted: AccountedMemory,
        os_process_peak: UnavailableMetric,
        device_memory: UnavailableMetric,
        gpu: UnavailableMetric,
        timestamp: UnavailableMetric,
        phases: Vec<PhaseTiming>,
    ) -> Result<Self, DomainError> {
        let feature_id = feature_id.into();
        if version != PERF_REPORT_VERSION {
            return Err(DomainError::try_new(
                ErrorCode::UnsupportedVersion,
                format!(
                    "perf report version {version} is unsupported (expected {PERF_REPORT_VERSION})"
                ),
            )
            .expect("message")
            .with_context(ContextKey::Field, "version")
            .expect("field")
            .with_context(ContextKey::Expected, PERF_REPORT_VERSION.to_string())
            .expect("expected")
            .with_context(ContextKey::Actual, version.to_string())
            .expect("actual"));
        }
        if feature_id.trim().is_empty() {
            return Err(DomainError::try_new(
                ErrorCode::InvalidArgument,
                "feature_id must be non-empty",
            )
            .expect("message")
            .with_context(ContextKey::Field, "feature_id")
            .expect("field"));
        }
        if phases.is_empty() {
            return Err(DomainError::try_new(
                ErrorCode::InvalidArgument,
                "perf report requires at least one phase",
            )
            .expect("message")
            .with_context(ContextKey::Field, "phases")
            .expect("field"));
        }
        let report = Self {
            version,
            feature_id,
            provenance,
            accounted,
            os_process_peak,
            device_memory,
            gpu,
            timestamp,
            phases,
        };
        report.validate()?;
        Ok(report)
    }

    pub fn feature_id(&self) -> &str {
        &self.feature_id
    }

    /// Structural validation (version, hashes, reasons). Does not check budgets.
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.version != PERF_REPORT_VERSION {
            return Err(DomainError::try_new(
                ErrorCode::UnsupportedVersion,
                format!(
                    "perf report version {} is unsupported (expected {PERF_REPORT_VERSION})",
                    self.version
                ),
            )
            .expect("message")
            .with_context(ContextKey::Field, "version")
            .expect("field"));
        }
        validate_sha256_hex(
            self.provenance.fixture_sha256(),
            "provenance.fixture_sha256",
        )?;
        validate_sha256_hex(self.provenance.config_sha256(), "provenance.config_sha256")?;
        if self.phases.is_empty() {
            return Err(DomainError::try_new(
                ErrorCode::InvalidArgument,
                "perf report requires at least one phase",
            )
            .expect("message")
            .with_context(ContextKey::Field, "phases")
            .expect("field"));
        }
        Ok(())
    }

    /// Fail closed when any phase exceeds its budget.
    pub fn check_budgets(&self) -> Result<(), DomainError> {
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

    fn sample_report(budget_ms: Option<u64>, wall_ms: u64) -> PerfReport {
        let env = EnvironmentInfo::try_new("testos", "testarch", "rustc-test").unwrap();
        let provenance = PerfProvenance::try_new(hex64('a'), hex64('b'), env).unwrap();
        let accounted = AccountedMemory::try_new(100, 40, 80, "explicit_counter").unwrap();
        PerfReport::try_new(
            PERF_REPORT_VERSION,
            "P1-F08",
            provenance,
            accounted,
            UnavailableMetric::try_new("os process peak not sampled in unit test").unwrap(),
            UnavailableMetric::try_new("no GPU device path in P1-F08").unwrap(),
            UnavailableMetric::try_new("GPU counters not implemented in P1-F08").unwrap(),
            UnavailableMetric::try_new("absolute timestamps not claimed; phase durations only")
                .unwrap(),
            vec![PhaseTiming::try_new("work", wall_ms, "Instant::elapsed", budget_ms).unwrap()],
        )
        .unwrap()
    }

    #[test]
    fn rejects_unsupported_version() {
        let env = EnvironmentInfo::try_new("os", "arch", "rustc").unwrap();
        let provenance = PerfProvenance::try_new(hex64('0'), hex64('1'), env).unwrap();
        let accounted = AccountedMemory::try_new(1, 1, 1, "explicit_counter").unwrap();
        let err = PerfReport::try_new(
            99,
            "P1-F08",
            provenance,
            accounted,
            UnavailableMetric::try_new("os").unwrap(),
            UnavailableMetric::try_new("device").unwrap(),
            UnavailableMetric::try_new("gpu").unwrap(),
            UnavailableMetric::try_new("ts").unwrap(),
            vec![PhaseTiming::try_new("a", 1, "Instant::elapsed", None).unwrap()],
        )
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::UnsupportedVersion);
    }

    #[test]
    fn budget_overflow_fails() {
        let report = sample_report(Some(5), 10);
        let err = report.check_budgets().unwrap_err();
        assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
    }

    #[test]
    fn budget_within_limit_ok() {
        let report = sample_report(Some(50), 10);
        report.check_budgets().unwrap();
    }

    #[test]
    fn blank_unavailable_reason_rejected() {
        let err = UnavailableMetric::try_new("   ").unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidArgument);
    }

    #[test]
    fn tracker_peak_follows_allocate_free() {
        let mut t = AccountedBytesTracker::new();
        t.allocate(100);
        t.allocate(50);
        t.free(100);
        let snap = t.snapshot("explicit_counter").unwrap();
        assert_eq!(snap.allocated_bytes, 150);
        assert_eq!(snap.current_bytes, 50);
        assert_eq!(snap.peak_bytes, 150);
    }

    #[test]
    fn invalid_hash_rejected() {
        let env = EnvironmentInfo::try_new("os", "arch", "rustc").unwrap();
        let err = PerfProvenance::try_new("not-a-hash", hex64('c'), env).unwrap_err();
        assert_eq!(err.code(), ErrorCode::MalformedRecord);
    }
}
