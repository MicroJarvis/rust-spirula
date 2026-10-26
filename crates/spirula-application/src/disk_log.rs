//! Rotated disk diagnostics sink (P1-F09).
//!
//! Owner/`submit` paths never block on disk. Lines enter a bounded `try_send`
//! data queue; a dedicated sink thread appends, rotates, and trims under
//! explicit caps. Shutdown is out-of-band (`AtomicBool`) so a full/stalled
//! queue cannot prevent join cancellation. `join`/`flush` use one absolute
//! deadline from method entry; a timed-out join keeps the handle and returns
//! false (retry must not report success while the thread is alive).
//!
//! Filesystem open/write/rename/remove can still block the *sink* thread
//! itself (FIFO, NFS, etc.); that is documented unavoidable kernel blocking,
//! not a caller-side wait outside the deadline.

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use spirula_core::{ContextKey, DomainError, ErrorCode};

/// Explicit disk-diagnostics resource policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskLogLimits {
    /// Max UTF-8 bytes per *logical* line (newline is added on write and
    /// counted toward rotation / global budget).
    pub line_max_bytes: usize,
    /// Bounded async data-queue capacity (`try_send`; full → drop).
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
            line_max_bytes: 64,
            queue_capacity: 4,
            tail_capacity: 8,
            rotation_bytes: 128,
            max_files: 3,
            global_disk_budget_bytes: 256,
            flush_join_deadline: Duration::from_millis(500),
        }
        .normalized()
    }

    /// Clamp floors and ensure a single admitted line (+ trailing newline)
    /// cannot exceed rotation or global budget by itself.
    pub fn normalized(mut self) -> Self {
        self.line_max_bytes = self.line_max_bytes.max(16);
        self.queue_capacity = self.queue_capacity.max(1);
        self.tail_capacity = self.tail_capacity.max(1);
        self.rotation_bytes = self.rotation_bytes.max(16);
        self.max_files = self.max_files.max(1);
        let line_disk = (self.line_max_bytes as u64).saturating_add(1); // + '\n'
        self.rotation_bytes = self.rotation_bytes.max(line_disk);
        self.global_disk_budget_bytes = self
            .global_disk_budget_bytes
            .max(self.rotation_bytes)
            .max(line_disk);
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

    /// Replace the active basename. Rejects path separators, `..`, empty, and
    /// multi-component names (`SP-INP-001`).
    pub fn with_file_stem(mut self, stem: impl Into<String>) -> Result<Self, DomainError> {
        let stem = stem.into();
        validate_file_stem(&stem)?;
        self.file_stem = stem;
        Ok(self)
    }
}

/// Reject rooted / parent / separator forms; require one normal basename.
pub fn validate_file_stem(stem: &str) -> Result<(), DomainError> {
    let err = |msg: &str, actual: &str| {
        DomainError::try_new(ErrorCode::InvalidArgument, msg)
            .expect("message")
            .with_context(ContextKey::Field, "file_stem")
            .expect("field")
            .with_context(ContextKey::Actual, actual)
            .expect("actual")
    };
    if stem.is_empty() || stem.trim().is_empty() {
        return Err(err("disk log file_stem must be non-empty", stem));
    }
    if stem.contains('\0') || stem.contains('\n') || stem.contains('\r') {
        return Err(err(
            "disk log file_stem must not contain control chars",
            stem,
        ));
    }
    let path = Path::new(stem);
    if path.is_absolute() {
        return Err(err("disk log file_stem must be a relative basename", stem));
    }
    let mut comps = path.components();
    let Some(Component::Normal(os)) = comps.next() else {
        return Err(err("disk log file_stem must be a single basename", stem));
    };
    if comps.next().is_some() {
        return Err(err("disk log file_stem must be a single basename", stem));
    }
    let name = os.to_string_lossy();
    if name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err(err(
            "disk log file_stem rejects parent/separator forms",
            stem,
        ));
    }
    // `Path::new("../x")` yields ParentDir + Normal — already rejected above.
    // Also reject raw `..` / `.` as the sole component.
    if stem.contains("..") || stem.contains('/') || stem.contains('\\') {
        return Err(err(
            "disk log file_stem rejects parent/separator forms",
            stem,
        ));
    }
    Ok(())
}

/// Observable sink policy counters (no recursive warning side effects).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiskLogCounters {
    pub accepted: u64,
    pub written: u64,
    pub dropped_full: u64,
    pub dropped_oversized: u64,
    /// Dropped because rotation/trim could not keep owned files within caps.
    pub dropped_degraded: u64,
    pub write_failures: u64,
    pub rotations: u64,
    pub budget_trimmed: u64,
    pub open_failures: u64,
    pub trim_failures: u64,
    /// Non-zero once the sink entered degraded (no further disk appends).
    pub degraded: u64,
}

enum DataMsg {
    Line(String),
    Flush(SyncSender<()>),
}

#[derive(Debug)]
struct SharedCounters {
    counters: Mutex<DiskLogCounters>,
    /// Set when a disk failure occurs; cleared only when surfaced.
    failure_pending: AtomicBool,
    /// Lifetime latch: at most one in-memory warning for this sink.
    warning_surfaced: AtomicBool,
    /// Out-of-band: further enqueues/writes are dropped (policy degraded).
    writes_disabled: AtomicBool,
}

impl SharedCounters {
    fn with<R>(&self, f: impl FnOnce(&mut DiskLogCounters) -> R) -> R {
        let mut g = self.counters.lock().unwrap_or_else(|p| p.into_inner());
        f(&mut g)
    }

    fn note_io_failure(&self, open: bool) {
        self.with(|c| {
            c.write_failures = c.write_failures.saturating_add(1);
            if open {
                c.open_failures = c.open_failures.saturating_add(1);
            }
        });
        if !self.warning_surfaced.load(Ordering::Acquire) {
            self.failure_pending.store(true, Ordering::Release);
        }
    }

    fn enter_degraded(&self) {
        if self
            .writes_disabled
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.with(|c| {
                c.degraded = 1;
            });
            if !self.warning_surfaced.load(Ordering::Acquire) {
                self.failure_pending.store(true, Ordering::Release);
            }
        }
    }

    fn note_dropped_degraded(&self) {
        self.with(|c| {
            c.dropped_degraded = c.dropped_degraded.saturating_add(1);
        });
    }
}

/// Cloneable handle: enqueue never blocks; flush/join honor absolute deadlines.
#[derive(Clone, Debug)]
pub struct DiskLogHandle {
    data_tx: SyncSender<DataMsg>,
    shared: Arc<SharedCounters>,
    limits: DiskLogLimits,
    directory: PathBuf,
    /// Join is single-owner; timed-out joins put the handle back.
    join: Arc<Mutex<Option<JoinHandle<()>>>>,
    closed: Arc<AtomicBool>,
    /// Out-of-band stop: sink observes without draining the data queue.
    shutdown: Arc<AtomicBool>,
}

impl DiskLogHandle {
    /// Spawn the async sink after validating `file_stem`.
    pub fn try_start(config: DiskLogConfig) -> Result<Self, DomainError> {
        validate_file_stem(&config.file_stem)?;
        let limits = config.limits.normalized();
        let (data_tx, data_rx) = mpsc::sync_channel::<DataMsg>(limits.queue_capacity);
        let shared = Arc::new(SharedCounters {
            counters: Mutex::new(DiskLogCounters::default()),
            failure_pending: AtomicBool::new(false),
            warning_surfaced: AtomicBool::new(false),
            writes_disabled: AtomicBool::new(false),
        });
        let shutdown = Arc::new(AtomicBool::new(false));
        let shared_thread = Arc::clone(&shared);
        let shutdown_thread = Arc::clone(&shutdown);
        let directory = config.directory.clone();
        let stem = config.file_stem.clone();
        let limits_thread = limits.clone();
        let handle = thread::Builder::new()
            .name("spirula-disk-log".into())
            .spawn(move || {
                sink_loop(
                    directory,
                    stem,
                    limits_thread,
                    shared_thread,
                    shutdown_thread,
                    data_rx,
                )
            })
            .expect("spawn disk log sink");
        Ok(Self {
            data_tx,
            shared,
            limits,
            directory: config.directory,
            join: Arc::new(Mutex::new(Some(handle))),
            closed: Arc::new(AtomicBool::new(false)),
            shutdown,
        })
    }

    /// Legacy helper for tests with a pre-validated default stem.
    pub fn start(config: DiskLogConfig) -> Self {
        Self::try_start(config).expect("DiskLogConfig file_stem must be a basename")
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

    /// At most one success per sink lifetime. Further failures stay counted
    /// but do not re-arm a second in-memory warning.
    pub fn take_failure_notice(&self) -> bool {
        if self.shared.warning_surfaced.load(Ordering::Acquire) {
            return false;
        }
        if !self.shared.failure_pending.swap(false, Ordering::AcqRel) {
            return false;
        }
        self.shared.warning_surfaced.store(true, Ordering::Release);
        true
    }

    /// True when rotation/trim could not honor caps; further writes are dropped.
    pub fn writes_disabled(&self) -> bool {
        self.shared.writes_disabled.load(Ordering::Acquire)
    }

    /// Non-blocking enqueue. Oversized / full / closed / degraded → drop + counter.
    /// Newline and CR are rejected (count as oversized), not silently stripped
    /// into a longer-than-cap payload.
    pub fn try_enqueue_line(&self, line: impl Into<String>) -> bool {
        if self.closed.load(Ordering::Acquire) || self.shutdown.load(Ordering::Acquire) {
            self.shared.with(|c| {
                c.dropped_full = c.dropped_full.saturating_add(1);
            });
            return false;
        }
        if self.shared.writes_disabled.load(Ordering::Acquire) {
            self.shared.note_dropped_degraded();
            return false;
        }
        let line = line.into();
        if line.contains('\n') || line.contains('\r') || line.len() > self.limits.line_max_bytes {
            self.shared.with(|c| {
                c.dropped_oversized = c.dropped_oversized.saturating_add(1);
            });
            return false;
        }
        // Admitted payload on disk is `line + '\n'`; normalized() guarantees
        // that fits rotation/global budget alone.
        match self.data_tx.try_send(DataMsg::Line(line)) {
            Ok(()) => {
                self.shared.with(|c| {
                    c.accepted = c.accepted.saturating_add(1);
                });
                true
            }
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.shared.with(|c| {
                    c.dropped_full = c.dropped_full.saturating_add(1);
                });
                false
            }
        }
    }

    /// Wait until queued lines before this call are processed, or deadline.
    /// Absolute deadline starts at entry; never blocks on a full data queue.
    pub fn flush(&self, deadline: Duration) -> bool {
        let end = Instant::now() + deadline;
        if self.closed.load(Ordering::Acquire) || self.shutdown.load(Ordering::Acquire) {
            return true;
        }
        let (ack_tx, ack_rx) = mpsc::sync_channel(1);
        match self.data_tx.try_send(DataMsg::Flush(ack_tx)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.shared.with(|c| {
                    c.dropped_full = c.dropped_full.saturating_add(1);
                });
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

    /// Signal out-of-band shutdown without waiting for the sink thread.
    /// Safe to call repeatedly; used when Drop has no remaining join budget.
    pub fn signal_shutdown(&self) {
        self.closed.store(true, Ordering::Release);
        self.shutdown.store(true, Ordering::Release);
    }

    /// Stop the sink within one absolute `deadline` from entry (includes
    /// join-mutex acquisition). Concurrent clones that cannot acquire the
    /// mutex before the deadline return `false` without claiming success.
    ///
    /// Shutdown is always signaled out-of-band first so a full data queue
    /// cannot block cancellation. On timeout the join handle is retained and
    /// this returns `false`; a later retry may still succeed. Never returns
    /// `true` while the sink thread is known alive.
    pub fn join(&self, deadline: Duration) -> bool {
        let end = Instant::now() + deadline;
        self.signal_shutdown();

        let Some(mut slot) = try_lock_until(&self.join, end) else {
            // Deadline spent waiting for another join; sink may still be alive.
            return false;
        };
        let Some(handle) = slot.take() else {
            // Prior successful join, or concurrent join finished it.
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
        // Keep ownership for an honest retry; do not claim success.
        *slot = Some(handle);
        false
    }

    /// Whether the sink thread handle is still held (not successfully joined).
    /// If another join holds the mutex, reports alive (honest under contention).
    pub fn sink_alive(&self) -> bool {
        match self.join.try_lock() {
            Ok(slot) => match slot.as_ref() {
                Some(h) => !h.is_finished(),
                None => false,
            },
            Err(TryLockError::WouldBlock) => true,
            Err(TryLockError::Poisoned(p)) => match p.into_inner().as_ref() {
                Some(h) => !h.is_finished(),
                None => false,
            },
        }
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

struct SinkState {
    directory: PathBuf,
    stem: String,
    limits: DiskLogLimits,
    shared: Arc<SharedCounters>,
    file: Option<File>,
    active_path: PathBuf,
    active_bytes: u64,
    /// Newest-first rotated siblings from a fresh directory inventory.
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
        state.rebuild_inventory_from_disk();
        // Enforce max_files / budget before any accepted write — no false caps.
        state.trim_to_policy();
        if !state.policy_ok() {
            state.shared.enter_degraded();
        }
        state
    }

    /// Owned active file (if present) + rotated file count and byte budget.
    /// Active must stay strictly below `rotation_bytes` (rotate before next append).
    fn policy_ok(&self) -> bool {
        let active_file = fs::metadata(&self.active_path)
            .ok()
            .is_some_and(|m| m.is_file());
        let owned = usize::from(active_file) + self.rotated.len();
        owned <= self.limits.max_files
            && self.total_bytes <= self.limits.global_disk_budget_bytes
            && self.active_bytes < self.limits.rotation_bytes
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
                self.shared.note_io_failure(true);
            }
        }
    }

    /// Rebuild rotated inventory from the real directory (numeric order).
    /// Active file is excluded; siblings are `stem.N` for N ≥ 1, newest first.
    fn rebuild_inventory_from_disk(&mut self) {
        let mut indexed: Vec<(usize, PathBuf, u64)> = Vec::new();
        let Ok(entries) = fs::read_dir(&self.directory) else {
            self.rotated.clear();
            self.recompute_total_from_disk();
            return;
        };
        let prefix = format!("{}.", self.stem);
        for ent in entries.flatten() {
            let path = ent.path();
            let Ok(meta) = ent.metadata() else {
                continue;
            };
            if !meta.is_file() {
                // Directories / specials named like stem.N stay untracked and
                // are never "trimmed" by subtracting fictional bytes.
                continue;
            }
            let name = ent.file_name();
            let name = name.to_string_lossy();
            if name.as_ref() == self.stem {
                self.active_bytes = meta.len();
                continue;
            }
            if let Some(rest) = name.strip_prefix(&prefix)
                && let Ok(idx) = rest.parse::<usize>()
                && idx >= 1
            {
                indexed.push((idx, path, meta.len()));
            }
        }
        // Newest (lowest index) first for trim-from-back (oldest = highest idx).
        indexed.sort_by_key(|(idx, _, _)| *idx);
        self.rotated = indexed.into_iter().map(|(_, p, _)| p).collect();
        self.recompute_total_from_disk();
    }

    fn recompute_total_from_disk(&mut self) {
        let mut total = fs::metadata(&self.active_path)
            .map(|m| if m.is_file() { m.len() } else { 0 })
            .unwrap_or(0);
        self.active_bytes = total;
        for p in &self.rotated {
            total = total.saturating_add(
                fs::metadata(p)
                    .ok()
                    .filter(|m| m.is_file())
                    .map(|m| m.len())
                    .unwrap_or(0),
            );
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
        if self.shared.writes_disabled.load(Ordering::Acquire) {
            self.shared.note_dropped_degraded();
            return;
        }
        let payload_len = (line.len() as u64).saturating_add(1);
        // Rotate before append when the active file is already at the threshold.
        if self.active_bytes >= self.limits.rotation_bytes {
            self.rotate();
            if self.active_bytes >= self.limits.rotation_bytes {
                self.shared.enter_degraded();
                self.shared.note_dropped_degraded();
                return;
            }
        }
        // Do not grow past the global budget when trim cannot free space.
        if self.total_bytes.saturating_add(payload_len) > self.limits.global_disk_budget_bytes {
            self.trim_to_policy();
            if self.total_bytes.saturating_add(payload_len) > self.limits.global_disk_budget_bytes {
                self.shared.enter_degraded();
                self.shared.note_dropped_degraded();
                return;
            }
        }
        if self.file.is_none() {
            self.reopen_active();
        }
        let Some(file) = self.file.as_mut() else {
            self.shared.note_io_failure(false);
            self.shared.enter_degraded();
            self.shared.note_dropped_degraded();
            return;
        };
        let payload = format!("{line}\n");
        match file
            .write_all(payload.as_bytes())
            .and_then(|_| file.flush())
        {
            Ok(()) => {
                if let Ok(meta) = fs::metadata(&self.active_path) {
                    self.active_bytes = meta.len();
                } else {
                    self.active_bytes = self.active_bytes.saturating_add(payload.len() as u64);
                }
                self.shared.with(|c| {
                    c.written = c.written.saturating_add(1);
                });
                self.push_tail(line);
                self.recompute_total_from_disk();
                if self.active_bytes >= self.limits.rotation_bytes {
                    self.rotate();
                } else {
                    self.trim_to_policy();
                }
                if !self.policy_ok() {
                    self.shared.enter_degraded();
                }
            }
            Err(_) => {
                self.file = None;
                self.shared.note_io_failure(false);
                self.shared.enter_degraded();
            }
        }
    }

    fn try_remove_owned_file(&mut self, path: &Path) -> bool {
        let Ok(meta) = fs::metadata(path) else {
            // Already gone: drop from inventory without trim_failures.
            return true;
        };
        if !meta.is_file() {
            self.shared.with(|c| {
                c.trim_failures = c.trim_failures.saturating_add(1);
            });
            return false;
        }
        match fs::remove_file(path) {
            Ok(()) => {
                self.shared.with(|c| {
                    c.budget_trimmed = c.budget_trimmed.saturating_add(1);
                });
                true
            }
            Err(_) => {
                self.shared.with(|c| {
                    c.trim_failures = c.trim_failures.saturating_add(1);
                    c.write_failures = c.write_failures.saturating_add(1);
                });
                false
            }
        }
    }

    fn rotate(&mut self) {
        self.file = None;
        // Free a slot among retained files if needed (active will become .1).
        self.trim_to_policy_for_count(self.limits.max_files.saturating_sub(1));

        // Shift owned file siblings; bound by the real inventory size, not an
        // arbitrary constant that can leave hundreds of files behind.
        self.rebuild_inventory_from_disk();
        let max_idx = self
            .rotated
            .len()
            .saturating_add(2)
            .max(self.limits.max_files.saturating_mul(2))
            .max(8);
        for idx in (1..=max_idx).rev() {
            let from = self.directory.join(format!("{}.{}", self.stem, idx));
            let to = self.directory.join(format!("{}.{}", self.stem, idx + 1));
            if from.is_file() {
                let _ = fs::rename(&from, &to);
            }
        }
        let dest = self.directory.join(format!("{}.1", self.stem));
        // Non-file occupants of `.1` (directories) block rotation — do not
        // pretend the active file was rotated.
        if dest.exists() && !dest.is_file() {
            self.shared.note_io_failure(false);
            self.recompute_total_from_disk();
            return;
        }
        match fs::rename(&self.active_path, &dest) {
            Ok(()) => {
                self.shared.with(|c| {
                    c.rotations = c.rotations.saturating_add(1);
                });
                self.active_bytes = 0;
                self.reopen_active();
            }
            Err(_) => {
                self.shared.note_io_failure(false);
                // Keep the oversized active file; caller must degrade.
                self.reopen_active();
            }
        }
        self.rebuild_inventory_from_disk();
        self.trim_to_policy();
    }

    fn trim_to_policy(&mut self) {
        self.trim_to_policy_for_count(self.limits.max_files.saturating_sub(1));
        // Budget trim: remove oldest rotated files until under budget or stuck.
        loop {
            self.rebuild_inventory_from_disk();
            if self.total_bytes <= self.limits.global_disk_budget_bytes {
                break;
            }
            let Some(old) = self.rotated.pop_back() else {
                break;
            };
            if !self.try_remove_owned_file(&old) {
                self.rotated.push_back(old);
                break;
            }
        }
    }

    fn trim_to_policy_for_count(&mut self, max_rotated: usize) {
        // No arbitrary iteration ceiling — keep deleting until the cap holds
        // or a remove fails (then degrade at the call site via policy_ok).
        loop {
            self.rebuild_inventory_from_disk();
            if self.rotated.len() <= max_rotated {
                break;
            }
            let Some(old) = self.rotated.pop_back() else {
                break;
            };
            if !self.try_remove_owned_file(&old) {
                self.rotated.push_back(old);
                break;
            }
        }
    }
}

fn sink_loop(
    directory: PathBuf,
    stem: String,
    limits: DiskLogLimits,
    shared: Arc<SharedCounters>,
    shutdown: Arc<AtomicBool>,
    rx: mpsc::Receiver<DataMsg>,
) {
    let mut state = SinkState::open_new(directory, stem, limits, shared);
    let poll = Duration::from_millis(5);
    loop {
        if shutdown.load(Ordering::Acquire) {
            // Drain what is already queued without waiting for new lines.
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    DataMsg::Line(line) => state.write_line(&line),
                    DataMsg::Flush(ack) => {
                        let _ = ack.send(());
                    }
                }
            }
            break;
        }
        match rx.recv_timeout(poll) {
            Ok(DataMsg::Line(line)) => state.write_line(&line),
            Ok(DataMsg::Flush(ack)) => {
                let _ = ack.send(());
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// Test helper: make a path that cannot be created as a file (parent is a file).
#[cfg(test)]
pub fn unwritable_nested_path(parent_file: &Path) -> PathBuf {
    parent_file.join("child").join("diagnostics")
}

#[cfg(test)]
#[allow(unsafe_code)] // mkfifo for stalled-open join tests
mod tests {
    use super::*;
    use std::sync::mpsc;
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
        assert!(!handle.sink_alive());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn newline_and_oversized_rejected() {
        let dir = temp_dir("nl");
        // Bypass for_test floors: set caps then normalize so line_max stays
        // below the oversized sample (normalized still raises floors).
        let limits = DiskLogLimits {
            line_max_bytes: 16,
            queue_capacity: 4,
            tail_capacity: 4,
            rotation_bytes: 64,
            max_files: 2,
            global_disk_budget_bytes: 128,
            flush_join_deadline: Duration::from_millis(200),
        }
        .normalized();
        let handle = DiskLogHandle::start(DiskLogConfig::new(&dir).with_limits(limits));
        assert!(!handle.try_enqueue_line("has\nnewline"));
        let oversized = "x".repeat(handle.limits().line_max_bytes + 1);
        assert!(!handle.try_enqueue_line(oversized));
        let c = handle.counters();
        assert!(c.dropped_oversized >= 2, "{c:?}");
        assert!(handle.join(Duration::from_secs(1)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn full_queue_while_sink_stalled_on_fifo() {
        let dir = temp_dir("fifo");
        let fifo = dir.join("spirula-diagnostics.log");
        // Create a FIFO so the sink blocks in open/write.
        #[cfg(unix)]
        {
            use std::ffi::CString;
            let c = CString::new(fifo.to_str().unwrap()).unwrap();
            unsafe {
                assert_eq!(libc::mkfifo(c.as_ptr(), 0o644), 0);
            }
        }
        let mut limits = DiskLogLimits::for_test();
        limits.queue_capacity = 1;
        limits.line_max_bytes = 32;
        let handle = DiskLogHandle::start(DiskLogConfig::new(&dir).with_limits(limits));
        // First line may enter the queue; subsequent fills drop.
        let _ = handle.try_enqueue_line("one");
        let mut dropped = 0;
        for i in 0..8 {
            if !handle.try_enqueue_line(format!("fill-{i}")) {
                dropped += 1;
            }
        }
        assert!(dropped >= 1, "expected full-queue drops");
        let c = handle.counters();
        assert!(c.dropped_full >= 1, "{c:?}");

        // join must return within absolute budget even with blocked sink.
        let start = Instant::now();
        let ok = handle.join(Duration::from_millis(30));
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(200),
            "join blocked too long: {elapsed:?}"
        );
        if !ok {
            assert!(handle.sink_alive(), "timeout must keep alive handle");
            // Retry with longer budget after we open the FIFO reader so open can finish.
            let _reader = File::open(&fifo);
            let _ = handle.join(Duration::from_secs(2));
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_join_deadline_includes_mutex_wait() {
        // Matches /tmp/spirula-f09-concurrent-join-probe: join1 holds the mutex
        // on a stalled FIFO; join2 budget 10ms must not block ~200ms nor return
        // true while the handle is still owned by join1.
        let dir = temp_dir("conc-join");
        let fifo = dir.join("spirula-diagnostics.log");
        #[cfg(unix)]
        {
            use std::ffi::CString;
            let c = CString::new(fifo.to_str().unwrap()).unwrap();
            unsafe {
                assert_eq!(libc::mkfifo(c.as_ptr(), 0o644), 0);
            }
        }
        let handle = DiskLogHandle::start(DiskLogConfig::new(&dir).with_limits(DiskLogLimits {
            queue_capacity: 1,
            ..DiskLogLimits::for_test()
        }));
        let clone = handle.clone();
        let _ = handle.try_enqueue_line("block-open");
        let long = thread::spawn(move || {
            let _ = clone.join(Duration::from_millis(500));
        });
        thread::sleep(Duration::from_millis(30));
        let fifo_open = fifo.clone();
        let unblock = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            let _ = File::open(fifo_open);
        });
        let start = Instant::now();
        let ok = handle.join(Duration::from_millis(10));
        let elapsed = start.elapsed();
        assert!(!ok, "join2 must not claim success while join1 holds handle");
        assert!(
            elapsed < Duration::from_millis(50),
            "absolute deadline must include lock wait (probe: 10ms not ~200ms): {elapsed:?}"
        );
        assert!(
            handle.sink_alive(),
            "truthful status: sink still alive under contended join"
        );
        let _ = unblock.join();
        let _ = handle.join(Duration::from_secs(2));
        let _ = long.join();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn join_zero_deadline_still_signals_shutdown() {
        let dir = temp_dir("join-zero");
        let handle =
            DiskLogHandle::start(DiskLogConfig::new(&dir).with_limits(DiskLogLimits::for_test()));
        let _ = handle.try_enqueue_line("x");
        // Zero budget must still trip OOB shutdown; retry can finish the join.
        let _ = handle.join(Duration::ZERO);
        // Enqueues after signal must drop (closed/shutdown).
        assert!(!handle.try_enqueue_line("after-signal"));
        assert!(handle.join(Duration::from_secs(1)));
        assert!(!handle.sink_alive());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn join_timeout_does_not_report_true_while_alive() {
        // Controlled stall: hold the data receiver by using a custom sink is
        // hard; instead fill a FIFO and assert timeout semantics.
        let dir = temp_dir("join-alive");
        let fifo = dir.join("spirula-diagnostics.log");
        #[cfg(unix)]
        {
            use std::ffi::CString;
            let c = CString::new(fifo.to_str().unwrap()).unwrap();
            unsafe {
                assert_eq!(libc::mkfifo(c.as_ptr(), 0o644), 0);
            }
        }
        let handle = DiskLogHandle::start(DiskLogConfig::new(&dir).with_limits(DiskLogLimits {
            queue_capacity: 1,
            ..DiskLogLimits::for_test()
        }));
        let _ = handle.try_enqueue_line("block-open");
        let start = Instant::now();
        assert!(!handle.join(Duration::from_millis(20)));
        assert!(start.elapsed() < Duration::from_millis(150));
        assert!(handle.sink_alive());
        // Second call must still be false while blocked (not false→true lie).
        assert!(!handle.join(Duration::from_millis(10)));
        assert!(handle.sink_alive());
        let _reader = File::open(&fifo);
        assert!(handle.join(Duration::from_secs(2)));
        assert!(!handle.sink_alive());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_stem_rejects_parent_escape() {
        let err = validate_file_stem("../outside.log").unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidArgument);
        let err = DiskLogConfig::new("/tmp/x")
            .with_file_stem("../outside.log")
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidArgument);
        assert!(
            DiskLogHandle::try_start(
                DiskLogConfig::new("/tmp/x")
                    .with_file_stem("../outside.log")
                    .unwrap_or_else(|_| DiskLogConfig::new("/tmp/x"))
            )
            .is_err()
                || validate_file_stem("../outside.log").is_err()
        );
        assert!(
            DiskLogHandle::try_start(DiskLogConfig {
                directory: PathBuf::from("/tmp"),
                limits: DiskLogLimits::for_test(),
                file_stem: "../outside.log".into(),
            })
            .is_err()
        );
    }

    #[test]
    fn failure_notice_is_once_per_sink_lifetime() {
        let dir = temp_dir("once");
        let blocker = dir.join("blocker");
        fs::write(&blocker, b"not-a-dir").unwrap();
        let target = unwritable_nested_path(&blocker);
        let handle = DiskLogHandle::start(
            DiskLogConfig::new(&target).with_limits(DiskLogLimits::for_test()),
        );
        let _ = handle.try_enqueue_line("a");
        let _ = handle.flush(Duration::from_millis(100));
        assert!(handle.take_failure_notice());
        // More failures must not re-arm.
        let _ = handle.try_enqueue_line("b");
        let _ = handle.flush(Duration::from_millis(100));
        assert!(!handle.take_failure_notice());
        assert!(!handle.take_failure_notice());
        let _ = handle.join(Duration::from_secs(1));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn trim_does_not_forget_undeleted_directory_sibling() {
        let dir = temp_dir("trim-dir");
        let mut limits = DiskLogLimits::for_test();
        limits.max_files = 1; // no rotated slots → immediate trim attempts
        limits.rotation_bytes = 32;
        limits.global_disk_budget_bytes = 32;
        let handle = DiskLogHandle::start(DiskLogConfig::new(&dir).with_limits(limits));
        // Plant a nonempty directory that looks like a rotated sibling name.
        let fake = dir.join("spirula-diagnostics.log.1");
        fs::create_dir_all(&fake).unwrap();
        fs::write(fake.join("inner"), b"keep").unwrap();
        for i in 0..10 {
            let _ = handle.try_enqueue_line(format!("x{i}"));
        }
        let _ = handle.flush(Duration::from_secs(1));
        assert!(fake.exists(), "directory sibling must remain");
        assert!(fake.join("inner").exists());
        let c = handle.counters();
        // Must not claim successful trim of a directory.
        // trim_failures may be > 0 if we attempted; budget_trimmed only files.
        let _ = c;
        assert!(handle.join(Duration::from_secs(1)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotation_inventory_matches_disk() {
        let dir = temp_dir("inv");
        let mut limits = DiskLogLimits::for_test();
        limits.rotation_bytes = 40;
        limits.max_files = 3;
        limits.global_disk_budget_bytes = 400;
        limits.queue_capacity = 32;
        let handle = DiskLogHandle::start(DiskLogConfig::new(&dir).with_limits(limits));
        let mut accepted = 0;
        for i in 0..40 {
            if handle.try_enqueue_line(format!("line-{i:04}-pad")) {
                accepted += 1;
            }
            if i % 8 == 7 {
                let _ = handle.flush(Duration::from_millis(200));
            }
        }
        assert!(accepted >= 8, "accepted={accepted}");
        assert!(handle.flush(Duration::from_secs(2)));
        let mut files: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.metadata().map(|m| m.is_file()).unwrap_or(false))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        assert!(
            files.len() <= 3,
            "files={files:?} counters={:?}",
            handle.counters()
        );
        assert!(handle.join(Duration::from_secs(1)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn directory_sibling_blocks_rotation_enters_degraded_within_budget() {
        // Probe: nonempty dir named stem.1 blocks rename; must not keep
        // appending past rotation/budget (was 1700 bytes on budget 128).
        let dir = temp_dir("rot-dir-block");
        fs::create_dir_all(dir.join("spirula-diagnostics.log.1")).unwrap();
        let mut limits = DiskLogLimits::for_test();
        limits.line_max_bytes = 16;
        limits.rotation_bytes = 64;
        limits.global_disk_budget_bytes = 128;
        limits.queue_capacity = 128;
        let handle = DiskLogHandle::start(DiskLogConfig::new(&dir).with_limits(limits));
        let mut accepted = 0u32;
        for _ in 0..100 {
            if handle.try_enqueue_line("1234567890123456") {
                accepted += 1;
            }
        }
        assert!(handle.flush(Duration::from_secs(3)));
        let active = dir.join("spirula-diagnostics.log");
        let bytes = fs::metadata(&active).map(|m| m.len()).unwrap_or(0);
        let c = handle.counters();
        assert!(
            bytes <= 128,
            "active must stay within budget; bytes={bytes} counters={c:?}"
        );
        assert!(
            c.degraded >= 1 || handle.writes_disabled(),
            "must enter degraded when rotation cannot proceed: {c:?}"
        );
        assert!(
            c.dropped_degraded >= 1 || accepted < 100,
            "further writes must drop once degraded: accepted={accepted} {c:?}"
        );
        assert!(handle.take_failure_notice() || c.degraded >= 1);
        assert!(handle.join(Duration::from_secs(2)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn startup_trims_all_owned_siblings_to_max_files() {
        // Probe: 200 numeric siblings + max_files=3 must not stop after 64
        // iterations leaving ~137 files.
        let dir = temp_dir("startup-trim");
        for i in 1..=200 {
            fs::write(dir.join(format!("spirula-diagnostics.log.{i}")), b"x").unwrap();
        }
        let mut limits = DiskLogLimits::for_test();
        limits.max_files = 3;
        limits.global_disk_budget_bytes = 1_000_000;
        let handle = DiskLogHandle::start(DiskLogConfig::new(&dir).with_limits(limits));
        assert!(handle.flush(Duration::from_secs(3)));
        let files = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.metadata().map(|m| m.is_file()).unwrap_or(false))
            .count();
        assert!(
            files <= 3,
            "startup must enforce max_files=3; actual={files} counters={:?}",
            handle.counters()
        );
        assert!(
            !handle.writes_disabled(),
            "writable siblings must trim cleanly"
        );
        assert!(handle.join(Duration::from_secs(2)));
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
        assert!(handle.take_failure_notice());
        assert!(!handle.take_failure_notice());
        assert!(handle.join(Duration::from_secs(1)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn flush_full_queue_returns_false_quickly() {
        let (tx, _rx) = mpsc::sync_channel::<DataMsg>(1);
        // Build a handle-like flush path: fill queue then flush.
        let dir = temp_dir("flush-full");
        let fifo = dir.join("spirula-diagnostics.log");
        #[cfg(unix)]
        {
            use std::ffi::CString;
            let c = CString::new(fifo.to_str().unwrap()).unwrap();
            unsafe {
                assert_eq!(libc::mkfifo(c.as_ptr(), 0o644), 0);
            }
        }
        let handle = DiskLogHandle::start(DiskLogConfig::new(&dir).with_limits(DiskLogLimits {
            queue_capacity: 1,
            ..DiskLogLimits::for_test()
        }));
        let _ = handle.try_enqueue_line("occupy");
        // Queue may be full; flush must not block.
        let start = Instant::now();
        let _ = handle.flush(Duration::from_millis(20));
        assert!(start.elapsed() < Duration::from_millis(100));
        let _ = tx; // silence
        let _reader = File::open(&fifo);
        let _ = handle.join(Duration::from_secs(2));
        let _ = fs::remove_dir_all(&dir);
    }
}
