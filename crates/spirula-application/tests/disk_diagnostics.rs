//! P1-F09: session-owned rotated disk diagnostics.

use spirula_application::{
    ApplicationService, Command, CommandPayload, DiskLogConfig, DiskLogLimits, Event, SessionLimits,
};
use spirula_core::{ContextKey, ErrorCode};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn temp_dir(label: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("spirula-app-disk-{}-{}", label, std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

#[test]
fn session_mirrors_probe_logs_to_disk() {
    let dir = temp_dir("mirror");
    let mut limits = SessionLimits::for_test();
    limits.worker_step_delay = Duration::from_millis(1);
    let disk = DiskLogConfig::new(&dir).with_limits(DiskLogLimits::for_test());
    let app = ApplicationService::start_with(limits, Some(disk)).expect("start");
    let _ = app.submit(Command::new(CommandPayload::ProbeLoad {
        label: "d".into(),
    }));
    assert!(app.wait_until(Duration::from_secs(2), |s| {
        s.loaded_label.as_deref() == Some("d")
    }));
    let id = match app.submit(Command::new(CommandPayload::ProbeRun { steps: 2 })) {
        spirula_application::SubmitOutcome::Accepted { id } => id,
        other => panic!("{other:?}"),
    };
    assert!(app.wait_until(Duration::from_secs(2), |_| {
        matches!(
            app.command_result(id),
            spirula_application::CommandResult::Ready(_)
        )
    }));
    assert!(app.flush_disk_log(Duration::from_secs(1)));
    let counters = app.disk_log_counters().expect("disk configured");
    assert!(
        counters.accepted >= 1 || counters.written >= 1,
        "{counters:?}"
    );
    let log = dir.join("spirula-diagnostics.log");
    let text = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        text.contains("probe")
            || text.contains("STATE")
            || text.contains("COMPLETED")
            || counters.written >= 1,
        "log={text:?} counters={counters:?}"
    );
    drop(app);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn shutdown_joins_disk_sink() {
    let dir = temp_dir("shutdown");
    let disk = DiskLogConfig::new(&dir).with_limits(DiskLogLimits::for_test());
    let app = ApplicationService::start_with(SessionLimits::for_test(), Some(disk)).expect("start");
    let _ = app.submit(Command::new(CommandPayload::Shutdown));
    assert!(app.wait_until(Duration::from_secs(2), |s| {
        s.state == spirula_application::SessionState::Closed
    }));
    drop(app);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn invalid_file_stem_rejected_before_session() {
    let dir = temp_dir("badstem");
    let cfg = DiskLogConfig {
        directory: dir.clone(),
        limits: DiskLogLimits::for_test(),
        file_stem: "../escape.log".into(),
    };
    match ApplicationService::start_with(SessionLimits::for_test(), Some(cfg)) {
        Err(err) => assert_eq!(err.code(), spirula_core::ErrorCode::InvalidArgument),
        Ok(_) => panic!("expected invalid file_stem rejection"),
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn disk_failure_warning_honors_session_error_bounds() {
    // Public-API probe (a67335e): tiny caps must still bound the disk Warning.
    let dir = temp_dir("bound-warn");
    let blocker = dir.join("blocker");
    fs::write(&blocker, b"file").unwrap();
    let limits = SessionLimits {
        error_message_max_bytes: 9,
        error_event_max_bytes: 73,
        ..SessionLimits::default()
    };
    let disk = DiskLogConfig::new(blocker.join("logs")).with_limits(DiskLogLimits::for_test());
    let app = ApplicationService::start_with(limits, Some(disk)).expect("start");
    let effective = app.limits();
    let (sub, _) = app.subscribe().expect("subscribe");
    let _ = app.submit(Command::new(CommandPayload::ProbeLoad {
        label: "ds".into(),
    }));
    let end = Instant::now() + Duration::from_secs(2);
    let mut saw = false;
    while Instant::now() < end {
        if let Ok(env) = sub.recv_timeout(Duration::from_millis(50))
            && let Event::Warning { error } = env.event
        {
            assert_eq!(error.code(), ErrorCode::WriteFailed);
            assert!(
                error.message().len() <= effective.error_message_max_bytes,
                "message_len={} max={} msg={:?}",
                error.message().len(),
                effective.error_message_max_bytes,
                error.message()
            );
            let approx = 64
                + error.message().len()
                + error
                    .context()
                    .iter()
                    .map(|(k, v)| k.as_str().len() + v.len() + 8)
                    .sum::<usize>();
            assert!(
                approx <= effective.error_event_max_bytes,
                "approx={approx} max={}",
                effective.error_event_max_bytes
            );
            // Non-recursive marker when caps still allow it.
            if let Some(field) = error.context_value(ContextKey::Field) {
                assert_eq!(field, "disk_log");
            }
            saw = true;
            break;
        }
    }
    assert!(saw, "expected disk failure Warning");
    drop(app);
    let _ = fs::remove_dir_all(&dir);
}
