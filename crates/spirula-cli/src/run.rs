//! Session run loop: submits through ApplicationService only.
//!
//! Presentation rule: exactly one stderr `error:` line for a failing process.
//! Stdout uses a bounded async sink so a full pipe cannot block SIGINT/Stop
//! admission. Cancel intent is sticky across phase boundaries: a rejected
//! Stop (e.g. Ready between load and run) does not clear intent, the next
//! stage is not started, and Stop is retried once a probe is active.

use crate::args::CliAction;
use crate::present::{self, OUTPUT_QUEUE_CAPACITY, OutputSink, capabilities_text, help_text};
use crate::signals;
use crate::usage_error;
use spirula_application::{
    ApplicationService, Command, CommandId, CommandPayload, CommandResult, DiskLogConfig,
    DiskLogLimits, Event, EventCursor, EventEnvelope, FakeWorkerBehavior, ReplayOutcome,
    RunOutcome, SessionLimits, SubmitOutcome,
};
use spirula_core::{DomainError, EXIT_CANCELED, EXIT_SUCCESS, ErrorCode, ProcessStage};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

/// Unified live / query / drain presentation: one render per event seq, and
/// at most one `Completed` line per command id.
struct PresentTracker {
    last_seq: u64,
    completed_cmds: HashSet<u64>,
    session_id: spirula_application::SessionId,
}

impl PresentTracker {
    fn new(session_id: spirula_application::SessionId) -> Self {
        Self {
            last_seq: 0,
            completed_cmds: HashSet::new(),
            session_id,
        }
    }

    fn present(&mut self, out: &OutputSink, envelope: &EventEnvelope) -> bool {
        let behind = envelope.seq <= self.last_seq;
        if let Event::Completed { .. } = &envelope.event
            && let Some(id) = envelope.command_id
        {
            if !self.completed_cmds.insert(id.get()) {
                self.last_seq = self.last_seq.max(envelope.seq);
                return false;
            }
            // First terminal for this command: skip render if seq is stale
            // relative to already-presented progress (wait finishes via
            // saw_completed); otherwise render once.
            if behind {
                return false;
            }
        } else if behind {
            return false;
        }
        self.last_seq = envelope.seq;
        let _ = out.try_write_event(envelope);
        true
    }

    fn saw_completed(&self, id: CommandId) -> bool {
        self.completed_cmds.contains(&id.get())
    }

    /// Replay retained control history after `last_seq` so recovery stays ordered.
    fn catch_up(&mut self, app: &ApplicationService, out: &OutputSink) {
        let cursor = EventCursor::new(self.session_id, self.last_seq);
        let Ok(replay) = app.replay(cursor) else {
            return;
        };
        match replay {
            ReplayOutcome::ControlEvents(events) => {
                for env in events {
                    let _ = self.present(out, &env);
                }
            }
            ReplayOutcome::ResyncRequired {
                retained_terminals, ..
            } => {
                for env in retained_terminals {
                    let _ = self.present(out, &env);
                }
            }
        }
    }
}

pub enum CliExit {
    Code(u8),
    Error {
        error: DomainError,
        stage: ProcessStage,
    },
}

pub fn run_action(action: CliAction) -> CliExit {
    match action {
        CliAction::Help => {
            present::write_stdout(&help_text());
            CliExit::Code(EXIT_SUCCESS)
        }
        CliAction::Capabilities => {
            present::write_stdout(&capabilities_text());
            CliExit::Code(EXIT_SUCCESS)
        }
        CliAction::ProbeLoad { label, log_dir } => run_session(SessionPlan {
            load_label: Some(label),
            dataset: None,
            steps: None,
            log_dir,
            step_delay_ms: None,
            save_on_stop: true,
        }),
        CliAction::ProbeRun {
            steps,
            load_label,
            log_dir,
            step_delay_ms,
            save_on_stop,
        } => run_session(SessionPlan {
            load_label,
            dataset: None,
            steps: Some(steps),
            log_dir,
            step_delay_ms,
            save_on_stop,
        }),
        CliAction::Probe {
            label,
            steps,
            log_dir,
            step_delay_ms,
            save_on_stop,
        } => run_session(SessionPlan {
            load_label: Some(label),
            dataset: None,
            steps,
            log_dir,
            step_delay_ms,
            save_on_stop,
        }),
        CliAction::LoadDataset {
            path,
            recon_dir,
            eval_mode,
            split,
            log_dir,
        } => run_session(SessionPlan {
            load_label: None,
            dataset: Some(DatasetPlan {
                path,
                recon_dir: recon_dir.unwrap_or_default(),
                eval_mode,
                split,
            }),
            steps: None,
            log_dir,
            step_delay_ms: None,
            save_on_stop: true,
        }),
    }
}

struct DatasetPlan {
    path: PathBuf,
    recon_dir: String,
    eval_mode: String,
    split: String,
}

struct SessionPlan {
    load_label: Option<String>,
    dataset: Option<DatasetPlan>,
    steps: Option<u64>,
    log_dir: Option<PathBuf>,
    step_delay_ms: Option<u64>,
    save_on_stop: bool,
}

/// Sticky process-cancel intent vs a successfully admitted Stop.
struct CancelGate {
    /// User asked to cancel (SIGINT). Never cleared until process exit.
    intent: bool,
    /// An admitted Stop is outstanding or completed for this intent wave.
    stop_admitted: bool,
    /// After a Rejected Stop (Ready/Idle), do not resubmit until
    /// [`Self::arm_retry_for_active`] — avoids spam and honors
    /// already-complete / invalid-state without losing intent.
    suppress_retry: bool,
    save_on_stop: bool,
}

impl CancelGate {
    fn new(save_on_stop: bool) -> Self {
        Self {
            intent: false,
            stop_admitted: false,
            suppress_retry: false,
            save_on_stop,
        }
    }

    fn observe_signals(&mut self) {
        if signals::stop_requested() {
            self.intent = true;
        }
    }

    /// Allow one more Stop attempt once a probe is active (or about to be).
    fn arm_retry_for_active(&mut self) {
        self.suppress_retry = false;
    }

    /// Submit Stop when intent is set and no Stop has been admitted yet.
    /// Rejected Stop (invalid in Ready/Idle) keeps `intent`, leaves
    /// `stop_admitted` false, and suppresses further attempts until armed
    /// for an active stage.
    fn try_admit_stop(&mut self, app: &ApplicationService, out: &OutputSink) {
        self.observe_signals();
        if !self.intent || self.stop_admitted || self.suppress_retry {
            return;
        }
        match app.submit(Command::new(CommandPayload::Stop {
            save: self.save_on_stop,
        })) {
            SubmitOutcome::Accepted { .. } => {
                self.stop_admitted = true;
            }
            SubmitOutcome::Rejected { error } => {
                self.suppress_retry = true;
                let _ = out.try_write_line(format!(
                    "rejected Stop {{ save: {} }}: {error}\n",
                    self.save_on_stop
                ));
            }
        }
    }
}

fn run_session(plan: SessionPlan) -> CliExit {
    signals::install();
    let out = OutputSink::start(OUTPUT_QUEUE_CAPACITY);

    let mut limits = SessionLimits::default();
    if let Some(ms) = plan.step_delay_ms {
        limits.worker_step_delay = Duration::from_millis(ms.max(1));
        limits.cancel_poll_interval = Duration::from_millis(ms.clamp(1, 50));
    }
    // Test-only: force notify/progress backpressure so live terminals can drop
    // and the CLI must recover via command_result / replay.
    if std::env::var_os("SPIRULA_TEST_TINY_NOTIFY").is_some() {
        limits.notification_queue_capacity = 1;
        limits.worker_progress_queue_capacity = 1;
    }

    let disk = plan.log_dir.as_ref().map(|dir| {
        let mut cfg = DiskLogConfig::new(dir);
        if std::env::var_os("SPIRULA_TEST_DISK_LIMITS").is_some() {
            cfg = cfg.with_limits(DiskLogLimits::for_test());
        }
        cfg
    });

    let app = match ApplicationService::start_with(limits, disk) {
        Ok(app) => app,
        Err(error) => {
            let _ = out.shutdown(Duration::from_millis(100));
            return CliExit::Error {
                error,
                stage: ProcessStage::BeforeSession,
            };
        }
    };
    apply_test_worker_behavior(&app);

    let (sub, snap) = match app.subscribe() {
        Ok(v) => v,
        Err(_) => {
            let _ = out.shutdown(Duration::from_millis(100));
            return CliExit::Error {
                error: usage_error(ErrorCode::Internal, "failed to subscribe to session events"),
                stage: ProcessStage::InSession,
            };
        }
    };

    let mut cancel = CancelGate::new(plan.save_on_stop);
    let mut saw_cancel_exit = false;
    let mut present = PresentTracker::new(snap.session_id);

    if let Some(ds) = &plan.dataset {
        cancel.try_admit_stop(&app, &out);
        if cancel.intent {
            return abort_canceled(&app, &sub, &out, &mut cancel, &mut present, saw_cancel_exit);
        }
        let id = match app.submit(Command::new(CommandPayload::LoadDataset {
            path: ds.path.display().to_string(),
            recon_dir: ds.recon_dir.clone(),
            eval_mode: ds.eval_mode.clone(),
            split: ds.split.clone(),
        })) {
            SubmitOutcome::Accepted { id } => id,
            SubmitOutcome::Rejected { error } => {
                let _ = out.shutdown(Duration::from_millis(200));
                return CliExit::Error {
                    error,
                    stage: ProcessStage::InSession,
                };
            }
        };
        cancel.try_admit_stop(&app, &out);
        match wait_for_command(
            &app,
            &sub,
            &out,
            id,
            &mut cancel,
            &mut present,
            &mut saw_cancel_exit,
        ) {
            WaitEnd::Exit(exit) => {
                return finalize_wait(&app, &out, WaitEnd::Exit(exit), saw_cancel_exit);
            }
            WaitEnd::Success => {
                return finish_idle_success(
                    &app,
                    &sub,
                    &out,
                    &mut cancel,
                    &mut present,
                    saw_cancel_exit,
                );
            }
        }
    }

    if let Some(label) = &plan.load_label {
        cancel.try_admit_stop(&app, &out);
        if cancel.intent {
            return abort_canceled(&app, &sub, &out, &mut cancel, &mut present, saw_cancel_exit);
        }
        let id = match app.submit(Command::new(CommandPayload::ProbeLoad {
            label: label.clone(),
        })) {
            SubmitOutcome::Accepted { id } => id,
            SubmitOutcome::Rejected { error } => {
                let _ = out.shutdown(Duration::from_millis(200));
                return CliExit::Error {
                    error,
                    stage: ProcessStage::InSession,
                };
            }
        };
        cancel.try_admit_stop(&app, &out);
        match wait_for_command(
            &app,
            &sub,
            &out,
            id,
            &mut cancel,
            &mut present,
            &mut saw_cancel_exit,
        ) {
            WaitEnd::Exit(exit) => {
                return finalize_wait(&app, &out, WaitEnd::Exit(exit), saw_cancel_exit);
            }
            WaitEnd::Success if plan.steps.is_some() => {}
            WaitEnd::Success => {
                return finish_idle_success(
                    &app,
                    &sub,
                    &out,
                    &mut cancel,
                    &mut present,
                    saw_cancel_exit,
                );
            }
        }
    }

    cancel.observe_signals();
    cancel.try_admit_stop(&app, &out);
    if cancel.intent {
        return abort_canceled(&app, &sub, &out, &mut cancel, &mut present, saw_cancel_exit);
    }

    if let Ok(raw) = std::env::var("SPIRULA_TEST_GAP_AFTER_LOAD_MS")
        && let Ok(ms) = raw.parse::<u64>()
    {
        let end = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < end {
            cancel.try_admit_stop(&app, &out);
            if cancel.intent {
                return abort_canceled(
                    &app,
                    &sub,
                    &out,
                    &mut cancel,
                    &mut present,
                    saw_cancel_exit,
                );
            }
            while let Some(env) = sub.try_recv() {
                let _ = present.present(&out, &env);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        cancel.try_admit_stop(&app, &out);
        if cancel.intent {
            return abort_canceled(&app, &sub, &out, &mut cancel, &mut present, saw_cancel_exit);
        }
    }

    if let Some(steps) = plan.steps {
        let id = match app.submit(Command::new(CommandPayload::ProbeRun { steps })) {
            SubmitOutcome::Accepted { id } => id,
            SubmitOutcome::Rejected { error } => {
                let _ = out.shutdown(Duration::from_millis(200));
                return CliExit::Error {
                    error,
                    stage: ProcessStage::InSession,
                };
            }
        };
        cancel.arm_retry_for_active();
        cancel.try_admit_stop(&app, &out);
        let end = wait_for_command(
            &app,
            &sub,
            &out,
            id,
            &mut cancel,
            &mut present,
            &mut saw_cancel_exit,
        );
        let _ = app.submit(Command::new(CommandPayload::Shutdown));
        drain_until_shutdown(&app, &sub, &out, &mut cancel, &mut present);
        return finalize_wait(&app, &out, end, saw_cancel_exit || cancel.intent);
    }

    finish_idle_success(&app, &sub, &out, &mut cancel, &mut present, saw_cancel_exit)
}

fn finish_idle_success(
    app: &ApplicationService,
    sub: &spirula_application::Subscription,
    out: &OutputSink,
    cancel: &mut CancelGate,
    present: &mut PresentTracker,
    saw_cancel_exit: bool,
) -> CliExit {
    cancel.observe_signals();
    if cancel.intent {
        return abort_canceled(app, sub, out, cancel, present, saw_cancel_exit);
    }
    let _ = app.submit(Command::new(CommandPayload::Shutdown));
    drain_until_shutdown(app, sub, out, cancel, present);
    finalize_code(app, out, EXIT_SUCCESS, saw_cancel_exit || cancel.intent)
}

fn abort_canceled(
    app: &ApplicationService,
    sub: &spirula_application::Subscription,
    out: &OutputSink,
    cancel: &mut CancelGate,
    present: &mut PresentTracker,
    saw_cancel_exit: bool,
) -> CliExit {
    cancel.try_admit_stop(app, out);
    let _ = app.submit(Command::new(CommandPayload::Shutdown));
    cancel.suppress_retry = true;
    drain_until_shutdown(app, sub, out, cancel, present);
    finalize_code(app, out, EXIT_CANCELED, true || saw_cancel_exit)
}

enum WaitEnd {
    Success,
    Exit(WaitExit),
}

enum WaitExit {
    Code(u8),
    Error(DomainError),
}

fn apply_test_worker_behavior(app: &ApplicationService) {
    let Ok(raw) = std::env::var("SPIRULA_TEST_WORKER_BEHAVIOR") else {
        return;
    };
    let behavior = match raw.as_str() {
        "fail" => FakeWorkerBehavior::Fail,
        "panic" => FakeWorkerBehavior::Panic,
        "cancel_save_fail" => FakeWorkerBehavior::CancelSaveFail,
        _ => FakeWorkerBehavior::Succeed,
    };
    app.set_worker_behavior(behavior);
}

fn classify_outcome(outcome: &RunOutcome, saw_cancel_exit: &mut bool) -> WaitEnd {
    match outcome {
        RunOutcome::Completed { .. } => WaitEnd::Success,
        RunOutcome::Canceled { .. } => {
            *saw_cancel_exit = true;
            WaitEnd::Exit(WaitExit::Code(EXIT_CANCELED))
        }
        RunOutcome::Failed { error } => WaitEnd::Exit(WaitExit::Error(error.without_source())),
    }
}

fn wait_for_command(
    app: &ApplicationService,
    sub: &spirula_application::Subscription,
    out: &OutputSink,
    id: CommandId,
    cancel: &mut CancelGate,
    present: &mut PresentTracker,
    saw_cancel_exit: &mut bool,
) -> WaitEnd {
    let pressure = std::env::var_os("SPIRULA_TEST_TINY_NOTIFY").is_some();
    loop {
        cancel.try_admit_stop(app, out);

        if pressure {
            std::thread::sleep(Duration::from_millis(5));
            if let Some(envelope) = sub.try_recv() {
                if let Event::CommandRejected { command, .. } = &envelope.event
                    && matches!(command, CommandPayload::Stop { .. })
                {
                    cancel.stop_admitted = false;
                    cancel.suppress_retry = true;
                }
                // Under pressure, skip live Completed so query/replay proves recovery.
                if !(matches!(envelope.event, Event::Completed { .. })
                    && envelope.command_id == Some(id))
                {
                    let _ = present.present(out, &envelope);
                }
            }
        } else {
            match sub.recv_timeout(Duration::from_millis(20)) {
                Ok(envelope) => {
                    let _ = present.present(out, &envelope);
                    if let Event::CommandRejected { command, .. } = &envelope.event
                        && matches!(command, CommandPayload::Stop { .. })
                    {
                        cancel.stop_admitted = false;
                        cancel.suppress_retry = true;
                    }
                    if let Event::Completed { outcome } = &envelope.event
                        && envelope.command_id == Some(id)
                        && present.saw_completed(id)
                    {
                        return classify_outcome(outcome, saw_cancel_exit);
                    }
                }
                Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => {}
            }
        }

        match app.command_result(id) {
            CommandResult::Ready(envelope) => {
                if let Event::Completed { outcome } = &envelope.event {
                    if present.saw_completed(id) {
                        // Live path already rendered this terminal once.
                        return classify_outcome(outcome, saw_cancel_exit);
                    }
                    if pressure {
                        let drops = app.snapshot().drops.notification;
                        let _ = out.try_write_line(format!("spirula:test notify-drops={drops}\n"));
                        if drops == 0 {
                            continue;
                        }
                    }
                    // Ordered catch-up from retained control history first.
                    present.catch_up(app, out);
                    if !present.saw_completed(id) {
                        let _ = present.present(out, &envelope);
                        let _ =
                            out.try_write_line("spirula:recovered command terminal via query\n");
                    } else if pressure {
                        let _ =
                            out.try_write_line("spirula:recovered command terminal via query\n");
                    }
                    return classify_outcome(outcome, saw_cancel_exit);
                }
                if matches!(envelope.event, Event::ShutdownComplete) {
                    let _ = present.present(out, &envelope);
                    return WaitEnd::Success;
                }
            }
            CommandResult::Expired => {
                return WaitEnd::Exit(WaitExit::Error(usage_error(
                    ErrorCode::InvalidState,
                    "command result expired before terminal was observed",
                )));
            }
            CommandResult::Pending | CommandResult::Unknown => {}
        }

        if app.snapshot().state == spirula_application::SessionState::Closed {
            return WaitEnd::Success;
        }
    }
}

fn drain_until_shutdown(
    app: &ApplicationService,
    sub: &spirula_application::Subscription,
    out: &OutputSink,
    cancel: &mut CancelGate,
    present: &mut PresentTracker,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        cancel.observe_signals();
        cancel.try_admit_stop(app, out);
        match sub.recv_timeout(Duration::from_millis(50)) {
            Ok(envelope) => {
                let _ = present.present(out, &envelope);
                if matches!(envelope.event, Event::ShutdownComplete) {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                present.catch_up(app, out);
                if app.snapshot().state == spirula_application::SessionState::Closed {
                    return;
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn finalize_wait(
    app: &ApplicationService,
    out: &OutputSink,
    end: WaitEnd,
    saw_cancel: bool,
) -> CliExit {
    let _ = app.flush_disk_log(Duration::from_millis(500));
    let _ = out.flush(Duration::from_millis(200));
    let _ = out.shutdown(Duration::from_millis(200));
    let cancel = saw_cancel || signals::stop_requested();
    match end {
        WaitEnd::Success => finalize_code_bare(EXIT_SUCCESS, cancel),
        WaitEnd::Exit(WaitExit::Code(code)) => finalize_code_bare(code, cancel),
        WaitEnd::Exit(WaitExit::Error(error)) => CliExit::Error {
            error,
            stage: ProcessStage::InSession,
        },
    }
}

fn finalize_code(
    app: &ApplicationService,
    out: &OutputSink,
    code: u8,
    saw_cancel: bool,
) -> CliExit {
    let _ = app.flush_disk_log(Duration::from_millis(200));
    let _ = out.flush(Duration::from_millis(200));
    let _ = out.shutdown(Duration::from_millis(200));
    finalize_code_bare(code, saw_cancel || signals::stop_requested())
}

fn finalize_code_bare(code: u8, saw_cancel: bool) -> CliExit {
    if saw_cancel && (code == EXIT_SUCCESS || code == EXIT_CANCELED) {
        return CliExit::Code(EXIT_CANCELED);
    }
    CliExit::Code(code)
}
