//! Presentation helpers. No business state.
//!
//! Stdout presentation is asynchronous and bounded: the session control loop
//! never blocks on a full pipe. Lines enter a bounded `try_send` queue; a
//! dedicated writer thread performs **blocking** raw `write(2)` on the
//! process stdout fd **without** taking Rust's global `stdout()` lock and
//! **without** changing inherited file-status flags (`O_NONBLOCK` stays
//! untouched — `dup` shares the open-file description, so flag mutation
//! would leak to parents across second-SIGINT `SIG_DFL` death where `Drop`
//! does not run).
//!
//! Join/flush use absolute deadlines. A timed-out join abandons the writer
//! handle (control stays responsive); the abandoned thread may still block
//! inside kernel `write` until the pipe drains or the process exits.

use spirula_application::{Event, EventEnvelope, RunOutcome, SessionState};
use spirula_core::DomainError;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Stable ready markers for subprocess tests (stdout). Synchronize on these
/// lines — never on fixed sleeps.
pub const READY_PREFIX: &str = "spirula:ready ";

/// Default bounded presentation queue (lines). Full → drop + counter.
pub const OUTPUT_QUEUE_CAPACITY: usize = 256;

/// Per-line wall budget for a blocking raw write before the sink counts a drop
/// and moves on (writer may still be inside the kernel write until drained).
pub const WRITE_LINE_DEADLINE: Duration = Duration::from_millis(50);

pub fn help_text() -> String {
    let mut out = String::new();
    out.push_str("Spirula Studio CLI (P1-F09 minimal session entry)\n\n");
    out.push_str("Usage:\n");
    out.push_str("  spirula help\n");
    out.push_str("  spirula capabilities\n");
    out.push_str("  spirula probe-load --label <name> [--log-dir <path>]\n");
    out.push_str(
        "  spirula probe-run --steps <n> [--load-label <name>] [--log-dir <path>] [--step-delay-ms <n>] [--save|--no-save]\n",
    );
    out.push_str(
        "  spirula probe --label <name> [--steps <n>] [--log-dir <path>] [--step-delay-ms <n>]\n\n",
    );
    out.push_str("Unavailable in this build:\n");
    out.push_str("  train     (P5-F07)\n");
    out.push_str("  preview   (P4-F06)\n\n");
    out.push_str(
        "Exit codes: 0 success, 1 failure, 2 usage (before session), 130 cancel/SIGINT.\n",
    );
    out.push_str("SIGINT submits Stop{save:true}, waits for the terminal, then exits 130.\n");
    out.push_str("A second SIGINT restores SIG_DFL and re-raises so this delivery terminates.\n");
    out
}

pub fn capabilities_text() -> String {
    let mut out = String::new();
    out.push_str("capabilities:\n");
    out.push_str("  available: help, capabilities, probe-load, probe-run, probe\n");
    out.push_str("  unavailable: train, preview\n");
    out.push_str("  session: shared spirula-application (fake probes only)\n");
    out.push_str("  disk_log: optional --log-dir (application-owned rotated sink)\n");
    out.push_str("  errors: DomainError codes; SPIRULA_ERROR_CODES=1 appends (code: …)\n");
    out
}

pub fn show_error_codes() -> bool {
    std::env::var_os("SPIRULA_ERROR_CODES").is_some_and(|v| v == "1" || v == "true")
}

pub fn write_error(error: &DomainError) {
    // stderr stays synchronous and short (one line); not on the session path.
    let line = error.cli_stderr_line(show_error_codes());
    let mut err = io::stderr().lock();
    let _ = writeln!(err, "{line}");
}

/// Bounded async stdout sink. Control/SIGINT paths must use [`Self::try_write_line`]
/// / [`Self::try_write_event`] only — never blocking `writeln!` on stdout.
#[derive(Debug)]
pub struct OutputSink {
    tx: SyncSender<SinkMsg>,
    drops: Arc<AtomicU64>,
    closed: Arc<AtomicBool>,
    join: Mutex<Option<JoinHandle<()>>>,
}

enum SinkMsg {
    Line(String),
    Flush(SyncSender<()>),
    Shutdown,
}

impl OutputSink {
    pub fn start(capacity: usize) -> Self {
        let (tx, rx) = mpsc::sync_channel(capacity.max(1));
        let drops = Arc::new(AtomicU64::new(0));
        let closed = Arc::new(AtomicBool::new(false));
        let drops_w = Arc::clone(&drops);
        let closed_w = Arc::clone(&closed);
        let handle = thread::Builder::new()
            .name("spirula-cli-stdout".into())
            .spawn(move || {
                // Never fcntl(O_NONBLOCK) on inherited stdout — OFD is shared.
                loop {
                    if closed_w.load(Ordering::Acquire) {
                        while let Ok(msg) = rx.try_recv() {
                            match msg {
                                SinkMsg::Shutdown => return,
                                SinkMsg::Line(_) => {
                                    drops_w.fetch_add(1, Ordering::Relaxed);
                                }
                                SinkMsg::Flush(ack) => {
                                    let _ = ack.send(());
                                }
                            }
                        }
                        return;
                    }
                    match rx.recv_timeout(Duration::from_millis(20)) {
                        Ok(SinkMsg::Line(line)) => {
                            if !write_stdout_raw_blocking(&line) {
                                drops_w.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        Ok(SinkMsg::Flush(ack)) => {
                            let _ = flush_stdout_raw();
                            let _ = ack.send(());
                        }
                        Ok(SinkMsg::Shutdown) => return,
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
            })
            .expect("spawn stdout sink");
        Self {
            tx,
            drops,
            closed,
            join: Mutex::new(Some(handle)),
        }
    }

    pub fn drops(&self) -> u64 {
        self.drops.load(Ordering::Relaxed)
    }

    pub fn try_write_line(&self, line: impl Into<String>) -> bool {
        if self.closed.load(Ordering::Acquire) {
            self.drops.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        match self.tx.try_send(SinkMsg::Line(line.into())) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.drops.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    pub fn try_write_event(&self, envelope: &EventEnvelope) -> bool {
        self.try_write_line(format_event_line(envelope))
    }

    pub fn flush(&self, deadline: Duration) -> bool {
        if self.closed.load(Ordering::Acquire) {
            return true;
        }
        let end = Instant::now() + deadline;
        let (ack_tx, ack_rx) = mpsc::sync_channel(1);
        match self.tx.try_send(SinkMsg::Flush(ack_tx)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.drops.fetch_add(1, Ordering::Relaxed);
                return false;
            }
            Err(TrySendError::Disconnected(_)) => return true,
        }
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        match ack_rx.recv_timeout(left) {
            Ok(()) => true,
            Err(RecvTimeoutError::Timeout) => false,
            Err(RecvTimeoutError::Disconnected) => true,
        }
    }

    /// Signal writer exit and join within one absolute `deadline` (includes
    /// join-mutex acquisition). Concurrent callers that cannot acquire the
    /// mutex before the deadline return `false` without blocking on the holder.
    ///
    /// On timeout the join handle is retained (honest: writer may still be
    /// blocked in `write(2)`); control never waits past the deadline.
    pub fn shutdown(&self, deadline: Duration) -> bool {
        let end = Instant::now() + deadline;
        if self
            .closed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let _ = self.tx.try_send(SinkMsg::Shutdown);
        }
        let Some(mut slot) = try_lock_until(&self.join, end) else {
            return false;
        };
        let Some(handle) = slot.take() else {
            return true;
        };
        while Instant::now() < end {
            if handle.is_finished() {
                let _ = handle.join();
                return true;
            }
            thread::sleep(Duration::from_millis(1));
        }
        if handle.is_finished() {
            let _ = handle.join();
            return true;
        }
        *slot = Some(handle);
        false
    }
}

/// Acquire `mutex` without exceeding absolute `end`. Returns `None` on timeout.
fn try_lock_until<'a, T>(mutex: &'a Mutex<T>, end: Instant) -> Option<MutexGuard<'a, T>> {
    loop {
        match mutex.try_lock() {
            Ok(guard) => return Some(guard),
            Err(TryLockError::Poisoned(p)) => return Some(p.into_inner()),
            Err(TryLockError::WouldBlock) => {
                if Instant::now() >= end {
                    return None;
                }
                thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

impl Drop for OutputSink {
    fn drop(&mut self) {
        let _ = self.shutdown(Duration::from_millis(200));
    }
}

fn format_event_line(envelope: &EventEnvelope) -> String {
    match &envelope.event {
        Event::StateChanged { from, to, reason } => {
            let mut s = format!("state {} -> {} ({reason})\n", from.as_str(), to.as_str());
            if matches!(
                to,
                SessionState::Ready | SessionState::Running | SessionState::Loading
            ) {
                s.push_str(&format!("{READY_PREFIX}{}\n", to.as_str()));
            }
            s
        }
        Event::Progress(p) => {
            let total = p
                .total_steps
                .map(|t| t.to_string())
                .unwrap_or_else(|| "?".into());
            format!("progress {}/{total}\n", p.step)
        }
        Event::Log { level, message } => format!("log {level:?}: {message}\n"),
        Event::Warning { error } => format!("warning: {error}\n"),
        Event::CommandAccepted { command } => format!("accepted {command:?}\n"),
        Event::CommandRejected { command, error } => {
            format!("rejected {command:?}: {error}\n")
        }
        Event::Completed { outcome } => format!("completed {}\n", format_outcome(outcome)),
        Event::ProbeLoaded { label } => format!("probe-loaded {label}\n"),
        Event::ProbeStarted { steps } => format!("probe-started steps={steps}\n"),
        Event::ImagePrepared {
            path,
            width,
            height,
            orientation,
            turns_only,
        } => format!(
            "image-prepared path={path} {width}x{height} orient={orientation} turns_only={turns_only}\n"
        ),
        Event::MaskPrepared {
            path,
            width,
            height,
            kept,
            alpha,
        } => format!("mask-prepared path={path} {width}x{height} kept={kept} alpha={alpha}\n"),
        Event::CheckpointSaved { step, label } => {
            format!("checkpoint step={step} label={label}\n")
        }
        Event::ShutdownComplete => "shutdown-complete\n".into(),
    }
}

fn format_outcome(outcome: &RunOutcome) -> String {
    match outcome {
        RunOutcome::Completed { step } => format!("ok step={step}"),
        RunOutcome::Canceled {
            step,
            reason,
            saved_checkpoint,
        } => format!("canceled step={step} reason={reason:?} saved={saved_checkpoint:?}"),
        RunOutcome::Failed { error } => format!("failed: {error}"),
    }
}

/// Direct stdout write for non-session paths (help/capabilities).
pub fn write_stdout(text: &str) {
    let _ = write_stdout_raw_blocking(text);
}

/// Blocking raw write on stdout fd — does not take `std::io::stdout()` lock
/// and does not change file-status flags. Returns false on hard error.
///
/// Honest limit: a full pipe can block this call on the writer thread until
/// the consumer drains or the process exits; [`OutputSink::shutdown`] still
/// returns by its deadline.
fn write_stdout_raw_blocking(line: &str) -> bool {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let fd = io::stdout().as_raw_fd();
        let mut bytes = line.as_bytes();
        let deadline = Instant::now() + WRITE_LINE_DEADLINE;
        while !bytes.is_empty() {
            if Instant::now() >= deadline {
                // Soft drop: do not mutate flags; leave any partial kernel write.
                return false;
            }
            // Wait briefly for writability without setting O_NONBLOCK.
            if !poll_stdout_writable(fd, Duration::from_millis(5)) {
                continue;
            }
            #[allow(unsafe_code)] // raw write(2) on stdout; no fcntl flag changes
            let n = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
            if n < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                // EAGAIN should be rare without O_NONBLOCK; treat as soft drop.
                if err.kind() == io::ErrorKind::WouldBlock {
                    continue;
                }
                return false;
            }
            if n == 0 {
                return false;
            }
            bytes = &bytes[n as usize..];
        }
        true
    }
    #[cfg(not(unix))]
    {
        let mut out = io::stdout().lock();
        out.write_all(line.as_bytes()).is_ok()
    }
}

fn flush_stdout_raw() -> bool {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let fd = io::stdout().as_raw_fd();
        #[allow(unsafe_code)] // fsync/fdatasync not required; fcntl no-op flush via write of 0
        let _ = fd;
        // Stdout pipes have no portable flush; success is best-effort.
        true
    }
    #[cfg(not(unix))]
    {
        io::stdout().flush().is_ok()
    }
}

#[cfg(unix)]
fn poll_stdout_writable(fd: i32, timeout: Duration) -> bool {
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLOUT,
        revents: 0,
    };
    let ms = timeout.as_millis().min(i32::MAX as u128) as i32;
    #[allow(unsafe_code)] // poll(2) readiness only; does not change fd flags
    let rc = unsafe { libc::poll(&mut pfd, 1, ms) };
    rc > 0 && (pfd.revents & (libc::POLLOUT | libc::POLLHUP | libc::POLLERR)) != 0
}

#[cfg(test)]
#[allow(unsafe_code)] // fcntl flags probe; pipe/subprocess isolation for shutdown
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn sink_does_not_set_stdout_nonblock() {
        use std::os::fd::AsRawFd;
        let fd = io::stdout().as_raw_fd();
        let before = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        assert!(before >= 0);
        let sink = OutputSink::start(4);
        let _ = sink.try_write_line("flag-probe\n");
        let _ = sink.flush(Duration::from_millis(200));
        let mid = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        assert_eq!(
            mid & libc::O_NONBLOCK,
            before & libc::O_NONBLOCK,
            "writer must not arm O_NONBLOCK on inherited stdout"
        );
        assert!(sink.shutdown(Duration::from_secs(1)));
        let after = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        assert_eq!(after & libc::O_NONBLOCK, before & libc::O_NONBLOCK);
    }

    /// Isolated child body (public-API probe). Parent wires stdout to a pipe
    /// with no reader so `write(2)` blocks; never `dup2` the test harness.
    #[cfg(unix)]
    fn output_shutdown_contention_child() {
        use std::sync::Barrier;

        let out = Arc::new(OutputSink::start(4));
        assert!(out.try_write_line(format!("{}\n", "x".repeat(8 * 1024 * 1024))));
        thread::sleep(Duration::from_millis(30));

        let clone = Arc::clone(&out);
        let barrier = Arc::new(Barrier::new(2));
        let b = Arc::clone(&barrier);
        let long = thread::spawn(move || {
            b.wait();
            let _ = clone.shutdown(Duration::from_millis(500));
        });
        barrier.wait();
        thread::sleep(Duration::from_millis(30));

        let start = Instant::now();
        let ok = out.shutdown(Duration::from_millis(10));
        let elapsed = start.elapsed();
        // Evidence line for the parent (stderr; stdout is the stalled pipe).
        eprintln!(
            "short_shutdown deadline_ms=10 elapsed_ms={} result={ok}",
            elapsed.as_millis()
        );
        assert!(
            !ok,
            "short shutdown must not claim success under contention"
        );
        assert!(
            elapsed < Duration::from_millis(50),
            "absolute deadline must include lock wait (probe: 10ms not ~470ms): {elapsed:?}"
        );
        let _ = long.join();
        // Skip Drop join on the still-blocked writer; process exit closes stdout.
        std::process::exit(0);
    }

    #[test]
    #[cfg(unix)]
    fn concurrent_shutdown_deadline_includes_mutex_wait() {
        // Subprocess isolation (same shape as final-output-shutdown-probe):
        // parent owns the pipe + deadline/kill-reap; child runs OutputSink only.
        // Do not dup2 this harness's stdout — that races parallel libtests.
        const CHILD_ENV: &str = "SPIRULA_TEST_OUTPUT_SHUTDOWN_CHILD";
        if std::env::var_os(CHILD_ENV).is_some() {
            output_shutdown_contention_child();
            return;
        }

        use std::io::Read;
        use std::os::fd::FromRawFd;
        use std::process::{Command, Stdio};
        use std::sync::mpsc;

        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let (read_fd, write_fd) = (fds[0], fds[1]);

        let mut cmd = Command::new(std::env::current_exe().expect("test exe"));
        cmd.env(CHILD_ENV, "1")
            .args([
                "--exact",
                "present::tests::concurrent_shutdown_deadline_includes_mutex_wait",
                "--nocapture",
            ])
            .stdout(unsafe { Stdio::from_raw_fd(write_fd) })
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().expect("spawn shutdown child");
        // Release any parent-owned write end still held by `Command`.
        drop(cmd);

        let mut stderr = child.stderr.take().expect("child stderr");
        let (err_tx, err_rx) = mpsc::channel::<String>();
        thread::spawn(move || {
            let mut buf = String::new();
            let _ = stderr.read_to_string(&mut buf);
            let _ = err_tx.send(buf);
        });

        // Hold read_fd open without draining so the child's stdout write blocks.
        let deadline = Instant::now() + Duration::from_secs(3);
        let status = loop {
            match child.try_wait() {
                Ok(Some(st)) => break st,
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(10));
                }
                Ok(None) | Err(_) => {
                    let _ = child.kill();
                    break child.wait().expect("reap killed child");
                }
            }
        };
        unsafe {
            libc::close(read_fd);
        }
        let err = err_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_default();
        assert!(
            status.success(),
            "shutdown child failed: {status:?}; stderr={err}"
        );
        let line = err
            .lines()
            .find(|l| l.starts_with("short_shutdown "))
            .unwrap_or("");
        assert!(
            line.contains("result=false"),
            "expected short shutdown false under contention; stderr={err}"
        );
        let elapsed_ms = line
            .split_whitespace()
            .find_map(|tok| tok.strip_prefix("elapsed_ms=")?.parse::<u64>().ok())
            .expect("elapsed_ms in evidence line");
        assert!(
            elapsed_ms < 50,
            "short shutdown must not block on join lock (~10ms, not ~470ms): {line}"
        );
    }
}
