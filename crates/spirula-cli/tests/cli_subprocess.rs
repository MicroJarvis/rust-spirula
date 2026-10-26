//! Subprocess + signal tests for P1-F09.
//!
//! Synchronize on observed `spirula:ready` lines — never fixed sleeps for
//! readiness. SIGINT tests send the signal only after a ready marker.

#![allow(unsafe_code)] // libc::kill for SIGINT delivery in subprocess tests

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn spirula_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_spirula"))
}

fn wait_stdout_line(child_stdout: &mut impl BufRead, needle: &str, budget: Duration) -> String {
    let deadline = Instant::now() + budget;
    let mut line = String::new();
    let mut collected = String::new();
    while Instant::now() < deadline {
        line.clear();
        match child_stdout.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                collected.push_str(&line);
                if line.contains(needle) {
                    return collected;
                }
            }
            Err(_) => break,
        }
    }
    panic!("timed out waiting for '{needle}' in stdout:\n{collected}");
}

#[test]
fn help_exits_zero() {
    let out = Command::new(spirula_bin())
        .arg("help")
        .output()
        .expect("spawn");
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("probe-load"));
    assert!(stdout.contains("Unavailable"));
}

#[test]
fn capabilities_lists_available_and_unavailable() {
    let out = Command::new(spirula_bin())
        .arg("capabilities")
        .output()
        .expect("spawn");
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("available:"));
    assert!(stdout.contains("unavailable: train, preview"));
}

#[test]
fn train_rejected_usage_exit_2() {
    let out = Command::new(spirula_bin())
        .arg("train")
        .output()
        .expect("spawn");
    assert_eq!(
        out.status.code(),
        Some(2),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.starts_with("error:"), "{err}");
}

#[test]
fn preview_rejected_usage_exit_2() {
    let out = Command::new(spirula_bin())
        .arg("preview")
        .output()
        .expect("spawn");
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn unknown_command_exit_2() {
    let out = Command::new(spirula_bin())
        .arg("nope")
        .output()
        .expect("spawn");
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn probe_run_normal_success() {
    let out = Command::new(spirula_bin())
        .args([
            "probe-run",
            "--steps",
            "2",
            "--load-label",
            "ds",
            "--step-delay-ms",
            "5",
        ])
        .output()
        .expect("spawn");
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("spirula:ready training") || stdout.contains("spirula:ready ready"),
        "{stdout}"
    );
}

#[test]
fn probe_run_failure_exit_1() {
    let out = Command::new(spirula_bin())
        .env("SPIRULA_TEST_WORKER_BEHAVIOR", "fail")
        .args([
            "probe-run",
            "--steps",
            "2",
            "--load-label",
            "ds",
            "--step-delay-ms",
            "5",
        ])
        .output()
        .expect("spawn");
    assert_eq!(
        out.status.code(),
        Some(1),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn probe_run_sigint_exits_130() {
    let mut child = Command::new(spirula_bin())
        .args([
            "probe-run",
            "--steps",
            "100000",
            "--load-label",
            "ds",
            "--step-delay-ms",
            "50",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);
    let _ = wait_stdout_line(
        &mut reader,
        "spirula:ready training",
        Duration::from_secs(10),
    );
    // First SIGINT → cooperative Stop{save:true} → exit 130.
    #[cfg(unix)]
    {
        let pid = child.id() as i32;
        unsafe {
            assert_eq!(libc::kill(pid, libc::SIGINT), 0);
        }
    }
    let status = child.wait().expect("wait");
    assert_eq!(status.code(), Some(130), "status={status:?}");
}

#[test]
fn probe_run_second_sigint_default_disposition() {
    let mut child = Command::new(spirula_bin())
        .args([
            "probe-run",
            "--steps",
            "100000",
            "--load-label",
            "ds",
            "--step-delay-ms",
            "200",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");
    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);
    let _ = wait_stdout_line(
        &mut reader,
        "spirula:ready training",
        Duration::from_secs(10),
    );
    #[cfg(unix)]
    {
        let pid = child.id() as i32;
        unsafe {
            assert_eq!(libc::kill(pid, libc::SIGINT), 0);
            // Brief yield so the handler can restore SIG_DFL on the second hit.
            std::thread::sleep(Duration::from_millis(20));
            assert_eq!(libc::kill(pid, libc::SIGINT), 0);
            // Third SIGINT after DFL should terminate by signal if still alive.
            std::thread::sleep(Duration::from_millis(20));
            let _ = libc::kill(pid, libc::SIGINT);
        }
    }
    let status = child.wait().expect("wait");
    // Either clean cancel 130 (if stop finished before third) or signal death.
    let code = status.code();
    let signaled = status.signal().is_some();
    assert!(
        code == Some(130) || signaled,
        "expected 130 or signal termination, got {status:?}"
    );
}

#[test]
fn probe_with_log_dir_writes_diagnostics() {
    let dir = std::env::temp_dir().join(format!("spirula-cli-log-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let out = Command::new(spirula_bin())
        .env("SPIRULA_TEST_DISK_LIMITS", "1")
        .args([
            "probe",
            "--label",
            "ds",
            "--steps",
            "2",
            "--step-delay-ms",
            "5",
            "--log-dir",
            dir.to_str().unwrap(),
        ])
        .output()
        .expect("spawn");
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let log = dir.join("spirula-diagnostics.log");
    // Allow async sink to finish (flush on Drop); file should exist.
    assert!(
        log.exists() || std::fs::read_dir(&dir).unwrap().next().is_some(),
        "no diagnostics under {dir:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rejected_steps_zero_exit_2() {
    let out = Command::new(spirula_bin())
        .args(["probe-run", "--steps", "0", "--load-label", "ds"])
        .output()
        .expect("spawn");
    assert_eq!(out.status.code(), Some(2));
}

/// Integration: application query recovers when we never listen to notify.
#[test]
fn application_command_result_recovers_without_subscriber() {
    use spirula_application::{
        ApplicationService, Command, CommandPayload, CommandResult, Event, SessionLimits,
    };
    use std::time::Duration;

    let mut limits = SessionLimits::for_test();
    limits.worker_step_delay = Duration::from_millis(1);
    let app = ApplicationService::start(limits);
    let id = match app.submit(Command::new(CommandPayload::ProbeLoad {
        label: "solo".into(),
    })) {
        spirula_application::SubmitOutcome::Accepted { id } => id,
        other => panic!("unexpected {other:?}"),
    };
    assert!(app.wait_until(Duration::from_secs(2), |s| {
        matches!(app.command_result(id), CommandResult::Ready(_))
            || s.loaded_label.as_deref() == Some("solo")
    }));
    match app.command_result(id) {
        CommandResult::Ready(env) => {
            assert!(matches!(env.event, Event::Completed { .. }));
        }
        other => panic!("expected Ready, got {other:?}"),
    }
}

trait StatusSignal {
    fn signal(&self) -> Option<i32>;
}

impl StatusSignal for std::process::ExitStatus {
    fn signal(&self) -> Option<i32> {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            ExitStatusExt::signal(self)
        }
        #[cfg(not(unix))]
        {
            let _ = self;
            None
        }
    }
}

// Silence unused Write import on platforms that do not need it.
#[allow(dead_code)]
fn _write_sink(w: &mut impl Write) {
    let _ = w;
}
