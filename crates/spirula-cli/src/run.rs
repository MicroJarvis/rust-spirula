//! Session run loop: submits through ApplicationService only.

use crate::args::CliAction;
use crate::exit_from_outcome;
use crate::present::{self, capabilities_text, help_text};
use crate::signals;
use crate::usage_error;
use spirula_application::{
    ApplicationService, Command, CommandPayload, CommandResult, DiskLogConfig, DiskLogLimits,
    Event, FakeWorkerBehavior, RunOutcome, SessionLimits, SessionState, SubmitOutcome,
};
use spirula_core::{
    DomainError, EXIT_CANCELED, EXIT_FAILURE, EXIT_SUCCESS, ErrorCode, ProcessStage,
};
use std::path::PathBuf;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

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
            print!("{}", help_text());
            CliExit::Code(EXIT_SUCCESS)
        }
        CliAction::Capabilities => {
            print!("{}", capabilities_text());
            CliExit::Code(EXIT_SUCCESS)
        }
        CliAction::ProbeLoad { label, log_dir } => run_session(SessionPlan {
            load_label: Some(label),
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
            steps,
            log_dir,
            step_delay_ms,
            save_on_stop,
        }),
    }
}

struct SessionPlan {
    load_label: Option<String>,
    steps: Option<u64>,
    log_dir: Option<PathBuf>,
    step_delay_ms: Option<u64>,
    save_on_stop: bool,
}

fn run_session(plan: SessionPlan) -> CliExit {
    signals::install();

    let mut limits = SessionLimits::default();
    if let Some(ms) = plan.step_delay_ms {
        limits.worker_step_delay = Duration::from_millis(ms.max(1));
        limits.cancel_poll_interval = Duration::from_millis(ms.clamp(1, 50));
    }
    // Test hook: inject fake worker behavior without a second state machine.
    if let Ok(raw) = std::env::var("SPIRULA_TEST_WORKER_BEHAVIOR") {
        // Applied after start.
        let _ = raw;
    }

    let disk = plan.log_dir.as_ref().map(|dir| {
        let mut cfg = DiskLogConfig::new(dir);
        if std::env::var_os("SPIRULA_TEST_DISK_LIMITS").is_some() {
            cfg = cfg.with_limits(DiskLogLimits::for_test());
        }
        cfg
    });

    let app = ApplicationService::start_with(limits, disk);
    apply_test_worker_behavior(&app);

    let (sub, _snap) = match app.subscribe() {
        Ok(v) => v,
        Err(_) => {
            return CliExit::Error {
                error: usage_error(ErrorCode::Internal, "failed to subscribe to session events"),
                stage: ProcessStage::InSession,
            };
        }
    };

    let mut stop_submitted = false;
    let mut saw_cancel_exit = false;

    if let Some(label) = &plan.load_label {
        let id = match app.submit(Command::new(CommandPayload::ProbeLoad {
            label: label.clone(),
        })) {
            SubmitOutcome::Accepted { id } => id,
            SubmitOutcome::Rejected { error } => {
                present::write_error(&error);
                return CliExit::Error {
                    error,
                    stage: ProcessStage::InSession,
                };
            }
        };
        match wait_for_command(
            &app,
            &sub,
            id,
            &mut stop_submitted,
            plan.save_on_stop,
            &mut saw_cancel_exit,
        ) {
            WaitEnd::Exit(code) => return finalize(&app, code, saw_cancel_exit),
            WaitEnd::Success if plan.steps.is_some() => {}
            WaitEnd::Success => {
                let _ = app.submit(Command::new(CommandPayload::Shutdown));
                drain_until_shutdown(&app, &sub, &mut stop_submitted, plan.save_on_stop);
                return finalize(&app, EXIT_SUCCESS, saw_cancel_exit);
            }
        }
    }

    if let Some(steps) = plan.steps {
        let id = match app.submit(Command::new(CommandPayload::ProbeRun { steps })) {
            SubmitOutcome::Accepted { id } => id,
            SubmitOutcome::Rejected { error } => {
                present::write_error(&error);
                return CliExit::Error {
                    error,
                    stage: ProcessStage::InSession,
                };
            }
        };
        let code = match wait_for_command(
            &app,
            &sub,
            id,
            &mut stop_submitted,
            plan.save_on_stop,
            &mut saw_cancel_exit,
        ) {
            WaitEnd::Success => EXIT_SUCCESS,
            WaitEnd::Exit(code) => code,
        };
        let _ = app.submit(Command::new(CommandPayload::Shutdown));
        drain_until_shutdown(&app, &sub, &mut stop_submitted, plan.save_on_stop);
        return finalize(&app, code, saw_cancel_exit);
    }

    // No load and no steps (should not parse): shut down.
    let _ = app.submit(Command::new(CommandPayload::Shutdown));
    drain_until_shutdown(&app, &sub, &mut stop_submitted, plan.save_on_stop);
    finalize(&app, EXIT_SUCCESS, saw_cancel_exit)
}

enum WaitEnd {
    /// Mutating command completed successfully; caller may continue.
    Success,
    /// Process should exit with this code (failure or cancel).
    Exit(u8),
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
            WaitEnd::Exit(EXIT_CANCELED)
        }
        RunOutcome::Failed { error } => {
            present::write_error(error);
            WaitEnd::Exit(exit_from_outcome(outcome))
        }
    }
}

fn wait_for_command(
    app: &ApplicationService,
    sub: &spirula_application::Subscription,
    id: spirula_application::CommandId,
    stop_submitted: &mut bool,
    save_on_stop: bool,
    saw_cancel_exit: &mut bool,
) -> WaitEnd {
    loop {
        maybe_handle_sigint(app, stop_submitted, save_on_stop);

        match sub.recv_timeout(Duration::from_millis(50)) {
            Ok(envelope) => {
                present::write_event(&envelope);
                if let Event::Completed { outcome } = &envelope.event
                    && envelope.command_id == Some(id)
                {
                    return classify_outcome(outcome, saw_cancel_exit);
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                // Recover terminal via query even if the live subscriber
                // dropped/coalesced events (slow consumer / disconnect).
                match app.command_result(id) {
                    CommandResult::Ready(envelope) => {
                        if let Event::Completed { outcome } = &envelope.event {
                            eprintln!("spirula:recovered command terminal via query");
                            return classify_outcome(outcome, saw_cancel_exit);
                        }
                        if matches!(envelope.event, Event::ShutdownComplete) {
                            return WaitEnd::Success;
                        }
                    }
                    CommandResult::Expired => {
                        return WaitEnd::Exit(EXIT_FAILURE);
                    }
                    CommandResult::Pending | CommandResult::Unknown => {}
                }
                if app.snapshot().state == SessionState::Closed {
                    return WaitEnd::Success;
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                // Subscriber dropped: fall back to command_result / replay.
                match app.command_result(id) {
                    CommandResult::Ready(envelope) => {
                        if let Event::Completed { outcome } = &envelope.event {
                            eprintln!("spirula:recovered after subscriber disconnect");
                            return classify_outcome(outcome, saw_cancel_exit);
                        }
                        return WaitEnd::Exit(EXIT_FAILURE);
                    }
                    CommandResult::Pending => {
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    _ => return WaitEnd::Exit(EXIT_FAILURE),
                }
            }
        }
    }
}

fn drain_until_shutdown(
    app: &ApplicationService,
    sub: &spirula_application::Subscription,
    stop_submitted: &mut bool,
    save_on_stop: bool,
) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        maybe_handle_sigint(app, stop_submitted, save_on_stop);
        match sub.recv_timeout(Duration::from_millis(50)) {
            Ok(envelope) => {
                present::write_event(&envelope);
                if matches!(envelope.event, Event::ShutdownComplete) {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if app.snapshot().state == SessionState::Closed {
                    return;
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn maybe_handle_sigint(app: &ApplicationService, stop_submitted: &mut bool, save_on_stop: bool) {
    if !*stop_submitted && signals::stop_requested() {
        *stop_submitted = true;
        let _ = app.submit(Command::new(CommandPayload::Stop { save: save_on_stop }));
    }
}

fn finalize(app: &ApplicationService, code: u8, saw_cancel: bool) -> CliExit {
    let _ = app.flush_disk_log(Duration::from_millis(500));
    // SIGINT path: contract exit 130 even if outcome mapping already set it.
    if (saw_cancel || signals::stop_requested()) && code == EXIT_SUCCESS {
        return CliExit::Code(EXIT_CANCELED);
    }
    CliExit::Code(code)
}
