//! Fake probe worker. One long-lived thread; never awaits subscribers.
//!
//! Step delays are sliced by `cancel_poll_interval` so a stalled/paused step
//! still publishes heartbeats and observes cancel within documented deadlines.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

use spirula_core::{CancelReason, CancelToken, ProgressSnapshot, TerminalOutcome};

use crate::bound::sanitize_worker_event;
use crate::ids::{CommandId, RunId};
use crate::limits::SessionLimits;

/// How the fake worker should finish (tests inject failures/panics).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum FakeWorkerBehavior {
    #[default]
    Succeed,
    Fail,
    Panic,
    /// On cancel with save requested: emit `Canceled` and mark save failed.
    /// Owner must keep `Canceled` and publish `Warning` (§5), not session `Failed`.
    CancelSaveFail,
}

/// Optional stall: hold inside a step without advancing, still heartbeat/cancel.
#[derive(Clone, Debug, Default)]
pub struct StallGate {
    inner: Arc<Mutex<bool>>,
}

impl StallGate {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_stalled(&self, stalled: bool) {
        *lock(&self.inner) = stalled;
    }

    pub fn is_stalled(&self) -> bool {
        *lock(&self.inner)
    }

    pub(crate) fn clone_arc(&self) -> Arc<Mutex<bool>> {
        Arc::clone(&self.inner)
    }

    /// Same gate handle (shared stall flag).
    pub fn share(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

/// Image prepare work offloaded from the owner dispatch loop (P3-F02).
#[derive(Debug)]
pub(crate) enum ImagePrepareRequest {
    Oriented {
        path: String,
        orientation: Option<u8>,
        turns_only: bool,
    },
    Mask {
        path: String,
        dst_w: u32,
        dst_h: u32,
        orientation: u8,
        turns_only: bool,
        flip: bool,
        alpha: bool,
        boundary_offset_milli: i32,
    },
}

/// Successful image prepare payload (Prepared event fields only).
#[derive(Debug)]
pub enum ImagePrepareOk {
    Oriented {
        path: String,
        width: u32,
        height: u32,
        orientation: u8,
        turns_only: bool,
    },
    Mask {
        path: String,
        width: u32,
        height: u32,
        kept: u64,
        alpha: bool,
    },
}

/// Successful dataset load summary (DatasetLoaded fields only).
#[derive(Debug, Clone)]
pub enum DatasetLoadOk {
    Summary {
        path: String,
        format: String,
        num_cameras: u64,
        num_images: u64,
        num_points: u64,
        num_train: u64,
        num_eval: u64,
    },
}

/// Messages the owner sends to the worker thread.
#[derive(Debug)]
pub(crate) enum WorkerRequest {
    Load {
        run_id: RunId,
        command_id: CommandId,
        label: String,
        cancel: CancelToken,
    },
    Run {
        run_id: RunId,
        command_id: CommandId,
        steps: u64,
        cancel: CancelToken,
        pause: Arc<Mutex<bool>>,
        stall: Arc<Mutex<bool>>,
        /// Live flag so Stop{save} can update a running worker.
        save_on_cancel: Arc<AtomicBool>,
    },
    PrepareImage {
        run_id: RunId,
        command_id: CommandId,
        request: ImagePrepareRequest,
        cancel: CancelToken,
        /// Shared with probe stall gate so tests can hold decode while Stop runs.
        stall: Arc<Mutex<bool>>,
    },
    LoadDataset {
        run_id: RunId,
        command_id: CommandId,
        path: String,
        recon_dir: String,
        eval_mode: String,
        split: String,
        cancel: CancelToken,
        stall: Arc<Mutex<bool>>,
    },
    Shutdown,
}

/// Progress or terminal reports from the worker. Owner assigns `seq`.
#[derive(Debug)]
pub enum WorkerEvent {
    Progress {
        run_id: RunId,
        command_id: CommandId,
        snapshot: ProgressSnapshot,
    },
    Log {
        run_id: RunId,
        command_id: CommandId,
        message: String,
    },
    LoadDone {
        run_id: RunId,
        command_id: CommandId,
        label: String,
    },
    /// Image prepare finished on the worker. Never paired with a Prepared event
    /// after cancel — canceled work uses [`Self::RunTerminal`] only.
    ImagePrepareDone {
        run_id: RunId,
        command_id: CommandId,
        result: Result<ImagePrepareOk, spirula_core::DomainError>,
    },
    /// Dataset load finished. Never paired with DatasetLoaded after cancel.
    DatasetLoadDone {
        run_id: RunId,
        command_id: CommandId,
        result: Result<DatasetLoadOk, spirula_core::DomainError>,
    },
    RunTerminal {
        run_id: RunId,
        command_id: CommandId,
        outcome: TerminalOutcome,
        checkpoint_label: Option<String>,
        /// Save-on-cancel attempted and failed. Owner keeps `Canceled` + Warning.
        save_failed: bool,
        /// Canonical final progress; owner publishes this before the terminal.
        final_progress: Option<ProgressSnapshot>,
    },
}

#[derive(Debug)]
pub(crate) struct WorkerHandle {
    tx: std::sync::mpsc::Sender<WorkerRequest>,
    join: Option<JoinHandle<()>>,
}

impl WorkerHandle {
    pub(crate) fn spawn(
        limits: SessionLimits,
        behavior: Arc<Mutex<FakeWorkerBehavior>>,
        progress_tx: SyncSender<WorkerEvent>,
        result_tx: SyncSender<WorkerEvent>,
    ) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let join = thread::Builder::new()
            .name("spirula-fake-worker".into())
            .spawn(move || worker_loop(rx, progress_tx, result_tx, limits, behavior))
            .expect("spawn fake worker");
        Self {
            tx,
            join: Some(join),
        }
    }

    pub(crate) fn send(&self, req: WorkerRequest) {
        let _ = self.tx.send(req);
    }

    /// Request shutdown and join. Idempotent if already joined.
    pub(crate) fn shutdown_join(&mut self) {
        let _ = self.tx.send(WorkerRequest::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        self.shutdown_join();
    }
}

fn try_progress(
    progress_tx: &SyncSender<WorkerEvent>,
    limits: &SessionLimits,
    event: WorkerEvent,
) -> bool {
    let Some(event) = sanitize_worker_event(event, limits) else {
        return false;
    };
    match progress_tx.try_send(event) {
        Ok(()) => true,
        Err(TrySendError::Full(_)) => false,
        Err(TrySendError::Disconnected(_)) => false,
    }
}

/// Terminal/load results use a separate bounded mailbox. Blocking send is
/// allowed here: the owner drains this before progress and never awaits
/// subscribers while applying a result. Payloads are bounded before enqueue.
fn forward_result(result_tx: &SyncSender<WorkerEvent>, limits: &SessionLimits, event: WorkerEvent) {
    if let Some(event) = sanitize_worker_event(event, limits) {
        let _ = result_tx.send(event);
    }
}

fn worker_loop(
    rx: Receiver<WorkerRequest>,
    progress_tx: SyncSender<WorkerEvent>,
    result_tx: SyncSender<WorkerEvent>,
    limits: SessionLimits,
    behavior: Arc<Mutex<FakeWorkerBehavior>>,
) {
    loop {
        match rx.recv() {
            Ok(WorkerRequest::Shutdown) => break,
            Ok(WorkerRequest::LoadDataset {
                run_id,
                command_id,
                path,
                recon_dir,
                eval_mode,
                split,
                cancel,
                stall,
            }) => {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_dataset_load(
                        &progress_tx,
                        &result_tx,
                        &limits,
                        run_id,
                        command_id,
                        path,
                        recon_dir,
                        eval_mode,
                        split,
                        cancel,
                        stall,
                    );
                }));
                if result.is_err() {
                    let err = spirula_core::DomainError::try_new(
                        spirula_core::ErrorCode::Internal,
                        "dataset load worker panicked",
                    )
                    .expect("message");
                    forward_result(
                        &result_tx,
                        &limits,
                        WorkerEvent::RunTerminal {
                            run_id,
                            command_id,
                            outcome: TerminalOutcome::Failed { error: err },
                            checkpoint_label: None,
                            save_failed: false,
                            final_progress: None,
                        },
                    );
                }
            }
            Ok(WorkerRequest::Load {
                run_id,
                command_id,
                label,
                cancel,
            }) => {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    if wait_sliced(
                        &limits,
                        &cancel,
                        None,
                        None,
                        &progress_tx,
                        run_id,
                        command_id,
                        0,
                        1,
                        Instant::now(),
                        &mut Instant::now(),
                    )
                    .is_err()
                    {
                        hold_after_cancel_for_test(&progress_tx, &limits, run_id, command_id);
                        finish_canceled(
                            &result_tx, &limits, run_id, command_id, 0, &cancel, None, false, None,
                        );
                        return;
                    }
                    match *lock(&behavior) {
                        FakeWorkerBehavior::Panic => {
                            panic!("injected fake worker panic on load")
                        }
                        FakeWorkerBehavior::Fail => {
                            let err = spirula_core::DomainError::try_new(
                                spirula_core::ErrorCode::Internal,
                                "injected fake load failure",
                            )
                            .expect("message");
                            forward_result(
                                &result_tx,
                                &limits,
                                WorkerEvent::RunTerminal {
                                    run_id,
                                    command_id,
                                    outcome: TerminalOutcome::Failed { error: err },
                                    checkpoint_label: None,
                                    save_failed: false,
                                    final_progress: None,
                                },
                            );
                        }
                        FakeWorkerBehavior::Succeed | FakeWorkerBehavior::CancelSaveFail => {
                            forward_result(
                                &result_tx,
                                &limits,
                                WorkerEvent::LoadDone {
                                    run_id,
                                    command_id,
                                    label,
                                },
                            );
                        }
                    }
                }));
                if result.is_err() {
                    let err = spirula_core::DomainError::try_new(
                        spirula_core::ErrorCode::Internal,
                        "fake worker panic",
                    )
                    .expect("message");
                    forward_result(
                        &result_tx,
                        &limits,
                        WorkerEvent::RunTerminal {
                            run_id,
                            command_id,
                            outcome: TerminalOutcome::Failed { error: err },
                            checkpoint_label: None,
                            save_failed: false,
                            final_progress: None,
                        },
                    );
                }
            }
            Ok(WorkerRequest::Run {
                run_id,
                command_id,
                steps,
                cancel,
                pause,
                stall,
                save_on_cancel,
            }) => {
                // Survives unwind: last actual progress so Failed can carry it
                // on the result mailbox (owner drains results before progress).
                let panic_progress: Arc<Mutex<Option<ProgressSnapshot>>> =
                    Arc::new(Mutex::new(None));
                let handoff = Arc::clone(&panic_progress);
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_probe(
                        &progress_tx,
                        &result_tx,
                        &limits,
                        &behavior,
                        run_id,
                        command_id,
                        steps,
                        cancel,
                        pause,
                        stall,
                        save_on_cancel,
                        &handoff,
                    );
                }));
                if result.is_err() {
                    let last = lock(&panic_progress).take();
                    let err = spirula_core::DomainError::try_new(
                        spirula_core::ErrorCode::Internal,
                        "fake worker panic",
                    )
                    .expect("message");
                    forward_result(
                        &result_tx,
                        &limits,
                        WorkerEvent::RunTerminal {
                            run_id,
                            command_id,
                            outcome: TerminalOutcome::Failed { error: err },
                            checkpoint_label: None,
                            save_failed: false,
                            final_progress: last,
                        },
                    );
                }
            }
            Ok(WorkerRequest::PrepareImage {
                run_id,
                command_id,
                request,
                cancel,
                stall,
            }) => {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_image_prepare(
                        &progress_tx,
                        &result_tx,
                        &limits,
                        run_id,
                        command_id,
                        request,
                        cancel,
                        stall,
                    );
                }));
                if result.is_err() {
                    let err = spirula_core::DomainError::try_new(
                        spirula_core::ErrorCode::Internal,
                        "image prepare worker panic",
                    )
                    .expect("message");
                    forward_result(
                        &result_tx,
                        &limits,
                        WorkerEvent::ImagePrepareDone {
                            run_id,
                            command_id,
                            result: Err(err),
                        },
                    );
                }
            }
            Err(_) => break,
        }
    }
}

fn make_progress(step: u64, steps: u64, elapsed: Duration) -> ProgressSnapshot {
    ProgressSnapshot::try_new(
        step,
        Some(steps.max(step)),
        0,
        elapsed.as_millis() as u64,
        None,
        Vec::new(),
    )
    .expect("valid progress")
}

#[allow(clippy::too_many_arguments)]
fn run_probe(
    progress_tx: &SyncSender<WorkerEvent>,
    result_tx: &SyncSender<WorkerEvent>,
    limits: &SessionLimits,
    behavior: &Arc<Mutex<FakeWorkerBehavior>>,
    run_id: RunId,
    command_id: CommandId,
    steps: u64,
    cancel: CancelToken,
    pause: Arc<Mutex<bool>>,
    stall: Arc<Mutex<bool>>,
    save_on_cancel: Arc<AtomicBool>,
    panic_progress: &Arc<Mutex<Option<ProgressSnapshot>>>,
) {
    let started = Instant::now();
    let mut last_heartbeat = Instant::now()
        .checked_sub(limits.heartbeat_interval)
        .unwrap_or_else(Instant::now);
    let mut step = 0u64;
    let log_msg = format!("probe run start steps={steps}");
    let _ = try_progress(
        progress_tx,
        limits,
        WorkerEvent::Log {
            run_id,
            command_id,
            message: log_msg,
        },
    );

    while step < steps {
        if wait_sliced(
            limits,
            &cancel,
            Some(&pause),
            Some(&stall),
            progress_tx,
            run_id,
            command_id,
            step,
            steps,
            started,
            &mut last_heartbeat,
        )
        .is_err()
        {
            hold_after_cancel_for_test(progress_tx, limits, run_id, command_id);
            let want_save = save_on_cancel.load(Ordering::Acquire);
            let save_failed =
                want_save && matches!(*lock(behavior), FakeWorkerBehavior::CancelSaveFail);
            let label = (want_save && !save_failed).then(|| format!("probe-stop-{step}"));
            let final_progress = Some(make_progress(step, steps, started.elapsed()));
            if let Some(ref snap) = final_progress {
                note_panic_progress(panic_progress, snap.clone());
            }
            finish_canceled(
                result_tx,
                limits,
                run_id,
                command_id,
                step,
                &cancel,
                label,
                save_failed,
                final_progress,
            );
            return;
        }

        step += 1;
        let snap = make_progress(step, steps, started.elapsed());
        note_panic_progress(panic_progress, snap.clone());
        let _ = try_progress(
            progress_tx,
            limits,
            WorkerEvent::Progress {
                run_id,
                command_id,
                snapshot: snap,
            },
        );
        last_heartbeat = Instant::now();
    }

    let final_progress = make_progress(step, steps, started.elapsed());
    // Record before optional panic so catch_unwind can attach it to Failed.
    note_panic_progress(panic_progress, final_progress.clone());
    // Also try progress mailbox; canonical copy rides with the terminal.
    let _ = try_progress(
        progress_tx,
        limits,
        WorkerEvent::Progress {
            run_id,
            command_id,
            snapshot: final_progress.clone(),
        },
    );

    match *lock(behavior) {
        FakeWorkerBehavior::Panic => panic!("injected fake worker panic on run"),
        FakeWorkerBehavior::Fail => {
            let err = spirula_core::DomainError::try_new(
                spirula_core::ErrorCode::Internal,
                "injected fake run failure",
            )
            .expect("message");
            forward_result(
                result_tx,
                limits,
                WorkerEvent::RunTerminal {
                    run_id,
                    command_id,
                    outcome: TerminalOutcome::Failed { error: err },
                    checkpoint_label: None,
                    save_failed: false,
                    final_progress: Some(final_progress),
                },
            );
        }
        FakeWorkerBehavior::Succeed | FakeWorkerBehavior::CancelSaveFail => {
            forward_result(
                result_tx,
                limits,
                WorkerEvent::RunTerminal {
                    run_id,
                    command_id,
                    outcome: TerminalOutcome::Completed { step },
                    checkpoint_label: None,
                    save_failed: false,
                    final_progress: Some(final_progress),
                },
            );
        }
    }
}

fn note_panic_progress(slot: &Arc<Mutex<Option<ProgressSnapshot>>>, snapshot: ProgressSnapshot) {
    *lock(slot) = Some(snapshot);
}

/// Run admitted COLMAP load off the owner thread. Canceled work emits only
/// `RunTerminal::Canceled` (never `DatasetLoadDone` / DatasetLoaded).
#[allow(clippy::too_many_arguments)]
fn run_dataset_load(
    progress_tx: &SyncSender<WorkerEvent>,
    result_tx: &SyncSender<WorkerEvent>,
    limits: &SessionLimits,
    run_id: RunId,
    command_id: CommandId,
    path: String,
    recon_dir: String,
    eval_mode: String,
    split: String,
    cancel: CancelToken,
    stall: Arc<Mutex<bool>>,
) {
    if wait_image_gate(progress_tx, limits, run_id, command_id, &cancel, &stall).is_err() {
        hold_after_cancel_for_test(progress_tx, limits, run_id, command_id);
        finish_canceled(
            result_tx, limits, run_id, command_id, 0, &cancel, None, false, None,
        );
        return;
    }

    let outcome = match crate::dataset_ops::run_load_colmap_cancelable(
        std::path::Path::new(&path),
        if recon_dir.is_empty() {
            None
        } else {
            Some(recon_dir.as_str())
        },
        &eval_mode,
        &split,
        Some(&cancel),
    ) {
        crate::dataset_ops::DatasetWorkResult::Ready(summary) => Ok(DatasetLoadOk::Summary {
            path: summary.path,
            format: summary.format,
            num_cameras: summary.num_cameras,
            num_images: summary.num_images,
            num_points: summary.num_points,
            num_train: summary.num_train,
            num_eval: summary.num_eval,
        }),
        crate::dataset_ops::DatasetWorkResult::Failed(e) => Err(e),
        crate::dataset_ops::DatasetWorkResult::Canceled => {
            hold_after_cancel_for_test(progress_tx, limits, run_id, command_id);
            finish_canceled(
                result_tx, limits, run_id, command_id, 0, &cancel, None, false, None,
            );
            return;
        }
    };

    if cancel.is_cancelled() {
        hold_after_cancel_for_test(progress_tx, limits, run_id, command_id);
        finish_canceled(
            result_tx, limits, run_id, command_id, 0, &cancel, None, false, None,
        );
        return;
    }

    forward_result(
        result_tx,
        limits,
        WorkerEvent::DatasetLoadDone {
            run_id,
            command_id,
            result: outcome,
        },
    );
}

/// Run admitted image prepare off the owner thread. Cancel is polled before
/// decode and after completion; canceled work emits only `RunTerminal::Canceled`
/// (never `ImagePrepareDone` / Prepared).
#[allow(clippy::too_many_arguments)]
fn run_image_prepare(
    progress_tx: &SyncSender<WorkerEvent>,
    result_tx: &SyncSender<WorkerEvent>,
    limits: &SessionLimits,
    run_id: RunId,
    command_id: CommandId,
    request: ImagePrepareRequest,
    cancel: CancelToken,
    stall: Arc<Mutex<bool>>,
) {
    if wait_image_gate(progress_tx, limits, run_id, command_id, &cancel, &stall).is_err() {
        hold_after_cancel_for_test(progress_tx, limits, run_id, command_id);
        finish_canceled(
            result_tx, limits, run_id, command_id, 0, &cancel, None, false, None,
        );
        return;
    }

    use crate::image_ops::ImageWorkResult;
    let outcome = match request {
        ImagePrepareRequest::Oriented {
            path,
            orientation,
            turns_only,
        } => {
            let policy = if turns_only {
                spirula_io::ApplyPolicy::TurnsOnly
            } else {
                spirula_io::ApplyPolicy::FullTransform
            };
            match crate::image_ops::run_prepare_oriented_rgb_cancelable(
                std::path::Path::new(&path),
                orientation,
                policy,
                Some(&cancel),
            ) {
                ImageWorkResult::Ready(img) => Ok(ImagePrepareOk::Oriented {
                    path,
                    width: img.width,
                    height: img.height,
                    orientation: img.orientation,
                    turns_only,
                }),
                ImageWorkResult::Failed(e) => Err(e),
                ImageWorkResult::Canceled => {
                    hold_after_cancel_for_test(progress_tx, limits, run_id, command_id);
                    finish_canceled(
                        result_tx, limits, run_id, command_id, 0, &cancel, None, false, None,
                    );
                    return;
                }
            }
        }
        ImagePrepareRequest::Mask {
            path,
            dst_w,
            dst_h,
            orientation,
            turns_only,
            flip,
            alpha,
            boundary_offset_milli,
        } => {
            let args = crate::image_ops::PrepareMaskArgs {
                dst_w,
                dst_h,
                kind: if alpha {
                    spirula_io::MaskSourceKind::AlphaThreshold128
                } else {
                    spirula_io::MaskSourceKind::NonZero
                },
                orientation,
                policy: if turns_only {
                    spirula_io::ApplyPolicy::TurnsOnly
                } else {
                    spirula_io::ApplyPolicy::FullTransform
                },
                flip,
                boundary_offset_frac: (boundary_offset_milli as f32) / 1000.0,
            };
            match crate::image_ops::run_prepare_normalized_mask_cancelable(
                std::path::Path::new(&path),
                args,
                Some(&cancel),
            ) {
                ImageWorkResult::Ready(mask) => {
                    // Wide masks: count kept with bounded-chunk cancel ticks.
                    let mut cp = spirula_io::CancelCheckpoint::new(Some(&cancel));
                    let mut kept = 0u64;
                    let mut canceled = false;
                    for &v in &mask.samples {
                        if cp.tick(1).is_err() {
                            canceled = true;
                            break;
                        }
                        if v == 1 {
                            kept = kept.saturating_add(1);
                        }
                    }
                    if canceled {
                        hold_after_cancel_for_test(progress_tx, limits, run_id, command_id);
                        finish_canceled(
                            result_tx, limits, run_id, command_id, 0, &cancel, None, false, None,
                        );
                        return;
                    }
                    Ok(ImagePrepareOk::Mask {
                        path,
                        width: mask.width,
                        height: mask.height,
                        kept,
                        alpha,
                    })
                }
                ImageWorkResult::Failed(e) => Err(e),
                ImageWorkResult::Canceled => {
                    hold_after_cancel_for_test(progress_tx, limits, run_id, command_id);
                    finish_canceled(
                        result_tx, limits, run_id, command_id, 0, &cancel, None, false, None,
                    );
                    return;
                }
            }
        }
    };

    // Final gate before enqueue: cancel admitted during the last stage still
    // wins (owner also arbitrates queued ImagePrepareDone vs pending cancel).
    if cancel.is_cancelled() {
        hold_after_cancel_for_test(progress_tx, limits, run_id, command_id);
        finish_canceled(
            result_tx, limits, run_id, command_id, 0, &cancel, None, false, None,
        );
        return;
    }

    forward_result(
        result_tx,
        limits,
        WorkerEvent::ImagePrepareDone {
            run_id,
            command_id,
            result: outcome,
        },
    );
}

/// Poll cancel while optionally stalled (same StallGate as probe runs).
/// `Err(())` means cancel won before decode should start.
fn wait_image_gate(
    progress_tx: &SyncSender<WorkerEvent>,
    limits: &SessionLimits,
    run_id: RunId,
    command_id: CommandId,
    cancel: &CancelToken,
    stall: &Mutex<bool>,
) -> Result<(), ()> {
    let poll = limits.poll_interval();
    let mut announced = false;
    loop {
        if cancel.is_cancelled() {
            return Err(());
        }
        if !*lock(stall) {
            return Ok(());
        }
        if !announced {
            let _ = try_progress(
                progress_tx,
                limits,
                WorkerEvent::Log {
                    run_id,
                    command_id,
                    message: "spirula:test image-prepare-stalled".into(),
                },
            );
            announced = true;
        }
        thread::sleep(poll.min(Duration::from_millis(5)));
    }
}

/// Test-only unfinished-cancel gate (`SPIRULA_TEST_HOLD_AFTER_CANCEL`).
///
/// After cancel is observed, publish an observed barrier log and block before
/// emitting the canceled terminal so a second SIGINT can force `SIG_DFL`
/// death. Independent of `worker_step_delay` / `wait_sliced` poll timing.
fn hold_after_cancel_for_test(
    progress_tx: &SyncSender<WorkerEvent>,
    limits: &SessionLimits,
    run_id: RunId,
    command_id: CommandId,
) {
    if std::env::var_os("SPIRULA_TEST_HOLD_AFTER_CANCEL").is_none() {
        return;
    }
    let _ = try_progress(
        progress_tx,
        limits,
        WorkerEvent::Log {
            run_id,
            command_id,
            message: "spirula:test cancel-held".into(),
        },
    );
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}

/// Sleep for one step delay (or until cancel), while paused/stalled, honoring
/// heartbeat and cancel poll deadlines. `Err(())` means canceled.
#[allow(clippy::too_many_arguments)]
fn wait_sliced(
    limits: &SessionLimits,
    cancel: &CancelToken,
    pause: Option<&Mutex<bool>>,
    stall: Option<&Mutex<bool>>,
    progress_tx: &SyncSender<WorkerEvent>,
    run_id: RunId,
    command_id: CommandId,
    step: u64,
    steps: u64,
    started: Instant,
    last_heartbeat: &mut Instant,
) -> Result<(), ()> {
    let poll = limits.poll_interval();
    let mut remaining = limits.worker_step_delay;
    loop {
        if cancel.is_cancelled() {
            return Err(());
        }
        let paused = pause.is_some_and(|p| *lock(p));
        let stalled = stall.is_some_and(|s| *lock(s));
        if last_heartbeat.elapsed() >= limits.heartbeat_interval {
            publish_progress(
                progress_tx,
                limits,
                run_id,
                command_id,
                step,
                steps,
                started.elapsed(),
            );
            *last_heartbeat = Instant::now();
        }
        if !paused && !stalled && remaining.is_zero() {
            return Ok(());
        }
        let slice = if paused || stalled {
            poll
        } else {
            remaining.min(poll)
        };
        thread::sleep(slice);
        if !paused && !stalled {
            remaining = remaining.saturating_sub(slice);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn finish_canceled(
    result_tx: &SyncSender<WorkerEvent>,
    limits: &SessionLimits,
    run_id: RunId,
    command_id: CommandId,
    step: u64,
    cancel: &CancelToken,
    checkpoint_label: Option<String>,
    save_failed: bool,
    final_progress: Option<ProgressSnapshot>,
) {
    let reason = cancel.reason().unwrap_or(CancelReason::User);
    let saved = if save_failed {
        None
    } else {
        checkpoint_label.clone()
    };
    forward_result(
        result_tx,
        limits,
        WorkerEvent::RunTerminal {
            run_id,
            command_id,
            outcome: TerminalOutcome::Canceled {
                step,
                reason,
                saved_checkpoint: saved,
            },
            checkpoint_label: if save_failed { None } else { checkpoint_label },
            save_failed,
            final_progress,
        },
    );
}

fn publish_progress(
    progress_tx: &SyncSender<WorkerEvent>,
    limits: &SessionLimits,
    run_id: RunId,
    command_id: CommandId,
    step: u64,
    steps: u64,
    elapsed: Duration,
) {
    let snapshot = make_progress(step, steps, elapsed);
    let _ = try_progress(
        progress_tx,
        limits,
        WorkerEvent::Progress {
            run_id,
            command_id,
            snapshot,
        },
    );
}
