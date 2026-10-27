//! P6-F07 application-owned presentation path (latest-frame slot).
//!
//! Opens `GpuContext` and produces host [`PresentedFrame`] values. UI consumes
//! frames only — never `wgpu` types. Binding: `docs/phase-6/presentation-path.md`.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
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
    pub canceled: u64,
    pub failed: u64,
}

/// Host clocks around one delivered frame (set-image / paint stay in UI).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveredFrame {
    pub frame: PresentedFrame,
    /// Host time from produce start to slot write.
    pub produce_to_slot: Duration,
    /// Host time from produce start to [`PresentationOwner::take_latest`] (if taken).
    pub produce_to_take: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerCmd {
    Resize { width: u32, height: u32 },
    ProduceOnce,
    Shutdown,
}

struct Slot {
    frame: PresentedFrame,
    produce_started: Instant,
    slotted_at: Instant,
}

struct Shared {
    size: Mutex<(u32, u32)>,
    slot: Mutex<Option<Slot>>,
    stats: Mutex<PresentationStats>,
    cancel: CancelToken,
    shutdown: AtomicBool,
    next_frame_id: AtomicU64,
    last_error: Mutex<Option<DomainError>>,
    device_label: Mutex<String>,
}

/// Application presentation owner: GPU worker + bounded latest-frame slot.
pub struct PresentationOwner {
    shared: Arc<Shared>,
    cmd_tx: SyncSender<WorkerCmd>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl PresentationOwner {
    /// Open a live GPU context and start the presentation worker.
    pub fn open(width: u32, height: u32) -> Result<Self, DomainError> {
        validate_size(width, height)?;
        let request = CapabilityRequest::diagnose_default();
        let ctx = GpuContext::open(&request).map_err(|e| e.error)?;
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

        let shared = Arc::new(Shared {
            size: Mutex::new((width, height)),
            slot: Mutex::new(None),
            stats: Mutex::new(PresentationStats::default()),
            cancel: CancelToken::new(),
            shutdown: AtomicBool::new(false),
            next_frame_id: AtomicU64::new(1),
            last_error: Mutex::new(None),
            device_label: Mutex::new(device_label),
        });

        let (cmd_tx, cmd_rx) = mpsc::sync_channel::<WorkerCmd>(4);
        let shared_worker = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("spirula-presentation".into())
            .spawn(move || {
                worker_loop(ctx, shared_worker, cmd_rx);
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

    pub fn device_label(&self) -> String {
        self.shared
            .device_label
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
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
        *self.shared.size.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Request a size change; next produce uses the new dimensions.
    pub fn resize(&self, width: u32, height: u32) -> Result<(), DomainError> {
        validate_size(width, height)?;
        *self.shared.size.lock().unwrap_or_else(|p| p.into_inner()) = (width, height);
        let _ = self.cmd_tx.try_send(WorkerCmd::Resize { width, height });
        Ok(())
    }

    /// Ask the worker to produce one frame into the latest-frame slot.
    ///
    /// Non-blocking for the caller: if the command queue is full, the request
    /// is dropped (producer already busy); slow consumers never block the
    /// worker because the slot is overwrite-latest.
    pub fn request_frame(&self) -> Result<(), DomainError> {
        if self.shared.shutdown.load(Ordering::Acquire) {
            return Err(invalid_state("presentation shut down"));
        }
        match self.cmd_tx.try_send(WorkerCmd::ProduceOnce) {
            Ok(()) => Ok(()),
            Err(mpsc::TrySendError::Full(_)) => Ok(()),
            Err(mpsc::TrySendError::Disconnected(_)) => {
                Err(invalid_state("presentation worker disconnected"))
            }
        }
    }

    /// Take the latest frame if present (clears the slot).
    pub fn take_latest(&self) -> Option<DeliveredFrame> {
        let slot = self
            .shared
            .slot
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()?;
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

    /// Peek without taking (test helper).
    pub fn peek_frame_id(&self) -> Option<u64> {
        self.shared
            .slot
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(|s| s.frame.frame_id)
    }

    pub fn cancel(&self) {
        self.shared.cancel.request(CancelReason::User);
    }

    /// Cooperative shutdown: stop worker, close GPU context, clear slot.
    pub fn shutdown(self) {
        self.shared.shutdown.store(true, Ordering::Release);
        self.shared.cancel.request(CancelReason::Shutdown);
        let _ = self.cmd_tx.send(WorkerCmd::Shutdown);
        if let Ok(mut guard) = self.worker.lock()
            && let Some(handle) = guard.take()
        {
            let _ = handle.join();
        }
        *self.shared.slot.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }
}

impl Drop for PresentationOwner {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Release);
        self.shared.cancel.request(CancelReason::Shutdown);
        let _ = self.cmd_tx.send(WorkerCmd::Shutdown);
        if let Ok(mut guard) = self.worker.lock()
            && let Some(handle) = guard.take()
        {
            let _ = handle.join();
        }
    }
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

fn worker_loop(mut ctx: GpuContext, shared: Arc<Shared>, cmd_rx: mpsc::Receiver<WorkerCmd>) {
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
            WorkerCmd::Resize { width, height } => {
                *shared.size.lock().unwrap_or_else(|p| p.into_inner()) = (width, height);
            }
            WorkerCmd::ProduceOnce => {
                if shared.cancel.is_cancelled() {
                    let mut stats = shared.stats.lock().unwrap_or_else(|p| p.into_inner());
                    stats.canceled = stats.canceled.saturating_add(1);
                    continue;
                }
                let (width, height) = *shared.size.lock().unwrap_or_else(|p| p.into_inner());
                let frame_id = shared.next_frame_id.fetch_add(1, Ordering::SeqCst);
                let produce_started = Instant::now();
                match produce_probe_rgba_frame(
                    &mut ctx,
                    width,
                    height,
                    frame_id,
                    Some(&shared.cancel),
                ) {
                    Ok(frame) => {
                        debug_assert!(rgba8_matches_cpu_probe(width, height, &frame.rgba8));
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
                    }
                }
            }
        }
    }
    // Release GPU resources before thread exit.
    if ctx.status() == ContextStatus::Live {
        let _ = ctx.resource_accounting();
        ctx.close();
    }
}

/// Headless one-shot produce without a long-lived owner (tests / diagnose).
pub fn produce_presentation_frame_once(
    width: u32,
    height: u32,
    cancel: Option<&CancelToken>,
) -> Result<(PresentedFrame, u64), DomainError> {
    validate_size(width, height)?;
    let request = CapabilityRequest::diagnose_default();
    let mut ctx = GpuContext::open(&request).map_err(|e| e.error)?;
    let frame = produce_probe_rgba_frame(&mut ctx, width, height, 1, cancel)?;
    let after = ctx.resource_accounting().current_bytes;
    ctx.close();
    Ok((frame, after))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn illegal_size_rejected_without_gpu() {
        match PresentationOwner::open(0, 8) {
            Ok(_) => panic!("expected size error"),
            Err(err) => assert_eq!(err.code(), ErrorCode::ValueOutOfRange),
        }
    }
}
