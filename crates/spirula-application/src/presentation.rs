//! P6-F07 application-owned presentation path (latest-frame slot).
//!
//! GPU open/produce runs on the presentation worker only. UI polls status and
//! frames without blocking. Binding: `docs/phase-6/presentation-path.md`.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use spirula_core::{
    CancelReason, CancelToken, ContextKey, DomainError, ErrorCode, PresentedFrame,
    rgba8_matches_cpu_probe,
};
use spirula_gpu::{
    CapabilityRequest, ContextStatus, GpuContext, WGPU_VERSION, produce_probe_rgba_frame,
};

/// Snapshot of presentation counters (capacity-1 latest-frame slot).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PresentationStats {
    pub produced: u64,
    pub taken: u64,
    pub overwritten: u64,
    pub stale_dropped: u64,
    pub canceled: u64,
    pub failed: u64,
}

/// Host clocks around one delivered frame (set-image / paint stay in UI).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveredFrame {
    pub frame: PresentedFrame,
    pub produce_to_slot: Duration,
    pub produce_to_take: Duration,
}

/// Worker / GPU lifecycle visible to UI (non-blocking poll).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresentationStatus {
    /// Worker started; `GpuContext::open` not finished yet.
    Opening,
    Ready {
        device_label: String,
    },
    Failed {
        code: ErrorCode,
        message: String,
    },
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerCmd {
    ProduceOnce,
    Shutdown,
}

struct Slot {
    frame: PresentedFrame,
    produce_started: Instant,
    slotted_at: Instant,
}

struct SizeState {
    width: u32,
    height: u32,
    /// Bumps on every successful [`PresentationOwner::resize`].
    generation: u64,
}

struct Shared {
    size: Mutex<SizeState>,
    slot: Mutex<Option<Slot>>,
    stats: Mutex<PresentationStats>,
    cancel: CancelToken,
    shutdown: AtomicBool,
    next_frame_id: AtomicU64,
    last_error: Mutex<Option<DomainError>>,
    status: Mutex<PresentationStatus>,
    status_cv: Condvar,
    /// Monotonic produce completions (including stale drops) for barriers.
    produce_epoch: AtomicU64,
    produce_cv: Condvar,
    produce_mu: Mutex<()>,
}

/// Application presentation owner: GPU worker + bounded latest-frame slot.
pub struct PresentationOwner {
    shared: Arc<Shared>,
    cmd_tx: SyncSender<WorkerCmd>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl PresentationOwner {
    /// Start the presentation worker. **Does not** open GPU on the caller
    /// thread — poll [`Self::status`] / [`Self::wait_ready`] for Ready/Failed.
    pub fn start(width: u32, height: u32) -> Result<Self, DomainError> {
        validate_size(width, height)?;
        let shared = Arc::new(Shared {
            size: Mutex::new(SizeState {
                width,
                height,
                generation: 1,
            }),
            slot: Mutex::new(None),
            stats: Mutex::new(PresentationStats::default()),
            cancel: CancelToken::new(),
            shutdown: AtomicBool::new(false),
            next_frame_id: AtomicU64::new(1),
            last_error: Mutex::new(None),
            status: Mutex::new(PresentationStatus::Opening),
            status_cv: Condvar::new(),
            produce_epoch: AtomicU64::new(0),
            produce_cv: Condvar::new(),
            produce_mu: Mutex::new(()),
        });

        // Capacity 1: ProduceOnce coalesces; Resize is never queued (shared size).
        let (cmd_tx, cmd_rx) = mpsc::sync_channel::<WorkerCmd>(1);
        let shared_worker = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("spirula-presentation".into())
            .spawn(move || {
                worker_loop(shared_worker, cmd_rx);
            })
            .map_err(|e| {
                DomainError::try_new(
                    ErrorCode::Internal,
                    format!("presentation worker spawn failed: {e}"),
                )
                .expect("msg")
            })?;

        Ok(Self {
            shared,
            cmd_tx,
            worker: Mutex::new(Some(worker)),
        })
    }

    /// Alias used by older call sites; same as [`Self::start`].
    pub fn open(width: u32, height: u32) -> Result<Self, DomainError> {
        Self::start(width, height)
    }

    pub fn status(&self) -> PresentationStatus {
        self.shared
            .status
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Deterministic barrier: wait until Ready or Failed (no sleep guessing).
    pub fn wait_ready(&self, timeout: Duration) -> Result<(), DomainError> {
        let mut st = self.shared.status.lock().unwrap_or_else(|p| p.into_inner());
        let deadline = Instant::now() + timeout;
        loop {
            match &*st {
                PresentationStatus::Ready { .. } => return Ok(()),
                PresentationStatus::Failed { code, message } => {
                    return Err(DomainError::try_new(*code, message.clone()).expect("msg"));
                }
                PresentationStatus::Closed => {
                    return Err(invalid_state("presentation closed before ready"));
                }
                PresentationStatus::Opening => {}
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(DomainError::try_new(
                    ErrorCode::SubmitFailed,
                    "presentation wait_ready timed out",
                )
                .expect("msg"));
            }
            let (guard, wait_res) = self
                .shared
                .status_cv
                .wait_timeout(st, deadline.saturating_duration_since(now))
                .unwrap_or_else(|p| p.into_inner());
            st = guard;
            let _ = wait_res;
            // Loop re-checks status / deadline.
        }
    }

    pub fn device_label(&self) -> String {
        match self.status() {
            PresentationStatus::Ready { device_label } => device_label,
            _ => format!("wgpu={WGPU_VERSION} status=not-ready"),
        }
    }

    pub fn stats(&self) -> PresentationStats {
        *self.shared.stats.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn last_error(&self) -> Option<DomainError> {
        self.shared
            .last_error
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(DomainError::without_source)
    }

    pub fn size(&self) -> (u32, u32) {
        let s = self.shared.size.lock().unwrap_or_else(|p| p.into_inner());
        (s.width, s.height)
    }

    pub fn size_generation(&self) -> u64 {
        self.shared
            .size
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .generation
    }

    /// Apply size immediately (not via the cmd queue) and bump generation.
    ///
    /// Clears the latest-frame slot so a cached prior-generation frame cannot
    /// be taken. Old queued Resize commands are impossible because resize is
    /// never enqueued (fixes Metal burst where stale Resize replayed size).
    pub fn resize(&self, width: u32, height: u32) -> Result<(), DomainError> {
        validate_size(width, height)?;
        if self.shared.shutdown.load(Ordering::Acquire)
            || matches!(self.status(), PresentationStatus::Closed)
        {
            return Err(invalid_state("presentation shut down"));
        }
        {
            let mut s = self.shared.size.lock().unwrap_or_else(|p| p.into_inner());
            s.width = width;
            s.height = height;
            s.generation = s.generation.saturating_add(1);
        }
        *self.shared.slot.lock().unwrap_or_else(|p| p.into_inner()) = None;
        Ok(())
    }

    /// Ask the worker to produce one frame (coalesced; never blocks the caller).
    pub fn request_frame(&self) -> Result<(), DomainError> {
        if self.shared.shutdown.load(Ordering::Acquire) {
            return Err(invalid_state("presentation shut down"));
        }
        match self.status() {
            PresentationStatus::Ready { .. } => {}
            PresentationStatus::Opening => {
                return Err(invalid_state("presentation still opening"));
            }
            PresentationStatus::Failed { code, message } => {
                return Err(DomainError::try_new(code, message).expect("msg"));
            }
            PresentationStatus::Closed => {
                return Err(invalid_state("presentation closed"));
            }
        }
        match self.cmd_tx.try_send(WorkerCmd::ProduceOnce) {
            Ok(()) => Ok(()),
            Err(mpsc::TrySendError::Full(_)) => Ok(()), // already pending produce
            Err(mpsc::TrySendError::Disconnected(_)) => {
                Err(invalid_state("presentation worker disconnected"))
            }
        }
    }

    /// Take the latest frame if present **and** its `size_generation` matches
    /// the current shared generation.
    pub fn take_latest(&self) -> Option<DeliveredFrame> {
        let current_gen = self.size_generation();
        let mut slot_guard = self.shared.slot.lock().unwrap_or_else(|p| p.into_inner());
        let slot = slot_guard.take()?;
        if slot.frame.size_generation != current_gen {
            let mut stats = self.shared.stats.lock().unwrap_or_else(|p| p.into_inner());
            stats.stale_dropped = stats.stale_dropped.saturating_add(1);
            return None;
        }
        let produce_to_take = slot.produce_started.elapsed();
        let produce_to_slot = slot
            .slotted_at
            .saturating_duration_since(slot.produce_started);
        {
            let mut stats = self.shared.stats.lock().unwrap_or_else(|p| p.into_inner());
            stats.taken = stats.taken.saturating_add(1);
        }
        Some(DeliveredFrame {
            frame: slot.frame,
            produce_to_slot,
            produce_to_take,
        })
    }

    pub fn peek_frame_id(&self) -> Option<u64> {
        let current_gen = self.size_generation();
        self.shared
            .slot
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .filter(|s| s.frame.size_generation == current_gen)
            .map(|s| s.frame.frame_id)
    }

    /// Wait until `produce_epoch` advances past `after` (deterministic barrier).
    pub fn wait_produce_epoch(&self, after: u64, timeout: Duration) -> Result<u64, DomainError> {
        let mut guard = self
            .shared
            .produce_mu
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let deadline = Instant::now() + timeout;
        loop {
            let now = self.shared.produce_epoch.load(Ordering::Acquire);
            if now > after {
                return Ok(now);
            }
            let rem = deadline.saturating_duration_since(Instant::now());
            if rem.is_zero() {
                return Err(DomainError::try_new(
                    ErrorCode::SubmitFailed,
                    "presentation wait_produce_epoch timed out",
                )
                .expect("msg"));
            }
            let (g, wait_res) = self
                .shared
                .produce_cv
                .wait_timeout(guard, rem)
                .unwrap_or_else(|p| p.into_inner());
            guard = g;
            let _ = wait_res;
        }
    }

    pub fn produce_epoch(&self) -> u64 {
        self.shared.produce_epoch.load(Ordering::Acquire)
    }

    /// Request one frame and wait until a matching-generation frame is takeable.
    pub fn request_and_wait_frame(&self, timeout: Duration) -> Result<DeliveredFrame, DomainError> {
        let before = self.produce_epoch();
        self.request_frame()?;
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(d) = self.take_latest() {
                return Ok(d);
            }
            let rem = deadline.saturating_duration_since(Instant::now());
            if rem.is_zero() {
                return Err(DomainError::try_new(
                    ErrorCode::SubmitFailed,
                    "presentation request_and_wait_frame timed out",
                )
                .expect("msg"));
            }
            let _ = self.wait_produce_epoch(before, rem)?;
        }
    }

    pub fn cancel(&self) {
        self.shared.cancel.request(CancelReason::User);
    }

    /// Stop worker and close GPU. Owner remains but cannot resurrect
    /// ([`Self::request_frame`] / [`Self::resize`] fail with Closed).
    pub fn close(&mut self) {
        self.shared.shutdown.store(true, Ordering::Release);
        self.shared.cancel.request(CancelReason::Shutdown);
        let _ = self.cmd_tx.send(WorkerCmd::Shutdown);
        if let Ok(mut guard) = self.worker.lock()
            && let Some(handle) = guard.take()
        {
            let _ = handle.join();
        }
        *self.shared.slot.lock().unwrap_or_else(|p| p.into_inner()) = None;
        set_status(&self.shared, PresentationStatus::Closed);
    }

    /// Consume and close (same as [`Self::close`]).
    pub fn shutdown(mut self) {
        self.close();
    }
}

impl Drop for PresentationOwner {
    fn drop(&mut self) {
        if !self.shared.shutdown.load(Ordering::Acquire) {
            self.close();
        } else if let Ok(mut guard) = self.worker.lock()
            && let Some(handle) = guard.take()
        {
            let _ = handle.join();
            set_status(&self.shared, PresentationStatus::Closed);
        }
    }
}

fn set_status(shared: &Shared, status: PresentationStatus) {
    *shared.status.lock().unwrap_or_else(|p| p.into_inner()) = status;
    shared.status_cv.notify_all();
}

fn bump_produce_epoch(shared: &Shared) {
    shared.produce_epoch.fetch_add(1, Ordering::AcqRel);
    let _guard = shared.produce_mu.lock().unwrap_or_else(|p| p.into_inner());
    shared.produce_cv.notify_all();
}

fn validate_size(width: u32, height: u32) -> Result<(), DomainError> {
    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        return Err(DomainError::try_new(
            ErrorCode::ValueOutOfRange,
            format!("presentation size {width}x{height} out of allowed 1..=4096"),
        )
        .expect("msg")
        .with_context(ContextKey::Field, "size")
        .expect("ctx"));
    }
    Ok(())
}

fn invalid_state(msg: impl Into<String>) -> DomainError {
    DomainError::try_new(ErrorCode::InvalidState, msg).expect("msg")
}

fn worker_loop(shared: Arc<Shared>, cmd_rx: mpsc::Receiver<WorkerCmd>) {
    let request = CapabilityRequest::diagnose_default();
    let mut ctx = match GpuContext::open(&request) {
        Ok(c) => c,
        Err(e) => {
            set_status(
                &shared,
                PresentationStatus::Failed {
                    code: e.error.code(),
                    message: e.error.message().to_owned(),
                },
            );
            // Drain until shutdown.
            while let Ok(cmd) = cmd_rx.recv() {
                if matches!(cmd, WorkerCmd::Shutdown) {
                    break;
                }
            }
            set_status(&shared, PresentationStatus::Closed);
            return;
        }
    };

    let device_label = ctx
        .report()
        .map(|r| {
            format!(
                "wgpu={WGPU_VERSION} backend={} device={}",
                r.backend.as_str(),
                r.adapter.identity.name
            )
        })
        .unwrap_or_else(|| format!("wgpu={WGPU_VERSION}"));
    set_status(
        &shared,
        PresentationStatus::Ready {
            device_label: device_label.clone(),
        },
    );

    loop {
        if shared.shutdown.load(Ordering::Acquire) {
            break;
        }
        let cmd = match cmd_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(c) => c,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        match cmd {
            WorkerCmd::Shutdown => break,
            WorkerCmd::ProduceOnce => {
                if shared.cancel.is_cancelled() {
                    let mut stats = shared.stats.lock().unwrap_or_else(|p| p.into_inner());
                    stats.canceled = stats.canceled.saturating_add(1);
                    bump_produce_epoch(&shared);
                    continue;
                }
                let (width, height, size_gen) = {
                    let s = shared.size.lock().unwrap_or_else(|p| p.into_inner());
                    (s.width, s.height, s.generation)
                };
                let frame_id = shared.next_frame_id.fetch_add(1, Ordering::SeqCst);
                let produce_started = Instant::now();
                match produce_probe_rgba_frame(
                    &mut ctx,
                    width,
                    height,
                    frame_id,
                    size_gen,
                    Some(&shared.cancel),
                ) {
                    Ok(frame) => {
                        debug_assert!(rgba8_matches_cpu_probe(width, height, &frame.rgba8));
                        let current_gen = shared
                            .size
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .generation;
                        if frame.size_generation != current_gen {
                            let mut stats = shared.stats.lock().unwrap_or_else(|p| p.into_inner());
                            stats.stale_dropped = stats.stale_dropped.saturating_add(1);
                            bump_produce_epoch(&shared);
                            continue;
                        }
                        let slotted_at = Instant::now();
                        let mut slot = shared.slot.lock().unwrap_or_else(|p| p.into_inner());
                        let mut stats = shared.stats.lock().unwrap_or_else(|p| p.into_inner());
                        if slot.is_some() {
                            stats.overwritten = stats.overwritten.saturating_add(1);
                        }
                        stats.produced = stats.produced.saturating_add(1);
                        *slot = Some(Slot {
                            frame,
                            produce_started,
                            slotted_at,
                        });
                        drop(stats);
                        drop(slot);
                        bump_produce_epoch(&shared);
                    }
                    Err(err) => {
                        let code = err.code();
                        let mut stats = shared.stats.lock().unwrap_or_else(|p| p.into_inner());
                        if code == ErrorCode::Canceled {
                            stats.canceled = stats.canceled.saturating_add(1);
                        } else {
                            stats.failed = stats.failed.saturating_add(1);
                        }
                        *shared.last_error.lock().unwrap_or_else(|p| p.into_inner()) = Some(err);
                        bump_produce_epoch(&shared);
                    }
                }
            }
        }
    }

    if ctx.status() == ContextStatus::Live {
        let _ = ctx.resource_accounting();
        ctx.close();
    }
    set_status(&shared, PresentationStatus::Closed);
}

/// Headless one-shot produce (caller thread opens GPU — diagnose/tests only).
pub fn produce_presentation_frame_once(
    width: u32,
    height: u32,
    cancel: Option<&CancelToken>,
) -> Result<(PresentedFrame, u64, usize), DomainError> {
    validate_size(width, height)?;
    let request = CapabilityRequest::diagnose_default();
    let mut ctx = GpuContext::open(&request).map_err(|e| e.error)?;
    let frame = produce_probe_rgba_frame(&mut ctx, width, height, 1, 0, cancel)?;
    let after = ctx.resource_accounting().current_bytes;
    let retained = ctx.retained_submission_count();
    ctx.close();
    Ok((frame, after, retained))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn illegal_size_rejected_without_gpu() {
        match PresentationOwner::start(0, 8) {
            Ok(_) => panic!("expected size error"),
            Err(err) => assert_eq!(err.code(), ErrorCode::ValueOutOfRange),
        }
    }
}
