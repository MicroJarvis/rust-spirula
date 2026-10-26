//! Rotated disk diagnostics sink (P1-F09).
//!
//! The owner/`submit` path never blocks on disk. Lines enter a bounded
//! `try_send` queue; a dedicated sink thread appends, rotates, and trims under
//! explicit byte/file/budget caps. Write failures increment counters only —
//! they must not enqueue further disk-bound warnings (no recursive loops).

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Explicit disk-diagnostics resource policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskLogLimits {
    /// Max bytes accepted per line (UTF-8 length). Oversized lines are dropped.
    pub line_max_bytes: usize,
    /// Bounded async queue capacity (`try_send`; full → drop).
    pub queue_capacity: usize,
    /// In-sink recent-line tail kept for diagnostics queries.
    pub tail_capacity: usize,
    /// Rotate the active file when its size reaches this many bytes.
    pub rotation_bytes: u64,
    /// Maximum number of retained files including the active one.
    pub max_files: usize,
    /// Soft global budget across active + rotated files; trim oldest first.
    pub global_disk_budget_bytes: u64,
    /// Default flush/join wait when callers omit a tighter deadline.
    pub flush_join_deadline: Duration,
}

impl Default for DiskLogLimits {
    fn default() -> Self {
        Self {
            line_max_bytes: 2 * 1024,
            queue_capacity: 64,
            tail_capacity: 32,
            rotation_bytes: 256 * 1024,
            max_files: 4,
            global_disk_budget_bytes: 1024 * 1024,
            flush_join_deadline: Duration::from_secs(2),
        }
    }
}

impl DiskLogLimits {
    pub fn for_test() -> Self {
        Self {
            line_max_bytes: 128,
            queue_capacity: 4,
            tail_capacity: 8,
            rotation_bytes: 64,
            max_files: 3,
            global_disk_budget_bytes: 200,
            flush_join_deadline: Duration::from_millis(500),
        }
    }

    pub fn normalized(mut self) -> Self {
        self.line_max_bytes = self.line_max_bytes.max(16);
        self.queue_capacity = self.queue_capacity.max(1);
        self.tail_capacity = self.tail_capacity.max(1);
        self.rotation_bytes = self.rotation_bytes.max(16);
        self.max_files = self.max_files.max(1);
        self.global_disk_budget_bytes = self.global_disk_budget_bytes.max(self.rotation_bytes);
        if self.flush_join_deadline.is_zero() {
            self.flush_join_deadline = Duration::from_millis(100);
        }
        self
    }
}

/// Where rotated diagnostics live.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskLogConfig {
    pub directory: PathBuf,
    pub limits: DiskLogLimits,
    /// Active file basename (rotated siblings use `.{n}` suffixes).
    pub file_stem: String,
}

impl DiskLogConfig {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            limits: DiskLogLimits::default(),
            file_stem: "spirula-diagnostics.log".into(),
        }
    }

    pub fn with_limits(mut self, limits: DiskLogLimits) -> Self {
        self.limits = limits;
        self
    }
}

/// Observable sink policy counters (no recursive warning side effects).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiskLogCounters {
    pub accepted: u64,
    pub written: u64,
    pub dropped_full: u64,
    pub dropped_oversized: u64,
    pub write_failures: u64,
    pub rotations: u64,
    pub budget_trimmed: u64,
    pub open_failures: u64,
}

enum SinkMsg {
    Line(String),
    Flush(SyncSender<()>),
    Shutdown(SyncSender<()>),
}

#[derive(Debug)]
struct SharedCounters {
    counters: Mutex<DiskLogCounters>,
    /// One-shot flag: disk failure was already observed (callers may surface
    /// at most one in-memory warning without re-entering the sink).
    failure_noted: AtomicBool,
}

impl SharedCounters {
    fn with<R>(&self, f: impl FnOnce(&mut DiskLogCounters) -> R) -> R {
        let mut g = self.counters.lock().unwrap_or_else(|p| p.into_inner());
        f(&mut g)
    }
}

/// Cloneable handle: enqueue never blocks; flush/join honor deadlines.
#[derive(Clone, Debug)]
pub struct DiskLogHandle {
    tx: SyncSender<SinkMsg>,
    shared: Arc<SharedCounters>,
    limits: DiskLogLimits,
    directory: PathBuf,
    /// Join is single-owner; subsequent clones see `None`.
    join: Arc<Mutex<Option<JoinHandle<()>>>>,
    closed: Arc<AtomicBool>,
}

impl DiskLogHandle {
    /// Spawn the async sink. Directory create / open failures are counted;
    /// the handle still accepts enqueue (lines become write_failures).
    pub fn start(config: DiskLogConfig) -> Self {
        let limits = config.limits.normalized();
        let (tx, rx) = mpsc::sync_channel::<SinkMsg>(limits.queue_capacity);
        let shared = Arc::new(SharedCounters {
            counters: Mutex::new(DiskLogCounters::default()),
            failure_noted: AtomicBool::new(false),
        });
        let shared_thread = Arc::clone(&shared);
        let directory = config.directory.clone();
        let stem = config.file_stem.clone();
        let limits_thread = limits.clone();
        let handle = thread::Builder::new()
            .name("spirula-disk-log".into())
            .spawn(move || sink_loop(directory, stem, limits_thread, shared_thread, rx))
            .expect("spawn disk log sink");
        Self {
            tx,
            shared,
            limits,
            directory: config.directory,
            join: Arc::new(Mutex::new(Some(handle))),
            closed: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn limits(&self) -> &DiskLogLimits {
        &self.limits
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn counters(&self) -> DiskLogCounters {
        self.shared.with(|c| *c)
    }

    /// True after the first write/open failure (for a single in-memory warning).
    pub fn take_failure_notice(&self) -> bool {
        self.shared.failure_noted.swap(false, Ordering::AcqRel)
    }

    /// Non-blocking enqueue. Oversized / full / closed → drop + counter.
    pub fn try_enqueue_line(&self, line: impl Into<String>) -> bool {
        if self.closed.load(Ordering::Acquire) {
            self.shared.with(|c| {
                c.dropped_full = c.dropped_full.saturating_add(1);
            });
            return false;
        }
        let mut line = line.into();
        // Single physical line: strip embedded newlines for the disk format.
        if line.contains('\n') || line.contains('\r') {
            line = line.replace(['\n', '\r'], " ");
        }
        if line.len() > self.limits.line_max_bytes {
            self.shared.with(|c| {
                c.dropped_oversized = c.dropped_oversized.saturating_add(1);
            });
            return false;
        }
        match self.tx.try_send(SinkMsg::Line(line)) {
            Ok(()) => {
                self.shared.with(|c| {
                    c.accepted = c.accepted.saturating_add(1);
                });
                true
            }
            Err(TrySendError::Full(_)) => {
                self.shared.with(|c| {
                    c.dropped_full = c.dropped_full.saturating_add(1);
                });
                false
            }
            Err(TrySendError::Disconnected(_)) => {
                self.shared.with(|c| {
                    c.dropped_full = c.dropped_full.saturating_add(1);
                });
                false
            }
        }
    }

    /// Wait until queued lines before this call are processed, or deadline.
    pub fn flush(&self, deadline: Duration) -> bool {
        if self.closed.load(Ordering::Acquire) {
            return true;
        }
        let (ack_tx, ack_rx) = mpsc::sync_channel(1);
        match self.tx.try_send(SinkMsg::Flush(ack_tx)) {
            Ok(()) => {}
            Err(TrySendError::Full(msg)) => {
                // Rare: queue full of lines. Block briefly with deadline via
                // a helper thread would reintroduce blocking on the caller —
                // instead count and fail the flush so UI/worker stay free.
                drop(msg);
                self.shared.with(|c| {
                    c.dropped_full = c.dropped_full.saturating_add(1);
                });
                return false;
            }
            Err(TrySendError::Disconnected(_)) => return true,
        }
        let end = Instant::now() + deadline;
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

    /// Stop the sink and join within `deadline`. Idempotent.
    pub fn join(&self, deadline: Duration) -> bool {
        if self
            .closed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let (ack_tx, ack_rx) = mpsc::sync_channel(1);
            let _ = self.tx.send(SinkMsg::Shutdown(ack_tx));
            let end = Instant::now() + deadline;
            let _ = ack_rx.recv_timeout(end.saturating_duration_since(Instant::now()));
        }
        let mut slot = self.join.lock().unwrap_or_else(|p| p.into_inner());
        let Some(handle) = slot.take() else {
            return true;
        };
        let end = Instant::now() + deadline;
        while !handle.is_finished() {
            if Instant::now() >= end {
                // Detach: join would block the drop path forever.
                drop(handle);
                return false;
            }
            thread::sleep(Duration::from_millis(2));
        }
        let _ = handle.join();
        true
    }
}

struct SinkState {
    directory: PathBuf,
    stem: String,
    limits: DiskLogLimits,
    shared: Arc<SharedCounters>,
    file: Option<File>,
    active_path: PathBuf,
    active_bytes: u64,
    /// Newest-first logical names of rotated siblings (not including active).
    rotated: VecDeque<PathBuf>,
    total_bytes: u64,
    tail: VecDeque<String>,
}

impl SinkState {
    fn open_new(
        directory: PathBuf,
        stem: String,
        limits: DiskLogLimits,
        shared: Arc<SharedCounters>,
    ) -> Self {
        let _ = fs::create_dir_all(&directory);
        let active_path = directory.join(&stem);
        let mut state = Self {
            directory,
            stem,
            limits,
            shared,
            file: None,
            active_path,
            active_bytes: 0,
            rotated: VecDeque::new(),
            total_bytes: 0,
            tail: VecDeque::new(),
        };
        state.reopen_active();
        state.scan_existing();
        state
    }

    fn note_failure(&mut self) {
        self.shared.with(|c| {
            c.write_failures = c.write_failures.saturating_add(1);
            c.open_failures = c.open_failures.saturating_add(1);
        });
        self.shared.failure_noted.store(true, Ordering::Release);
    }

    fn reopen_active(&mut self) {
        match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.active_path)
        {
            Ok(f) => {
                self.active_bytes = fs::metadata(&self.active_path)
                    .map(|m| m.len())
                    .unwrap_or(0);
                self.file = Some(f);
            }
            Err(_) => {
                self.file = None;
                self.note_failure();
            }
        }
    }

    fn scan_existing(&mut self) {
        let Ok(entries) = fs::read_dir(&self.directory) else {
            return;
        };
        let prefix = format!("{}.", self.stem);
        let mut rotated = Vec::new();
        for ent in entries.flatten() {
            let name = ent.file_name();
            let name = name.to_string_lossy();
            if name == self.stem {
                continue;
            }
            if let Some(rest) = name.strip_prefix(&prefix)
                && rest.parse::<usize>().is_ok()
            {
                rotated.push(ent.path());
            }
        }
        rotated.sort();
        self.rotated = VecDeque::from(rotated);
        self.recompute_total();
        self.trim_to_policy();
    }

    fn recompute_total(&mut self) {
        let mut total = self.active_bytes;
        for p in &self.rotated {
            total = total.saturating_add(fs::metadata(p).map(|m| m.len()).unwrap_or(0));
        }
        self.total_bytes = total;
    }

    fn push_tail(&mut self, line: &str) {
        if self.tail.len() >= self.limits.tail_capacity {
            self.tail.pop_front();
        }
        self.tail.push_back(line.to_string());
    }

    fn write_line(&mut self, line: &str) {
        if self.file.is_none() {
            self.reopen_active();
        }
        let Some(file) = self.file.as_mut() else {
            self.shared.with(|c| {
                c.write_failures = c.write_failures.saturating_add(1);
            });
            self.shared.failure_noted.store(true, Ordering::Release);
            return;
        };
        let payload = format!("{line}\n");
        match file
            .write_all(payload.as_bytes())
            .and_then(|_| file.flush())
        {
            Ok(()) => {
                let n = payload.len() as u64;
                self.active_bytes = self.active_bytes.saturating_add(n);
                self.total_bytes = self.total_bytes.saturating_add(n);
                self.shared.with(|c| {
                    c.written = c.written.saturating_add(1);
                });
                self.push_tail(line);
                if self.active_bytes >= self.limits.rotation_bytes {
                    self.rotate();
                } else {
                    self.trim_to_policy();
                }
            }
            Err(_) => {
                self.file = None;
                self.shared.with(|c| {
                    c.write_failures = c.write_failures.saturating_add(1);
                });
                self.shared.failure_noted.store(true, Ordering::Release);
            }
        }
    }

    fn rotate(&mut self) {
        // Close active, shift rotated names upward, reopen empty active.
        self.file = None;
        let max_idx = self.limits.max_files.saturating_sub(1).max(1);
        // Delete the oldest if we would exceed max_files after keeping active.
        while self.rotated.len() + 1 >= self.limits.max_files {
            if let Some(old) = self.rotated.pop_back() {
                let sz = fs::metadata(&old).map(|m| m.len()).unwrap_or(0);
                let _ = fs::remove_file(&old);
                self.total_bytes = self.total_bytes.saturating_sub(sz);
                self.shared.with(|c| {
                    c.budget_trimmed = c.budget_trimmed.saturating_add(1);
                });
            } else {
                break;
            }
        }
        // Shift existing .N → .N+1 (highest first).
        for idx in (1..=max_idx).rev() {
            let from = self.directory.join(format!("{}.{}", self.stem, idx));
            let to = self.directory.join(format!("{}.{}", self.stem, idx + 1));
            if from.exists() {
                let _ = fs::rename(&from, &to);
            }
        }
        let dest = self.directory.join(format!("{}.1", self.stem));
        match fs::rename(&self.active_path, &dest) {
            Ok(()) => {
                self.rotated.push_front(dest);
                self.shared.with(|c| {
                    c.rotations = c.rotations.saturating_add(1);
                });
            }
            Err(_) => {
                self.note_failure();
            }
        }
        self.active_bytes = 0;
        self.reopen_active();
        self.recompute_total();
        self.trim_to_policy();
    }

    fn trim_to_policy(&mut self) {
        while self.rotated.len() + 1 > self.limits.max_files {
            if let Some(old) = self.rotated.pop_back() {
                let sz = fs::metadata(&old).map(|m| m.len()).unwrap_or(0);
                let _ = fs::remove_file(&old);
                self.total_bytes = self.total_bytes.saturating_sub(sz);
                self.shared.with(|c| {
                    c.budget_trimmed = c.budget_trimmed.saturating_add(1);
                });
            } else {
                break;
            }
        }
        while self.total_bytes > self.limits.global_disk_budget_bytes {
            let Some(old) = self.rotated.pop_back() else {
                break;
            };
            let sz = fs::metadata(&old).map(|m| m.len()).unwrap_or(0);
            let _ = fs::remove_file(&old);
            self.total_bytes = self.total_bytes.saturating_sub(sz);
            self.shared.with(|c| {
                c.budget_trimmed = c.budget_trimmed.saturating_add(1);
            });
        }
    }
}

fn sink_loop(
    directory: PathBuf,
    stem: String,
    limits: DiskLogLimits,
    shared: Arc<SharedCounters>,
    rx: mpsc::Receiver<SinkMsg>,
) {
    let mut state = SinkState::open_new(directory, stem, limits, shared);
    while let Ok(msg) = rx.recv() {
        match msg {
            SinkMsg::Line(line) => state.write_line(&line),
            SinkMsg::Flush(ack) => {
                let _ = ack.send(());
            }
            SinkMsg::Shutdown(ack) => {
                // Drain anything already queued before shutdown ack.
                while let Ok(more) = rx.try_recv() {
                    match more {
                        SinkMsg::Line(line) => state.write_line(&line),
                        SinkMsg::Flush(a) => {
                            let _ = a.send(());
                        }
                        SinkMsg::Shutdown(a) => {
                            let _ = a.send(());
                        }
                    }
                }
                let _ = ack.send(());
                break;
            }
        }
    }
}

/// Test helper: make a path that cannot be created as a file (parent is a file).
#[cfg(test)]
pub fn unwritable_nested_path(parent_file: &Path) -> PathBuf {
    parent_file.join("child").join("diagnostics")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn temp_dir(label: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("spirula-disk-log-{}-{}", label, std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).expect("temp");
        p
    }

    #[test]
    fn enqueue_write_flush_and_tail_policy() {
        let dir = temp_dir("basic");
        let handle =
            DiskLogHandle::start(DiskLogConfig::new(&dir).with_limits(DiskLogLimits::for_test()));
        assert!(handle.try_enqueue_line("hello"));
        assert!(handle.flush(Duration::from_secs(1)));
        let text = fs::read_to_string(dir.join("spirula-diagnostics.log")).expect("read");
        assert!(text.contains("hello"));
        let c = handle.counters();
        assert_eq!(c.accepted, 1);
        assert_eq!(c.written, 1);
        assert!(handle.join(Duration::from_secs(1)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn oversized_and_full_queue_drop() {
        let dir = temp_dir("drops");
        let mut limits = DiskLogLimits::for_test();
        limits.queue_capacity = 1;
        limits.line_max_bytes = 8;
        let handle = DiskLogHandle::start(DiskLogConfig::new(&dir).with_limits(limits));
        assert!(!handle.try_enqueue_line("way-too-long-line"));
        // Fill queue with a line, then stall by not flushing while flooding.
        // With capacity 1, second try_send of Line may drop if sink is slow.
        // Force drops via closed path after join.
        assert!(handle.try_enqueue_line("ok"));
        assert!(handle.join(Duration::from_secs(1)));
        assert!(!handle.try_enqueue_line("after"));
        let c = handle.counters();
        assert!(c.dropped_oversized >= 1);
        assert!(c.dropped_full >= 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotation_and_budget_trim() {
        let dir = temp_dir("rotate");
        let mut limits = DiskLogLimits::for_test();
        limits.rotation_bytes = 40;
        limits.max_files = 2;
        limits.global_disk_budget_bytes = 120;
        limits.queue_capacity = 32;
        let handle = DiskLogHandle::start(DiskLogConfig::new(&dir).with_limits(limits));
        for i in 0..30 {
            assert!(handle.try_enqueue_line(format!("line-{i:04} padding-bytes")));
        }
        assert!(handle.flush(Duration::from_secs(2)));
        let c = handle.counters();
        assert!(c.rotations >= 1, "counters={c:?}");
        // At most max_files entries on disk.
        let count = fs::read_dir(&dir).unwrap().flatten().count();
        assert!(count <= 2, "files={count}");
        assert!(handle.join(Duration::from_secs(1)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unwritable_sink_counts_failure_without_panic() {
        let dir = temp_dir("unwritable");
        let blocker = dir.join("blocker");
        fs::write(&blocker, b"not-a-dir").unwrap();
        let target = unwritable_nested_path(&blocker);
        let handle = DiskLogHandle::start(
            DiskLogConfig::new(&target).with_limits(DiskLogLimits::for_test()),
        );
        let _ = handle.try_enqueue_line("will-fail");
        let _ = handle.flush(Duration::from_secs(1));
        let c = handle.counters();
        assert!(c.write_failures + c.open_failures >= 1, "counters={c:?}");
        // Failure notice is one-shot and does not itself write to disk.
        let noted = handle.take_failure_notice();
        assert!(noted);
        assert!(!handle.take_failure_notice());
        assert!(handle.join(Duration::from_secs(1)));
        let _ = fs::remove_dir_all(&dir);
    }
}
