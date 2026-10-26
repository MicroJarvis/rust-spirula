//! Presentation helpers. No business state.
//!
//! Stdout presentation is asynchronous and bounded: the session control loop
//! never blocks on a full pipe or a stalled consumer. Overflow (queue or
//! kernel write WouldBlock) increments [`OutputSink::drops`] and drops the
//! line. Shutdown is out-of-band (`closed`) so a full queue cannot block
//! teardown past the join deadline.

use spirula_application::{Event, EventEnvelope, RunOutcome, SessionState};
use spirula_core::DomainError;
use std::io::{self, ErrorKind, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Stable ready markers for subprocess tests (stdout). Synchronize on these
/// lines — never on fixed sleeps.
pub const READY_PREFIX: &str = "spirula:ready ";

/// Default bounded presentation queue (lines). Full → drop + counter.
pub const OUTPUT_QUEUE_CAPACITY: usize = 256;

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
                set_stdout_nonblocking();
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
                            if !write_stdout_nonblocking(&line) {
                                drops_w.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        Ok(SinkMsg::Flush(ack)) => {
                            let _ = io::stdout().flush();
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

    /// Signal writer exit and join within `deadline`. Idempotent.
    pub fn shutdown(&self, deadline: Duration) -> bool {
        let end = Instant::now() + deadline;
        if self
            .closed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let _ = self.tx.try_send(SinkMsg::Shutdown);
        }
        let mut slot = self.join.lock().unwrap_or_else(|p| p.into_inner());
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
    let mut out = io::stdout().lock();
    let _ = write!(out, "{text}");
    let _ = out.flush();
}

fn set_stdout_nonblocking() {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let fd = io::stdout().as_raw_fd();
        // SAFETY: F_GETFL/F_SETFL on the process stdout fd only.
        #[allow(unsafe_code)] // fcntl O_NONBLOCK so a full pipe cannot stall the sink thread
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags >= 0 {
                let _ = libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
            }
        }
    }
}

/// Best-effort non-blocking write. Returns false when the line is dropped
/// (WouldBlock / Interrupted exhausted / other write error).
fn write_stdout_nonblocking(line: &str) -> bool {
    let mut out = io::stdout().lock();
    let mut bytes = line.as_bytes();
    let mut spins = 0u32;
    while !bytes.is_empty() {
        match out.write(bytes) {
            Ok(0) => return false,
            Ok(n) => bytes = &bytes[n..],
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                spins += 1;
                if spins > 32 {
                    return false;
                }
                thread::sleep(Duration::from_millis(1));
            }
            Err(_) => return false,
        }
    }
    let _ = out.flush();
    true
}
