//! Subprocess + signal tests for P1-F09.
//!
//! Synchronize on observed `spirula:ready` lines — never fixed sleeps for
//! readiness. Reader/wait helpers use absolute deadlines and always kill/reap
//! on timeout.

#![allow(unsafe_code)] // libc::kill / wait status for SIGINT delivery

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

fn spirula_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_spirula"))
}

struct ChildSession {
    child: Child,
    stdout_rx: mpsc::Receiver<String>,
    collected: String,
}

impl ChildSession {
    fn spawn(args: &[&str], envs: &[(&str, &str)]) -> Self {
        let mut cmd = Command::new(spirula_bin());
        cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("spawn spirula");
        let mut stdout = child.stdout.take().expect("stdout");
        let (tx, rx) = mpsc::channel::<String>();
        thread::spawn(move || {
            let mut buf = [0u8; 512];
            let mut carry = String::new();
            loop {
                match stdout.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        carry.push_str(&String::from_utf8_lossy(&buf[..n]));
                        while let Some(pos) = carry.find('\n') {
                            let mut line = carry[..pos + 1].to_string();
                            carry.drain(..pos + 1);
                            if tx.send(line.split_off(0)).is_err() {
                                return;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            if !carry.is_empty() {
                let _ = tx.send(carry);
            }
        });
        Self {
            child,
            stdout_rx: rx,
            collected: String::new(),
        }
    }

    fn wait_stdout_contains(&mut self, needle: &str, budget: Duration) -> Result<(), String> {
        let deadline = Instant::now() + budget;
        while Instant::now() < deadline {
            let left = deadline.saturating_duration_since(Instant::now());
            match self
                .stdout_rx
                .recv_timeout(left.min(Duration::from_millis(50)))
            {
                Ok(line) => {
                    self.collected.push_str(&line);
                    if self.collected.contains(needle) {
                        return Ok(());
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        Err(format!(
            "timed out waiting for '{needle}' in:\n{}",
            self.collected
        ))
    }

    fn wait_exit(&mut self, budget: Duration) -> std::process::ExitStatus {
        let deadline = Instant::now() + budget;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return status,
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(10));
                }
                Ok(None) | Err(_) => {
                    let _ = self.child.kill();
                    return self.child.wait().expect("reap after kill");
                }
            }
        }
    }
}

impl Drop for ChildSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn run_output(args: &[&str], envs: &[(&str, &str)], budget: Duration) -> std::process::Output {
    let mut cmd = Command::new(spirula_bin());
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("spawn");
    let deadline = Instant::now() + budget;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().expect("output"),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) | Err(_) => {
                let _ = child.kill();
                return child.wait_with_output().expect("killed output");
            }
        }
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

#[test]
fn help_exits_zero() {
    let out = run_output(&["help"], &[], Duration::from_secs(5));
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("probe-load"));
}

#[test]
fn capabilities_lists_available_and_unavailable() {
    let out = run_output(&["capabilities"], &[], Duration::from_secs(5));
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("unavailable: train, preview"));
}

#[test]
fn train_rejected_usage_exit_2_one_stderr_line() {
    let out = run_output(&["train"], &[], Duration::from_secs(5));
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(err.lines().filter(|l| l.starts_with("error:")).count(), 1);
}

#[test]
fn preview_rejected_usage_exit_2() {
    let out = run_output(&["preview"], &[], Duration::from_secs(5));
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn unknown_command_exit_2() {
    let out = run_output(&["nope"], &[], Duration::from_secs(5));
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn rejected_probe_run_without_load_one_stderr_with_code() {
    // Ready is required for ProbeRun; without load the admission rejects.
    let out = run_output(
        &["probe-run", "--steps", "2"],
        &[("SPIRULA_ERROR_CODES", "1")],
        Duration::from_secs(10),
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        err.lines().filter(|l| l.starts_with("error:")).count(),
        1,
        "exactly one stderr error line, got:\n{err}"
    );
    assert!(
        err.contains("SP-SESS-001"),
        "expected stable code on stderr: {err}"
    );
}

#[test]
fn probe_empty_label_zero_steps_usage_exit_2_one_stderr() {
    let out = run_output(
        &["probe", "--label", "", "--steps", "0"],
        &[("SPIRULA_ERROR_CODES", "1")],
        Duration::from_secs(5),
    );
    assert_eq!(
        out.status.code(),
        Some(2),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(err.lines().filter(|l| l.starts_with("error:")).count(), 1);
    assert!(err.contains("SP-INP-001"), "{err}");
}

#[test]
fn help_and_capabilities_reject_unknown_flags_exit_2() {
    for args in [["help", "--nonsense"], ["capabilities", "--nonsense"]] {
        let out = run_output(
            &args,
            &[("SPIRULA_ERROR_CODES", "1")],
            Duration::from_secs(5),
        );
        assert_eq!(
            out.status.code(),
            Some(2),
            "args={args:?} stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(err.lines().filter(|l| l.starts_with("error:")).count(), 1);
        assert!(err.contains("SP-INP-002"), "{err}");
    }
}

#[test]
fn rejected_steps_zero_exit_2() {
    let out = run_output(
        &["probe-run", "--steps", "0", "--load-label", "ds"],
        &[],
        Duration::from_secs(5),
    );
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr)
            .lines()
            .filter(|l| l.starts_with("error:"))
            .count(),
        1
    );
}

fn count_completed_lines(stdout: &str) -> usize {
    stdout
        .lines()
        .filter(|l| l.starts_with("completed "))
        .count()
}

#[test]
fn probe_run_normal_success_reaches_run_terminal() {
    let out = run_output(
        &[
            "probe-run",
            "--steps",
            "2",
            "--load-label",
            "ds",
            "--step-delay-ms",
            "5",
        ],
        &[],
        Duration::from_secs(15),
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("spirula:ready training"),
        "must enter Running: {stdout}"
    );
    assert!(
        stdout.contains("probe-started") || stdout.contains("progress"),
        "must show run activity: {stdout}"
    );
    // Load terminal + run terminal — exactly once each (no live/query dup).
    assert_eq!(
        count_completed_lines(&stdout),
        2,
        "exactly one completed per command (ordered final state); got: {stdout}"
    );
    // Query/replay may surface a terminal before live under scheduling; the
    // acceptance gate is one ordered completed per command, not absence of
    // spirula:recovered.
}

#[test]
fn probe_run_failure_exit_1_one_stderr_error() {
    let out = run_output(
        &[
            "probe-run",
            "--steps",
            "2",
            "--load-label",
            "ds",
            "--step-delay-ms",
            "5",
        ],
        &[
            ("SPIRULA_TEST_WORKER_BEHAVIOR", "fail"),
            ("SPIRULA_ERROR_CODES", "1"),
        ],
        Duration::from_secs(15),
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(err.lines().filter(|l| l.starts_with("error:")).count(), 1);
    assert!(err.contains("(code:"), "{err}");
}

#[test]
fn sigint_at_load_run_gap_skips_probe_run_exit_130() {
    // Hold after load Ready so SIGINT lands in the Ready gap where Stop is
    // invalid; intent must still abort before ProbeRun.
    let mut session = ChildSession::spawn(
        &[
            "probe",
            "--label",
            "ds",
            "--steps",
            "100000",
            "--step-delay-ms",
            "20",
        ],
        &[("SPIRULA_TEST_GAP_AFTER_LOAD_MS", "800")],
    );
    session
        .wait_stdout_contains("spirula:ready ready", Duration::from_secs(10))
        .expect("load ready");
    #[cfg(unix)]
    {
        let pid = session.child.id() as i32;
        assert_eq!(unsafe { libc::kill(pid, libc::SIGINT) }, 0);
    }
    let status = session.wait_exit(Duration::from_secs(10));
    assert_eq!(
        status.code(),
        Some(130),
        "status={status:?} out={}",
        session.collected
    );
    assert!(
        !session.collected.contains("probe-started"),
        "must not start ProbeRun after cancel intent: {}",
        session.collected
    );
}

#[test]
fn sigint_while_stdout_blocked_still_cancels_before_long_run() {
    // Hold the stdout consumer so the pipe backs up. A blocking writeln would
    // stall the wait loop; OutputSink must still observe SIGINT and abort
    // before ProbeRun. Ready is observed via application disk log, not stdout.
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let log_dir =
        std::env::temp_dir().join(format!("spirula-cli-blocked-stdout-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&log_dir);
    std::fs::create_dir_all(&log_dir).expect("log dir");

    let release = Arc::new(AtomicBool::new(false));
    let release_reader = Arc::clone(&release);
    let mut child = Command::new(spirula_bin())
        .args([
            "probe",
            "--label",
            "ds",
            "--steps",
            "100000",
            "--step-delay-ms",
            "20",
            "--log-dir",
            log_dir.to_str().expect("utf8 path"),
        ])
        .env("SPIRULA_TEST_GAP_AFTER_LOAD_MS", "800")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");
    let mut stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    thread::spawn(move || {
        while !release_reader.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(10));
        }
        let mut buf = [0u8; 4096];
        let mut all = Vec::new();
        loop {
            match stdout.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => all.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }
        let _ = tx.send(all);
    });

    let ready_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let disk = std::fs::read_dir(&log_dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .collect::<String>();
        if disk.to_ascii_lowercase().contains("ready") {
            break;
        }
        if Instant::now() >= ready_deadline {
            release.store(true, Ordering::SeqCst);
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_dir_all(&log_dir);
            panic!("disk log never reached ready; got:\n{disk}");
        }
        thread::sleep(Duration::from_millis(10));
    }

    let pid = child.id() as i32;
    assert_eq!(unsafe { libc::kill(pid, libc::SIGINT) }, 0);
    // Keep stdout blocked briefly so cancel must not depend on draining.
    thread::sleep(Duration::from_millis(200));
    release.store(true, Ordering::SeqCst);

    let deadline = Instant::now() + Duration::from_secs(8);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                break child.wait().expect("reap");
            }
        }
    };
    let collected = rx.recv_timeout(Duration::from_secs(2)).unwrap_or_default();
    let text = String::from_utf8_lossy(&collected);
    let _ = std::fs::remove_dir_all(&log_dir);
    assert_eq!(status.code(), Some(130), "status={status:?} text={text}");
    assert!(
        !text.contains("probe-started steps=100000"),
        "must not run after cancel: {text}"
    );
    assert!(
        !text.contains("accepted ProbeRun"),
        "must not accept ProbeRun after cancel: {text}"
    );
}

#[test]
fn probe_run_sigint_exits_130() {
    let mut session = ChildSession::spawn(
        &[
            "probe-run",
            "--steps",
            "100000",
            "--load-label",
            "ds",
            "--step-delay-ms",
            "50",
        ],
        &[],
    );
    session
        .wait_stdout_contains("spirula:ready training", Duration::from_secs(10))
        .expect("ready");
    #[cfg(unix)]
    {
        let pid = session.child.id() as i32;
        assert_eq!(unsafe { libc::kill(pid, libc::SIGINT) }, 0);
    }
    let status = session.wait_exit(Duration::from_secs(10));
    assert_eq!(status.code(), Some(130), "status={status:?}");
}

#[test]
fn probe_run_second_sigint_terminates_with_exactly_two_signals() {
    // Deterministic unfinished-cancel gate: worker holds after cancel is
    // observed (SPIRULA_TEST_HOLD_AFTER_CANCEL) and emits an observed barrier
    // before any canceled terminal. wait_sliced's 50ms poll must not race a
    // cooperative exit 130 before the second SIGINT.
    let mut session = ChildSession::spawn(
        &[
            "probe-run",
            "--steps",
            "100000",
            "--load-label",
            "ds",
            "--step-delay-ms",
            "20",
        ],
        &[("SPIRULA_TEST_HOLD_AFTER_CANCEL", "1")],
    );
    session
        .wait_stdout_contains("spirula:ready training", Duration::from_secs(15))
        .expect("ready");
    #[cfg(unix)]
    {
        let pid = session.child.id() as i32;
        assert_eq!(unsafe { libc::kill(pid, libc::SIGINT) }, 0);
        session
            .wait_stdout_contains("spirula:test cancel-held", Duration::from_secs(10))
            .expect("cancel-held barrier");
        assert_eq!(unsafe { libc::kill(pid, libc::SIGINT) }, 0);
    }
    let status = session.wait_exit(Duration::from_secs(10));
    // Must be SIGINT termination — not a clean exit 130 from cooperative stop.
    assert_eq!(
        status.signal(),
        Some(libc::SIGINT),
        "expected SIGINT termination after exactly 2 signals, got {status:?}; stdout={}",
        session.collected
    );
    assert!(status.code().is_none(), "must not be a normal exit code");
}

#[test]
fn probe_with_log_dir_writes_diagnostics() {
    let dir = std::env::temp_dir().join(format!("spirula-cli-log-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let out = run_output(
        &[
            "probe",
            "--label",
            "ds",
            "--steps",
            "2",
            "--step-delay-ms",
            "5",
            "--log-dir",
            dir.to_str().unwrap(),
        ],
        &[("SPIRULA_TEST_DISK_LIMITS", "1")],
        Duration::from_secs(15),
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        dir.join("spirula-diagnostics.log").exists()
            || std::fs::read_dir(&dir).unwrap().next().is_some(),
        "no diagnostics under {dir:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Adapter-level proof: tiny notify/progress queues + Progress flood force
/// dropped live terminals; CLI must restore via command_result (success).
#[test]
fn adapter_recovers_terminal_under_slow_consumer_pressure() {
    let out = run_output(
        &[
            "probe-run",
            "--steps",
            "120",
            "--load-label",
            "ds",
            "--step-delay-ms",
            "1",
        ],
        &[("SPIRULA_TEST_TINY_NOTIFY", "1")],
        Duration::from_secs(30),
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("spirula:test notify-drops=")
            && !stdout.contains("spirula:test notify-drops=0"),
        "must deliberately force notify drops: {stdout}"
    );
    assert!(
        stdout.contains("spirula:recovered"),
        "must prove query/replay restoration under notify pressure: {stdout}"
    );
    assert_eq!(
        count_completed_lines(&stdout),
        2,
        "one completed per command under pressure; got: {stdout}"
    );
}

/// Same pressure as success recovery, but with an injected run failure.
#[test]
fn adapter_recovers_failure_under_slow_consumer_pressure() {
    let out = run_output(
        &[
            "probe-run",
            "--steps",
            "120",
            "--load-label",
            "ds",
            "--step-delay-ms",
            "1",
        ],
        &[
            ("SPIRULA_TEST_TINY_NOTIFY", "1"),
            ("SPIRULA_TEST_WORKER_BEHAVIOR", "fail"),
        ],
        Duration::from_secs(30),
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("spirula:test notify-drops=")
            && !stdout.contains("spirula:test notify-drops=0"),
        "must deliberately force notify drops: {stdout}"
    );
    assert!(
        stdout.contains("spirula:recovered"),
        "failure terminal must restore via query under pressure: {stdout}"
    );
    // Injected fail aborts at load — exactly one ordered completed terminal.
    assert_eq!(
        count_completed_lines(&stdout),
        1,
        "exactly one completed for the failed load under pressure; got: {stdout}"
    );
    assert_eq!(
        stderr.lines().filter(|l| l.starts_with("error:")).count(),
        1,
        "stderr={stderr}"
    );
}

/// Parent keeps a dup of the child's stdout write OFD; child must never arm
/// `O_NONBLOCK` on that shared description (RAII restore is insufficient for
/// second-SIGINT `SIG_DFL` death where `Drop` does not run).
///
/// `Command::spawn` takes `&mut self` and may keep the parent-owned write fd
/// until `Command` is dropped — drop it immediately after spawn so drain can
/// see EOF once the child exits and `watch_fd` is closed. Never treat dup as
/// OFD isolation.
#[cfg(unix)]
struct StdoutFlagWatch {
    child: Child,
    captured: std::sync::Arc<std::sync::Mutex<String>>,
    watch_fd: i32,
    before: i32,
    drain_done: mpsc::Receiver<()>,
    /// Kept so we can join after a bounded wait; abandoned on timeout.
    drain: Option<thread::JoinHandle<()>>,
}

#[cfg(unix)]
impl StdoutFlagWatch {
    fn spawn(args: &[&str], envs: &[(&str, &str)]) -> Self {
        use std::os::fd::FromRawFd;

        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let (read_fd, write_fd) = (fds[0], fds[1]);
        let watch_fd = unsafe { libc::dup(write_fd) };
        assert!(watch_fd >= 0);
        let before = unsafe { libc::fcntl(watch_fd, libc::F_GETFL) };
        assert!(before >= 0);

        let mut cmd = Command::new(spirula_bin());
        cmd.args(args)
            .stdout(unsafe { Stdio::from_raw_fd(write_fd) });
        cmd.stderr(Stdio::null());
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let child = cmd.spawn().expect("spawn");
        // Critical: release any parent-owned write end still held by `cmd`.
        // Without this, closing `watch_fd` alone never delivers EOF to drain.
        drop(cmd);

        let captured = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let captured_w = std::sync::Arc::clone(&captured);
        let (done_tx, drain_done) = mpsc::channel();
        let drain = thread::spawn(move || {
            let mut reader = unsafe { std::fs::File::from_raw_fd(read_fd) };
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Ok(mut g) = captured_w.lock() {
                            g.push_str(&String::from_utf8_lossy(&buf[..n]));
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = done_tx.send(());
        });

        Self {
            child,
            captured,
            watch_fd,
            before,
            drain_done,
            drain: Some(drain),
        }
    }

    fn pid(&self) -> i32 {
        self.child.id() as i32
    }

    fn wait_stdout_contains(&self, needle: &str, budget: Duration) -> Result<(), String> {
        let deadline = Instant::now() + budget;
        while Instant::now() < deadline {
            {
                let g = self.captured.lock().expect("captured");
                if g.contains(needle) {
                    return Ok(());
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        let got = self.captured.lock().expect("captured").clone();
        Err(format!("timed out waiting for '{needle}' in:\n{got}"))
    }

    fn wait_exit(&mut self, budget: Duration) -> std::process::ExitStatus {
        let deadline = Instant::now() + budget;
        loop {
            match self.child.try_wait() {
                Ok(Some(st)) => return st,
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(10));
                }
                Ok(None) | Err(_) => {
                    let _ = self.child.kill();
                    return self.child.wait().expect("reap");
                }
            }
        }
    }

    /// Sample OFD flags, close the last parent write end, bounded-drain join.
    fn finish(mut self) -> (i32, i32, std::process::ExitStatus) {
        let status = self.wait_exit(Duration::from_secs(12));
        let after = unsafe { libc::fcntl(self.watch_fd, libc::F_GETFL) };
        unsafe {
            libc::close(self.watch_fd);
        }
        self.watch_fd = -1;
        // Child exited + cmd dropped + watch_fd closed ⇒ all write ends gone.
        match self.drain_done.recv_timeout(Duration::from_secs(2)) {
            Ok(()) => {
                if let Some(h) = self.drain.take() {
                    let _ = h.join();
                }
            }
            Err(_) => {
                // Do not join unboundedly; abandon the drain thread.
                self.drain.take();
            }
        }
        (self.before, after, status)
    }
}

#[cfg(unix)]
impl Drop for StdoutFlagWatch {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if self.watch_fd >= 0 {
            unsafe {
                libc::close(self.watch_fd);
            }
            self.watch_fd = -1;
        }
        // Best-effort bounded drain wake; ignore timeout.
        let _ = self.drain_done.recv_timeout(Duration::from_millis(200));
        self.drain.take();
    }
}

#[cfg(unix)]
#[test]
fn stdout_ofd_flags_unchanged_after_normal_exit() {
    let watch = StdoutFlagWatch::spawn(
        &[
            "probe-run",
            "--steps",
            "1",
            "--load-label",
            "ds",
            "--step-delay-ms",
            "1",
        ],
        &[],
    );
    let (before, after, status) = watch.finish();
    assert_eq!(status.code(), Some(0), "{status:?}");
    assert_eq!(
        after & libc::O_NONBLOCK,
        before & libc::O_NONBLOCK,
        "normal exit must not leave O_NONBLOCK on shared OFD"
    );
}

#[cfg(unix)]
#[test]
fn stdout_ofd_flags_unchanged_after_failure_exit() {
    let watch = StdoutFlagWatch::spawn(
        &[
            "probe-run",
            "--steps",
            "1",
            "--load-label",
            "ds",
            "--step-delay-ms",
            "1",
        ],
        &[("SPIRULA_TEST_WORKER_BEHAVIOR", "fail")],
    );
    let (before, after, status) = watch.finish();
    assert_eq!(status.code(), Some(1), "{status:?}");
    assert_eq!(
        after & libc::O_NONBLOCK,
        before & libc::O_NONBLOCK,
        "failure exit must not leave O_NONBLOCK on shared OFD"
    );
}

#[cfg(unix)]
#[test]
fn stdout_ofd_flags_unchanged_after_first_sigint_exit_130() {
    let watch = StdoutFlagWatch::spawn(
        &[
            "probe-run",
            "--steps",
            "100000",
            "--load-label",
            "ds",
            "--step-delay-ms",
            "20",
        ],
        &[],
    );
    watch
        .wait_stdout_contains("spirula:ready training", Duration::from_secs(15))
        .expect("ready");
    assert_eq!(unsafe { libc::kill(watch.pid(), libc::SIGINT) }, 0);
    let (before, after, status) = watch.finish();
    assert_eq!(status.code(), Some(130), "{status:?}");
    assert_eq!(
        after & libc::O_NONBLOCK,
        before & libc::O_NONBLOCK,
        "first SIGINT exit 130 must not leave O_NONBLOCK on shared OFD"
    );
}

#[cfg(unix)]
#[test]
fn stdout_ofd_flags_unchanged_after_second_sigint_dfl_death() {
    // Drop does not run on SIG_DFL termination — flags must never have been armed.
    let watch = StdoutFlagWatch::spawn(
        &[
            "probe-run",
            "--steps",
            "100000",
            "--load-label",
            "ds",
            "--step-delay-ms",
            "20",
        ],
        &[("SPIRULA_TEST_HOLD_AFTER_CANCEL", "1")],
    );
    watch
        .wait_stdout_contains("spirula:ready training", Duration::from_secs(15))
        .expect("ready");
    assert_eq!(unsafe { libc::kill(watch.pid(), libc::SIGINT) }, 0);
    watch
        .wait_stdout_contains("spirula:test cancel-held", Duration::from_secs(10))
        .expect("cancel-held barrier");
    assert_eq!(unsafe { libc::kill(watch.pid(), libc::SIGINT) }, 0);
    let (before, after, status) = watch.finish();
    assert_eq!(
        status.signal(),
        Some(libc::SIGINT),
        "expected SIGINT death, got {status:?}"
    );
    assert_eq!(
        after & libc::O_NONBLOCK,
        before & libc::O_NONBLOCK,
        "second-SIGINT DFL death must not leave O_NONBLOCK on shared OFD"
    );
}

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
    assert!(app.wait_until(Duration::from_secs(2), |_| {
        matches!(app.command_result(id), CommandResult::Ready(_))
    }));
    match app.command_result(id) {
        CommandResult::Ready(env) => {
            assert!(matches!(env.event, Event::Completed { .. }));
        }
        other => panic!("expected Ready, got {other:?}"),
    }
}

#[allow(dead_code)]
fn _write_sink(w: &mut impl Write) {
    let _ = w;
}
