//! CPU micro-sample for P1-F08: generate a performance report, atomically
//! publish it, then read it back. Accounted bytes come from an explicit
//! counter; OS/GPU/timestamp fields stay unavailable with reasons.

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use spirula_core::{
    AccountedBytesTracker, DirtyStatus, EnvironmentInfo, ErrorCode, PERF_REPORT_VERSION,
    PerfProvenance, PerfReport, PhaseTiming, ProcessStage, SourceRevision, UnavailableMetric,
    process_exit_code,
};
use spirula_io::{PublishOptions, load_perf_report, provenance_hashes, save_perf_report};

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

    // Stand-in fixture/config payloads hashed into provenance (not guessed metrics).
    let fixture_bytes = b"spirula-p1-f08-sample-fixture-v1";
    let config_bytes = b"{\"sample\":true,\"feature\":\"P1-F08\"}";
    let (fixture_sha256, config_sha256) = provenance_hashes(fixture_bytes, config_bytes);

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
        fixture_sha256,
        config_sha256,
        SourceRevision::unavailable(
            "source revision digest unavailable: sample does not embed a git tree hash",
        )?,
        DirtyStatus::unavailable(
            "working-tree dirty status unavailable: sample does not inspect git status",
        )?,
        environment,
    )?;

    const RESOURCE_BUDGET_BYTES: u64 = 256 * 1024;
    const WORK_COUNT: u64 = 8;
    let mut tracker = AccountedBytesTracker::new();
    let phase_started = Instant::now();
    for _ in 0..WORK_COUNT {
        let chunk_len = 8 * 1024_u64;
        tracker.preflight(chunk_len, RESOURCE_BUDGET_BYTES)?;
        let chunk = vec![0u8; chunk_len as usize];
        tracker.allocate(chunk_len)?;
        let checksum = chunk.iter().fold(0u8, |acc, b| acc.wrapping_add(*b));
        std::hint::black_box(checksum);
        tracker.free(chunk_len)?;
    }
    let wall_time_ns = phase_started.elapsed().as_nanos() as u64;
    let accounted = tracker.snapshot("explicit_counter")?;

    let report = PerfReport::try_new(
        PERF_REPORT_VERSION,
        "P1-F08",
        provenance,
        accounted,
        Some(RESOURCE_BUDGET_BYTES),
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
            Some(60_000_000_000),
            Some(WORK_COUNT),
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
        "accounted allocated={} current={} peak={} source={} budget={}",
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
