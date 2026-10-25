//! Fake probe worker. One long-lived thread; never awaits subscribers.
//!
//! Step delays are sliced by `cancel_poll_interval` so a stalled/paused step
//! still publishes heartbeats and observes cancel within documented deadlines.

use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use spirula_core::{CancelReason, CancelToken, ProgressSnapshot, TerminalOutcome};

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
        *self.inner.lock().expect("stall") = stalled;
    }

    pub fn is_stalled(&self) -> bool {
        *self.inner.lock().expect("stall")
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
        save_on_cancel: bool,
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
    RunTerminal {
        run_id: RunId,
        command_id: CommandId,
        outcome: TerminalOutcome,
        checkpoint_label: Option<String>,
        /// Save-on-cancel attempted and failed. Owner keeps `Canceled` + Warning.
        save_failed: bool,
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
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(WorkerRequest::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn try_progress(progress_tx: &SyncSender<WorkerEvent>, event: WorkerEvent) -> bool {
    match progress_tx.try_send(event) {
        Ok(()) => true,
        Err(TrySendError::Full(_)) => false,
        Err(TrySendError::Disconnected(_)) => false,
    }
}

/// Terminal/load results use a separate bounded mailbox. Blocking send is
/// allowed here: the owner drains this before progress and never awaits
/// subscribers while applying a result.
fn forward_result(result_tx: &SyncSender<WorkerEvent>, event: WorkerEvent) {
    let _ = result_tx.send(event);
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
                        finish_canceled(&result_tx, run_id, command_id, 0, &cancel, None, false);
                        return;
                    }
                    match *behavior.lock().expect("behavior") {
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
                                WorkerEvent::RunTerminal {
                                    run_id,
                                    command_id,
                                    outcome: TerminalOutcome::Failed { error: err },
                                    checkpoint_label: None,
                                    save_failed: false,
                                },
                            );
                        }
                        FakeWorkerBehavior::Succeed | FakeWorkerBehavior::CancelSaveFail => {
                            forward_result(
                                &result_tx,
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
                        WorkerEvent::RunTerminal {
                            run_id,
                            command_id,
                            outcome: TerminalOutcome::Failed { error: err },
                            checkpoint_label: None,
                            save_failed: false,
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
                    );
                }));
                if result.is_err() {
                    let err = spirula_core::DomainError::try_new(
                        spirula_core::ErrorCode::Internal,
                        "fake worker panic",
                    )
                    .expect("message");
                    forward_result(
                        &result_tx,
                        WorkerEvent::RunTerminal {
                            run_id,
                            command_id,
                            outcome: TerminalOutcome::Failed { error: err },
                            checkpoint_label: None,
                            save_failed: false,
                        },
                    );
                }
            }
            Err(_) => break,
        }
    }
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
    save_on_cancel: bool,
) {
    let started = Instant::now();
    let mut last_heartbeat = Instant::now()
        .checked_sub(limits.heartbeat_interval)
        .unwrap_or_else(Instant::now);
    let mut step = 0u64;
    let _ = try_progress(
        progress_tx,
        WorkerEvent::Log {
            run_id,
            command_id,
            message: format!("probe run start steps={steps}"),
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
            let save_failed = save_on_cancel
                && matches!(
                    *behavior.lock().expect("behavior"),
                    FakeWorkerBehavior::CancelSaveFail
                );
            let label = (save_on_cancel && !save_failed).then(|| format!("probe-stop-{step}"));
            finish_canceled(
                result_tx,
                run_id,
                command_id,
                step,
                &cancel,
                label,
                save_failed,
            );
            return;
        }

        step += 1;
        publish_progress(
            progress_tx,
            run_id,
            command_id,
            step,
            steps,
            started.elapsed(),
        );
        last_heartbeat = Instant::now();
    }

    publish_progress(
        progress_tx,
        run_id,
        command_id,
        step,
        steps,
        started.elapsed(),
    );

    match *behavior.lock().expect("behavior") {
        FakeWorkerBehavior::Panic => panic!("injected fake worker panic on run"),
        FakeWorkerBehavior::Fail => {
            let err = spirula_core::DomainError::try_new(
                spirula_core::ErrorCode::Internal,
                "injected fake run failure",
            )
            .expect("message");
            forward_result(
                result_tx,
                WorkerEvent::RunTerminal {
                    run_id,
                    command_id,
                    outcome: TerminalOutcome::Failed { error: err },
                    checkpoint_label: None,
                    save_failed: false,
                },
            );
        }
        FakeWorkerBehavior::Succeed | FakeWorkerBehavior::CancelSaveFail => {
            forward_result(
                result_tx,
                WorkerEvent::RunTerminal {
                    run_id,
                    command_id,
                    outcome: TerminalOutcome::Completed { step },
                    checkpoint_label: None,
                    save_failed: false,
                },
            );
        }
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
        let paused = pause.is_some_and(|p| *p.lock().expect("pause"));
        let stalled = stall.is_some_and(|s| *s.lock().expect("stall"));
        if last_heartbeat.elapsed() >= limits.heartbeat_interval {
            publish_progress(
                progress_tx,
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

fn finish_canceled(
    result_tx: &SyncSender<WorkerEvent>,
    run_id: RunId,
    command_id: CommandId,
    step: u64,
    cancel: &CancelToken,
    checkpoint_label: Option<String>,
    save_failed: bool,
) {
    let reason = cancel.reason().unwrap_or(CancelReason::User);
    let saved = if save_failed {
        None
    } else {
        checkpoint_label.clone()
    };
    forward_result(
        result_tx,
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
        },
    );
}

fn publish_progress(
    progress_tx: &SyncSender<WorkerEvent>,
    run_id: RunId,
    command_id: CommandId,
    step: u64,
    steps: u64,
    elapsed: Duration,
) {
    let snapshot = ProgressSnapshot::try_new(
        step,
        Some(steps.max(step)),
        0,
        elapsed.as_millis() as u64,
        None,
        Vec::new(),
    )
    .expect("valid progress");
    let _ = try_progress(
        progress_tx,
        WorkerEvent::Progress {
            run_id,
            command_id,
            snapshot,
        },
    );
}
