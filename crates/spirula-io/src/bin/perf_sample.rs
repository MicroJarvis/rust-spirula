//! CPU micro-sample for P1-F08: generate a performance report, atomically
//! publish it, then read it back. Accounted bytes come from an explicit
//! counter; OS/GPU/timestamp fields stay unavailable with reasons.

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use spirula_core::{
    AccountedBytesTracker, EnvironmentInfo, ErrorCode, PERF_REPORT_VERSION, PerfProvenance,
    PerfReport, PhaseTiming, ProcessStage, UnavailableMetric, process_exit_code,
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
    let provenance = PerfProvenance::try_new(fixture_sha256, config_sha256, environment)?;

    let mut tracker = AccountedBytesTracker::new();
    let phase_started = Instant::now();
    {
        let chunk = vec![0u8; 64 * 1024];
        tracker.allocate(chunk.len() as u64);
        // Touch bytes so the allocation is not optimized away.
        let checksum = chunk.iter().fold(0u8, |acc, b| acc.wrapping_add(*b));
        std::hint::black_box(checksum);
        tracker.free(chunk.len() as u64);
    }
    let wall_time_ms = phase_started.elapsed().as_millis() as u64;
    let accounted = tracker.snapshot("explicit_counter")?;

    let report = PerfReport::try_new(
        PERF_REPORT_VERSION,
        "P1-F08",
        provenance,
        accounted,
        UnavailableMetric::try_new(
            "OS process peak (RSS/working-set) is not sampled in P1-F08; use accounted bytes",
        )?,
        UnavailableMetric::try_new(
            "device-wide GPU memory is unavailable: P1-F08 has no GPU runtime path",
        )?,
        UnavailableMetric::try_new(
            "GPU timing/counters unavailable: P1-F08 has no GPU runtime path",
        )?,
        UnavailableMetric::try_new(
            "absolute wall-clock timestamps are not claimed; only phase durations are recorded",
        )?,
        vec![PhaseTiming::try_new(
            "accounted_allocate_free",
            wall_time_ms,
            "std::time::Instant::elapsed",
            Some(60_000),
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
        "accounted allocated={} current={} peak={} source={}",
        loaded.accounted.allocated_bytes,
        loaded.accounted.current_bytes,
        loaded.accounted.peak_bytes,
        loaded.accounted.source()
    );
    println!(
        "phases={} first_wall_ms={}",
        loaded.phases.len(),
        loaded.phases[0].wall_time_ms
    );
    Ok(())
}
