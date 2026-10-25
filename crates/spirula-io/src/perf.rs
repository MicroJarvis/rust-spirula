//! Persist and reload versioned performance reports (P1-F08).
//!
//! Serialization uses `serde_json`. Publication reuses the atomic sibling
//! temp + replace path from [`crate::atomic`].

use std::fs;
use std::path::Path;

use serde_json::{Map, Value};
use spirula_core::{
    AccountedMemory, ContextKey, DomainError, EnvironmentInfo, ErrorCode, PERF_REPORT_SCHEMA,
    PERF_REPORT_VERSION, PerfProvenance, PerfReport, PhaseTiming, UnavailableMetric,
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
        "os_process_peak".into(),
        unavailable_to_value(&report.os_process_peak),
    );
    memory.insert(
        "device_memory".into(),
        unavailable_to_value(&report.device_memory),
    );
    root.insert("memory".into(), Value::Object(memory));

    root.insert("gpu".into(), unavailable_to_value(&report.gpu));
    root.insert("timestamp".into(), unavailable_to_value(&report.timestamp));

    let phases: Vec<Value> = report
        .phases
        .iter()
        .map(|phase| {
            let mut obj = Map::new();
            obj.insert("name".into(), Value::String(phase.name().into()));
            obj.insert("wall_time_ms".into(), Value::from(phase.wall_time_ms));
            obj.insert("source".into(), Value::String(phase.source().into()));
            match phase.budget_ms {
                Some(budget) => {
                    obj.insert("budget_ms".into(), Value::from(budget));
                }
                None => {
                    obj.insert("budget_ms".into(), Value::Null);
                }
            }
            Value::Object(obj)
        })
        .collect();
    root.insert("phases".into(), Value::Array(phases));
    Value::Object(root)
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
    let version = expect_u64(obj, "version")? as u32;
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
    let feature_id = expect_str(obj, "feature_id")?.to_owned();

    let provenance_obj = expect_object(expect_key(obj, "provenance")?, "provenance")?;
    let fixture_sha256 = expect_str(provenance_obj, "fixture_sha256")?.to_owned();
    let config_sha256 = expect_str(provenance_obj, "config_sha256")?.to_owned();
    let env_obj = expect_object(expect_key(provenance_obj, "environment")?, "environment")?;
    let environment = EnvironmentInfo::try_new(
        expect_str(env_obj, "os")?,
        expect_str(env_obj, "arch")?,
        expect_str(env_obj, "rustc")?,
    )?;
    let provenance = PerfProvenance::try_new(fixture_sha256, config_sha256, environment)?;

    let memory_obj = expect_object(expect_key(obj, "memory")?, "memory")?;
    let accounted_obj = expect_object(expect_key(memory_obj, "accounted")?, "memory.accounted")?;
    let accounted = AccountedMemory::try_new(
        expect_u64(accounted_obj, "allocated_bytes")?,
        expect_u64(accounted_obj, "current_bytes")?,
        expect_u64(accounted_obj, "peak_bytes")?,
        expect_str(accounted_obj, "source")?,
    )?;
    let os_process_peak = unavailable_from_value(
        expect_key(memory_obj, "os_process_peak")?,
        "memory.os_process_peak",
    )?;
    let device_memory = unavailable_from_value(
        expect_key(memory_obj, "device_memory")?,
        "memory.device_memory",
    )?;
    let gpu = unavailable_from_value(expect_key(obj, "gpu")?, "gpu")?;
    let timestamp = unavailable_from_value(expect_key(obj, "timestamp")?, "timestamp")?;

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
        let budget = match phase_obj.get("budget_ms") {
            None | Some(Value::Null) => None,
            Some(Value::Number(n)) => Some(n.as_u64().ok_or_else(|| {
                DomainError::try_new(
                    ErrorCode::MalformedRecord,
                    format!("{ctx}.budget_ms must be an unsigned integer"),
                )
                .expect("message")
                .with_context(ContextKey::Field, format!("{ctx}.budget_ms"))
                .expect("field")
            })?),
            Some(_) => {
                return Err(DomainError::try_new(
                    ErrorCode::MalformedRecord,
                    format!("{ctx}.budget_ms must be null or an unsigned integer"),
                )
                .expect("message")
                .with_context(ContextKey::Field, format!("{ctx}.budget_ms"))
                .expect("field"));
            }
        };
        phases.push(PhaseTiming::try_new(
            expect_str(phase_obj, "name")?,
            expect_u64(phase_obj, "wall_time_ms")?,
            expect_str(phase_obj, "source")?,
            budget,
        )?);
    }

    PerfReport::try_new(
        version,
        feature_id,
        provenance,
        accounted,
        os_process_peak,
        device_memory,
        gpu,
        timestamp,
        phases,
    )
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
        AccountedBytesTracker, AccountedMemory, EnvironmentInfo, ErrorCode, PERF_REPORT_VERSION,
        PerfProvenance, PhaseTiming, UnavailableMetric,
    };

    fn hex64(ch: char) -> String {
        std::iter::repeat_n(ch, 64).collect()
    }

    fn sample_report() -> PerfReport {
        let env = EnvironmentInfo::try_new("testos", "testarch", "rustc-test").unwrap();
        let provenance = PerfProvenance::try_new(hex64('a'), hex64('b'), env).unwrap();
        let accounted = AccountedMemory::try_new(64, 32, 64, "explicit_counter").unwrap();
        PerfReport::try_new(
            PERF_REPORT_VERSION,
            "P1-F08",
            provenance,
            accounted,
            UnavailableMetric::try_new("os process peak not sampled").unwrap(),
            UnavailableMetric::try_new("no GPU device path in P1-F08").unwrap(),
            UnavailableMetric::try_new("GPU counters not implemented in P1-F08").unwrap(),
            UnavailableMetric::try_new("absolute timestamps not claimed; phase durations only")
                .unwrap(),
            vec![PhaseTiming::try_new("allocate", 1, "Instant::elapsed", Some(1_000)).unwrap()],
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
    fn budget_overflow_detected_after_load() {
        let mut report = sample_report();
        report.phases[0] =
            PhaseTiming::try_new("allocate", 50, "Instant::elapsed", Some(10)).unwrap();
        let err = report.check_budgets().unwrap_err();
        assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
    }

    #[test]
    fn tracker_feeds_accounted_section() {
        let mut tracker = AccountedBytesTracker::new();
        tracker.allocate(128);
        let snap = tracker.snapshot("explicit_counter").unwrap();
        assert_eq!(snap.peak_bytes, 128);
        let (fx, cfg) = provenance_hashes(b"fixture", b"config");
        assert_eq!(fx.len(), 64);
        assert_eq!(cfg.len(), 64);
    }
}
