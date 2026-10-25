//! Persist and reload versioned performance reports (P1-F08).
//!
//! Serialization uses `serde_json`. Publication reuses the atomic sibling
//! temp + replace path from [`crate::atomic`].

use std::fs;
use std::path::Path;

use serde_json::{Map, Value};
use spirula_core::{
    AccountedMemory, ContextKey, DirtyStatus, DomainError, EnvironmentInfo, ErrorCode,
    PERF_REPORT_SCHEMA, PerfProvenance, PerfReport, PhaseTiming, SourceRevision, UnavailableMetric,
    parse_schema_version,
};

use crate::atomic::{PublishOptions, PublishOutcome, publish_bytes_validated};
use crate::hash::sha256_hex;

/// Atomically write a validated report as JSON.
pub fn save_perf_report(
    path: &Path,
    report: &PerfReport,
    options: PublishOptions<'_>,
) -> Result<PublishOutcome, DomainError> {
    report.validate()?;
    let text = report_to_json(report)?;
    publish_bytes_validated(path, text.as_bytes(), options, |bytes| {
        let _ = parse_perf_report_bytes(bytes)?;
        Ok(())
    })
}

/// Read and validate a report from disk.
pub fn load_perf_report(path: &Path) -> Result<PerfReport, DomainError> {
    let bytes = fs::read(path).map_err(|error| {
        DomainError::try_new(ErrorCode::ReadFailed, "failed to read perf report")
            .expect("message")
            .with_context(ContextKey::Path, path.display().to_string())
            .expect("path")
            .with_source(error)
    })?;
    parse_perf_report_bytes(&bytes).map_err(|error| attach_path(error, path))
}

/// Serialize a report to canonical UTF-8 JSON (pretty-printed, trailing newline).
pub fn report_to_json(report: &PerfReport) -> Result<String, DomainError> {
    report.validate()?;
    let value = report_to_value(report);
    let mut text = serde_json::to_string_pretty(&value).map_err(|error| {
        DomainError::try_new(ErrorCode::Internal, "failed to serialize perf report")
            .expect("message")
            .with_source(error)
    })?;
    text.push('\n');
    Ok(text)
}

/// Parse report JSON bytes.
pub fn parse_perf_report_bytes(bytes: &[u8]) -> Result<PerfReport, DomainError> {
    let text = std::str::from_utf8(bytes).map_err(|error| {
        DomainError::try_new(ErrorCode::MalformedRecord, "perf report is not UTF-8")
            .expect("message")
            .with_context(ContextKey::Format, PERF_REPORT_SCHEMA)
            .expect("format")
            .with_source(error)
    })?;
    parse_perf_report_str(text)
}

/// Parse report JSON text.
pub fn parse_perf_report_str(text: &str) -> Result<PerfReport, DomainError> {
    let value: Value = serde_json::from_str(text).map_err(|error| {
        DomainError::try_new(ErrorCode::MalformedRecord, "perf report JSON parse failed")
            .expect("message")
            .with_context(ContextKey::Format, PERF_REPORT_SCHEMA)
            .expect("format")
            .with_source(error)
    })?;
    report_from_value(&value)
}

fn report_to_value(report: &PerfReport) -> Value {
    let mut root = Map::new();
    root.insert("schema".into(), Value::String(PERF_REPORT_SCHEMA.into()));
    root.insert("version".into(), Value::from(report.version));
    root.insert(
        "feature_id".into(),
        Value::String(report.feature_id().into()),
    );

    let mut provenance = Map::new();
    provenance.insert(
        "fixture_sha256".into(),
        Value::String(report.provenance.fixture_sha256().into()),
    );
    provenance.insert(
        "config_sha256".into(),
        Value::String(report.provenance.config_sha256().into()),
    );
    provenance.insert(
        "source_revision".into(),
        source_revision_to_value(report.provenance.source_revision()),
    );
    provenance.insert("dirty".into(), dirty_to_value(report.provenance.dirty()));
    let mut environment = Map::new();
    environment.insert(
        "os".into(),
        Value::String(report.provenance.environment.os.clone()),
    );
    environment.insert(
        "arch".into(),
        Value::String(report.provenance.environment.arch.clone()),
    );
    environment.insert(
        "rustc".into(),
        Value::String(report.provenance.environment.rustc.clone()),
    );
    provenance.insert("environment".into(), Value::Object(environment));
    root.insert("provenance".into(), Value::Object(provenance));

    let mut memory = Map::new();
    let mut accounted = Map::new();
    accounted.insert(
        "allocated_bytes".into(),
        Value::from(report.accounted.allocated_bytes),
    );
    accounted.insert(
        "current_bytes".into(),
        Value::from(report.accounted.current_bytes),
    );
    accounted.insert(
        "peak_bytes".into(),
        Value::from(report.accounted.peak_bytes),
    );
    accounted.insert(
        "source".into(),
        Value::String(report.accounted.source().into()),
    );
    memory.insert("accounted".into(), Value::Object(accounted));
    memory.insert(
        "resource_budget_bytes".into(),
        match report.resource_budget_bytes {
            Some(v) => Value::from(v),
            None => Value::Null,
        },
    );
    memory.insert(
        "os_process_peak".into(),
        unavailable_to_value(&report.os_process_peak),
    );
    memory.insert(
        "device_memory".into(),
        unavailable_to_value(&report.device_memory),
    );
    root.insert("memory".into(), Value::Object(memory));

    root.insert("gpu".into(), unavailable_to_value(&report.gpu));
    root.insert(
        "gpu_timestamp_query".into(),
        unavailable_to_value(&report.gpu_timestamp_query),
    );
    root.insert(
        "wall_clock_timestamp".into(),
        unavailable_to_value(&report.wall_clock_timestamp),
    );

    let phases: Vec<Value> = report
        .phases
        .iter()
        .map(|phase| {
            let mut obj = Map::new();
            obj.insert("name".into(), Value::String(phase.name().into()));
            obj.insert("wall_time_ns".into(), Value::from(phase.wall_time_ns));
            obj.insert("source".into(), Value::String(phase.source().into()));
            obj.insert(
                "budget_ns".into(),
                match phase.budget_ns {
                    Some(budget) => Value::from(budget),
                    None => Value::Null,
                },
            );
            obj.insert(
                "work_count".into(),
                match phase.work_count {
                    Some(count) => Value::from(count),
                    None => Value::Null,
                },
            );
            Value::Object(obj)
        })
        .collect();
    root.insert("phases".into(), Value::Array(phases));
    Value::Object(root)
}

fn source_revision_to_value(revision: &SourceRevision) -> Value {
    let mut obj = Map::new();
    match revision {
        SourceRevision::Digest { sha256 } => {
            obj.insert("status".into(), Value::String("digest".into()));
            obj.insert("sha256".into(), Value::String(sha256.clone()));
        }
        SourceRevision::Unavailable { reason } => {
            obj.insert("status".into(), Value::String("unavailable".into()));
            obj.insert("reason".into(), Value::String(reason.clone()));
        }
    }
    Value::Object(obj)
}

fn dirty_to_value(dirty: &DirtyStatus) -> Value {
    let mut obj = Map::new();
    match dirty {
        DirtyStatus::Clean => {
            obj.insert("status".into(), Value::String("clean".into()));
        }
        DirtyStatus::Dirty => {
            obj.insert("status".into(), Value::String("dirty".into()));
        }
        DirtyStatus::Unavailable { reason } => {
            obj.insert("status".into(), Value::String("unavailable".into()));
            obj.insert("reason".into(), Value::String(reason.clone()));
        }
    }
    Value::Object(obj)
}

fn unavailable_to_value(metric: &UnavailableMetric) -> Value {
    let mut obj = Map::new();
    obj.insert("status".into(), Value::String("unavailable".into()));
    obj.insert("reason".into(), Value::String(metric.reason().into()));
    Value::Object(obj)
}

fn report_from_value(value: &Value) -> Result<PerfReport, DomainError> {
    let obj = expect_object(value, "report")?;
    let schema = expect_str(obj, "schema")?;
    if schema != PERF_REPORT_SCHEMA {
        return Err(DomainError::try_new(
            ErrorCode::UnsupportedFormat,
            format!("unexpected perf report schema `{schema}`"),
        )
        .expect("message")
        .with_context(ContextKey::Format, schema)
        .expect("format")
        .with_context(ContextKey::Expected, PERF_REPORT_SCHEMA)
        .expect("expected"));
    }
    let version = parse_schema_version(expect_u64(obj, "version")?)?;
    let feature_id = expect_str(obj, "feature_id")?.to_owned();

    let provenance_obj = expect_object(expect_key(obj, "provenance")?, "provenance")?;
    let fixture_sha256 = expect_str(provenance_obj, "fixture_sha256")?.to_owned();
    let config_sha256 = expect_str(provenance_obj, "config_sha256")?.to_owned();
    let source_revision =
        source_revision_from_value(expect_key(provenance_obj, "source_revision")?)?;
    let dirty = dirty_from_value(expect_key(provenance_obj, "dirty")?)?;
    let env_obj = expect_object(expect_key(provenance_obj, "environment")?, "environment")?;
    let environment = EnvironmentInfo::try_new(
        expect_str(env_obj, "os")?,
        expect_str(env_obj, "arch")?,
        expect_str(env_obj, "rustc")?,
    )?;
    let provenance = PerfProvenance::try_new(
        fixture_sha256,
        config_sha256,
        source_revision,
        dirty,
        environment,
    )?;

    let memory_obj = expect_object(expect_key(obj, "memory")?, "memory")?;
    let accounted_obj = expect_object(expect_key(memory_obj, "accounted")?, "memory.accounted")?;
    let accounted = AccountedMemory::try_new(
        expect_u64(accounted_obj, "allocated_bytes")?,
        expect_u64(accounted_obj, "current_bytes")?,
        expect_u64(accounted_obj, "peak_bytes")?,
        expect_str(accounted_obj, "source")?,
    )?;
    let resource_budget_bytes = optional_u64(memory_obj, "resource_budget_bytes")?;
    let os_process_peak = unavailable_from_value(
        expect_key(memory_obj, "os_process_peak")?,
        "memory.os_process_peak",
    )?;
    let device_memory = unavailable_from_value(
        expect_key(memory_obj, "device_memory")?,
        "memory.device_memory",
    )?;
    let gpu = unavailable_from_value(expect_key(obj, "gpu")?, "gpu")?;
    let gpu_timestamp_query = unavailable_from_value(
        expect_key(obj, "gpu_timestamp_query")?,
        "gpu_timestamp_query",
    )?;
    let wall_clock_timestamp = unavailable_from_value(
        expect_key(obj, "wall_clock_timestamp")?,
        "wall_clock_timestamp",
    )?;

    let phases_val = expect_key(obj, "phases")?;
    let phases_arr = phases_val.as_array().ok_or_else(|| {
        DomainError::try_new(ErrorCode::MalformedRecord, "phases must be an array")
            .expect("message")
            .with_context(ContextKey::Field, "phases")
            .expect("field")
    })?;
    let mut phases = Vec::with_capacity(phases_arr.len());
    for (index, phase_val) in phases_arr.iter().enumerate() {
        let ctx = format!("phases[{index}]");
        let phase_obj = expect_object(phase_val, &ctx)?;
        let budget = optional_u64(phase_obj, "budget_ns")?;
        let work_count = optional_u64(phase_obj, "work_count")?;
        phases.push(PhaseTiming::try_new(
            expect_str(phase_obj, "name")?,
            expect_u64(phase_obj, "wall_time_ns")?,
            expect_str(phase_obj, "source")?,
            budget,
            work_count,
        )?);
    }

    PerfReport::try_new(
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
    )
}

fn source_revision_from_value(value: &Value) -> Result<SourceRevision, DomainError> {
    let obj = expect_object(value, "provenance.source_revision")?;
    let status = expect_str(obj, "status")?;
    match status {
        "digest" => SourceRevision::digest(expect_str(obj, "sha256")?),
        "unavailable" => SourceRevision::unavailable(expect_str(obj, "reason")?),
        other => Err(DomainError::try_new(
            ErrorCode::MalformedRecord,
            format!("provenance.source_revision.status `{other}` is unsupported"),
        )
        .expect("message")
        .with_context(ContextKey::Field, "provenance.source_revision.status")
        .expect("field")
        .with_context(ContextKey::Actual, other)
        .expect("actual")),
    }
}

fn dirty_from_value(value: &Value) -> Result<DirtyStatus, DomainError> {
    let obj = expect_object(value, "provenance.dirty")?;
    let status = expect_str(obj, "status")?;
    match status {
        "clean" => Ok(DirtyStatus::Clean),
        "dirty" => Ok(DirtyStatus::Dirty),
        "unavailable" => DirtyStatus::unavailable(expect_str(obj, "reason")?),
        other => Err(DomainError::try_new(
            ErrorCode::MalformedRecord,
            format!("provenance.dirty.status `{other}` is unsupported"),
        )
        .expect("message")
        .with_context(ContextKey::Field, "provenance.dirty.status")
        .expect("field")
        .with_context(ContextKey::Actual, other)
        .expect("actual")),
    }
}

fn unavailable_from_value(value: &Value, field: &str) -> Result<UnavailableMetric, DomainError> {
    let obj = expect_object(value, field)?;
    let status = expect_str(obj, "status")?;
    if status != "unavailable" {
        return Err(DomainError::try_new(
            ErrorCode::MalformedRecord,
            format!("{field}.status must be `unavailable` in P1-F08 reports"),
        )
        .expect("message")
        .with_context(ContextKey::Field, format!("{field}.status"))
        .expect("field")
        .with_context(ContextKey::Actual, status)
        .expect("actual"));
    }
    UnavailableMetric::try_new(expect_str(obj, "reason")?)
}

fn expect_key<'a>(obj: &'a Map<String, Value>, key: &str) -> Result<&'a Value, DomainError> {
    obj.get(key).ok_or_else(|| {
        DomainError::try_new(
            ErrorCode::MalformedRecord,
            format!("missing perf report field `{key}`"),
        )
        .expect("message")
        .with_context(ContextKey::Field, key)
        .expect("field")
    })
}

fn expect_object<'a>(value: &'a Value, field: &str) -> Result<&'a Map<String, Value>, DomainError> {
    value.as_object().ok_or_else(|| {
        DomainError::try_new(
            ErrorCode::MalformedRecord,
            format!("{field} must be a JSON object"),
        )
        .expect("message")
        .with_context(ContextKey::Field, field)
        .expect("field")
    })
}

fn expect_str<'a>(obj: &'a Map<String, Value>, key: &str) -> Result<&'a str, DomainError> {
    match expect_key(obj, key)? {
        Value::String(s) => Ok(s.as_str()),
        _ => Err(DomainError::try_new(
            ErrorCode::MalformedRecord,
            format!("{key} must be a string"),
        )
        .expect("message")
        .with_context(ContextKey::Field, key)
        .expect("field")),
    }
}

fn expect_u64(obj: &Map<String, Value>, key: &str) -> Result<u64, DomainError> {
    match expect_key(obj, key)? {
        Value::Number(n) => n.as_u64().ok_or_else(|| {
            DomainError::try_new(
                ErrorCode::MalformedRecord,
                format!("{key} must be an unsigned integer"),
            )
            .expect("message")
            .with_context(ContextKey::Field, key)
            .expect("field")
        }),
        _ => Err(DomainError::try_new(
            ErrorCode::MalformedRecord,
            format!("{key} must be a number"),
        )
        .expect("message")
        .with_context(ContextKey::Field, key)
        .expect("field")),
    }
}

fn optional_u64(obj: &Map<String, Value>, key: &str) -> Result<Option<u64>, DomainError> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => Ok(Some(n.as_u64().ok_or_else(|| {
            DomainError::try_new(
                ErrorCode::MalformedRecord,
                format!("{key} must be an unsigned integer"),
            )
            .expect("message")
            .with_context(ContextKey::Field, key)
            .expect("field")
        })?)),
        Some(_) => Err(DomainError::try_new(
            ErrorCode::MalformedRecord,
            format!("{key} must be null or an unsigned integer"),
        )
        .expect("message")
        .with_context(ContextKey::Field, key)
        .expect("field")),
    }
}

fn attach_path(error: DomainError, path: &Path) -> DomainError {
    if error.context_value(ContextKey::Path).is_some() {
        return error;
    }
    error
        .with_context(ContextKey::Path, path.display().to_string())
        .expect("path context was absent")
}

/// Build provenance hashes from in-memory fixture and config payloads.
pub fn provenance_hashes(fixture_bytes: &[u8], config_bytes: &[u8]) -> (String, String) {
    (sha256_hex(fixture_bytes), sha256_hex(config_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::temp_harness::TempHarness;
    use spirula_core::{
        AccountedBytesTracker, AccountedMemory, DirtyStatus, EnvironmentInfo, ErrorCode,
        PERF_REPORT_VERSION, PerfProvenance, PhaseTiming, SourceRevision, UnavailableMetric,
    };

    fn hex64(ch: char) -> String {
        std::iter::repeat_n(ch, 64).collect()
    }

    fn sample_report() -> PerfReport {
        let env = EnvironmentInfo::try_new("testos", "testarch", "rustc-test").unwrap();
        let provenance = PerfProvenance::try_new(
            hex64('a'),
            hex64('b'),
            SourceRevision::unavailable("io unit test has no source digest").unwrap(),
            DirtyStatus::unavailable("io unit test does not inspect a worktree").unwrap(),
            env,
        )
        .unwrap();
        let accounted = AccountedMemory::try_new(64, 32, 64, "explicit_counter").unwrap();
        PerfReport::try_new(
            PERF_REPORT_VERSION,
            "P1-F08",
            provenance,
            accounted,
            Some(1_024),
            UnavailableMetric::try_new("os process peak not sampled").unwrap(),
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
                PhaseTiming::try_new(
                    "allocate",
                    1_500,
                    "Instant::elapsed",
                    Some(1_000_000),
                    Some(1),
                )
                .unwrap(),
            ],
        )
        .unwrap()
    }

    #[test]
    fn round_trip_atomic_save_load() {
        let harness = TempHarness::new().unwrap();
        let path = harness.prepare_file("report.json").unwrap();
        let report = sample_report();
        save_perf_report(&path, &report, PublishOptions::default()).unwrap();
        let loaded = load_perf_report(&path).unwrap();
        assert_eq!(loaded, report);
        loaded.check_budgets().unwrap();
        assert!(loaded.phases[0].wall_time_ns > 0);
    }

    #[test]
    fn rejects_wrapped_version_that_aliases_schema() {
        let mut text = report_to_json(&sample_report()).unwrap();
        // 2^32 + 1 would become schema version 1 under a truncating cast.
        text = text.replace("\"version\": 1", "\"version\": 4294967297");
        let err = parse_perf_report_str(&text).unwrap_err();
        assert_eq!(err.code(), ErrorCode::UnsupportedVersion);
    }

    #[test]
    fn rejects_unsupported_version_on_parse() {
        let mut text = report_to_json(&sample_report()).unwrap();
        text = text.replace("\"version\": 1", "\"version\": 99");
        let err = parse_perf_report_str(&text).unwrap_err();
        assert_eq!(err.code(), ErrorCode::UnsupportedVersion);
    }

    #[test]
    fn rejects_malformed_json() {
        let err = parse_perf_report_str("{not json").unwrap_err();
        assert_eq!(err.code(), ErrorCode::MalformedRecord);
    }

    #[test]
    fn save_rejects_mutated_invalid_nested_fields() {
        let harness = TempHarness::new().unwrap();
        let path = harness.prepare_file("bad.json").unwrap();
        let mut report = sample_report();
        report.accounted.current_bytes = 10_000;
        let err = save_perf_report(&path, &report, PublishOptions::default()).unwrap_err();
        assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
        assert!(!path.exists());
        report.accounted.current_bytes = 32;
        report.provenance.environment.rustc.clear();
        let err = save_perf_report(&path, &report, PublishOptions::default()).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidArgument);
    }

    #[test]
    fn resource_and_phase_budget_overflow_detected() {
        let mut report = sample_report();
        // Peak 64 under resident cap 16 fails; volume is irrelevant.
        report.resource_budget_bytes = Some(16);
        let err = report.check_budgets().unwrap_err();
        assert_eq!(err.code(), ErrorCode::ValueOutOfRange);

        report.resource_budget_bytes = Some(1_024);
        report.phases[0] =
            PhaseTiming::try_new("allocate", 50, "Instant::elapsed", Some(10), Some(1)).unwrap();
        let err = report.check_budgets().unwrap_err();
        assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
    }

    #[test]
    fn tracker_feeds_accounted_section() {
        let mut tracker = AccountedBytesTracker::new();
        tracker.preflight(128, 256).unwrap();
        tracker.allocate(128).unwrap();
        let snap = tracker.snapshot("explicit_counter").unwrap();
        assert_eq!(snap.peak_bytes, 128);
        let (fx, cfg) = provenance_hashes(b"fixture", b"config");
        assert_eq!(fx.len(), 64);
        assert_eq!(cfg.len(), 64);
    }

    #[test]
    fn gpu_timestamp_query_distinct_from_wall_clock() {
        let json = report_to_json(&sample_report()).unwrap();
        assert!(json.contains("gpu_timestamp_query"));
        assert!(json.contains("wall_clock_timestamp"));
        assert!(json.contains("GPU timestamp-query timing unavailable"));
        assert!(json.contains("absolute wall-clock timestamps are not claimed"));
    }
}
