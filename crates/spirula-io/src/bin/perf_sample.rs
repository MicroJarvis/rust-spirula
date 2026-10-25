//! CPU micro-sample for P1-F08: generate a performance report, atomically
//! publish it, then read it back.
//!
//! Fixture bytes are hashed **and** executed. Config JSON is hashed **and**
//! parsed to define chunk size, iterations, and the resident resource budget.
//! Source revision/dirty remain explicitly unavailable (not inferred from CWD).

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use spirula_core::{
    DirtyStatus, EnvironmentInfo, ErrorCode, PERF_REPORT_VERSION, PerfProvenance, PerfReport,
    PhaseTiming, ProcessStage, SourceRevision, UnavailableMetric, process_exit_code,
};
use spirula_io::{
    PublishOptions, canonical_sample_config_json, canonical_sample_fixture, load_perf_report,
    run_measured_workload, save_perf_report,
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::from(0),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(process_exit_code(&error, ProcessStage::BeforeSession) as u8)
        }
    }
}

fn run() -> Result<(), spirula_core::DomainError> {
    let out = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| env::temp_dir().join("spirula-perf-sample-report.json"));

    let fixture_bytes = canonical_sample_fixture();
    let config_bytes = canonical_sample_config_json().into_bytes();
    let work = run_measured_workload(&fixture_bytes, &config_bytes)?;

    let rustc = option_env!("RUSTC_VERSION")
        .map(str::to_owned)
        .unwrap_or_else(|| {
            format!(
                "unavailable: compile-time RUSTC_VERSION unset (host {}-{})",
                env::consts::OS,
                env::consts::ARCH
            )
        });
    let environment = EnvironmentInfo::from_host(rustc)?;
    let provenance = PerfProvenance::try_new(
        work.fixture_sha256.clone(),
        work.config_sha256.clone(),
        SourceRevision::unavailable(
            "source revision digest unavailable: sample does not embed a git tree hash",
        )?,
        DirtyStatus::unavailable(
            "working-tree dirty status unavailable: sample does not inspect git status",
        )?,
        environment,
    )?;

    let wall_time_ns = work.wall_time.as_nanos() as u64;
    let report = PerfReport::try_new(
        PERF_REPORT_VERSION,
        work.config.feature.clone(),
        provenance,
        work.accounted.clone(),
        Some(work.config.resource_budget_bytes),
        UnavailableMetric::try_new(
            "OS process peak (RSS/working-set) is not sampled in P1-F08; use accounted bytes",
        )?,
        UnavailableMetric::try_new(
            "device-wide GPU memory is unavailable: P1-F08 has no GPU runtime path",
        )?,
        UnavailableMetric::try_new("GPU counters unavailable: P1-F08 has no GPU runtime path")?,
        UnavailableMetric::try_new(
            "GPU timestamp-query timing unavailable: P1-F08 has no GPU runtime path",
        )?,
        UnavailableMetric::try_new(
            "absolute wall-clock timestamps are not claimed; only phase durations are recorded",
        )?,
        vec![PhaseTiming::try_new(
            "accounted_allocate_free",
            wall_time_ns,
            "std::time::Instant::elapsed",
            Some(work.config.phase_budget_ns),
            Some(work.config.iterations),
        )?],
    )?;
    report.check_budgets()?;

    save_perf_report(&out, &report, PublishOptions::default())?;
    let loaded = load_perf_report(&out)?;
    loaded.validate()?;
    loaded.check_budgets()?;
    if loaded != report {
        return Err(spirula_core::DomainError::try_new(
            ErrorCode::IntegrityMismatch,
            "loaded perf report does not match the published report",
        )
        .expect("message"));
    }

    println!("wrote {}", out.display());
    println!(
        "fixture_sha256={} config_sha256={} checksum={}",
        work.fixture_sha256, work.config_sha256, work.checksum
    );
    println!(
        "accounted allocated={} current={} peak={} source={} scoped_resident_budget={} (work buffer only; not process RSS)",
        loaded.accounted.allocated_bytes,
        loaded.accounted.current_bytes,
        loaded.accounted.peak_bytes,
        loaded.accounted.source(),
        loaded.resource_budget_bytes.unwrap_or(0)
    );
    println!(
        "phases={} first_wall_ns={} work_count={}",
        loaded.phases.len(),
        loaded.phases[0].wall_time_ns,
        loaded.phases[0].work_count.unwrap_or(0)
    );
    Ok(())
}
