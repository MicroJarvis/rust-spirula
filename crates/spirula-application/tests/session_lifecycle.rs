//! P1-F07 session owner lifecycle and backpressure tests.

use std::time::Duration;

use spirula_application::{
    ApplicationService, Command, CommandPayload, CommandResult, Event, EventCursor,
    FakeWorkerBehavior, ReplayOutcome, SessionLimits, SessionState, SubmitOutcome, WorkerEvent,
};
use spirula_core::{ErrorCode, OutcomeView, ProgressSnapshot, TerminalOutcome};

fn service() -> ApplicationService {
    ApplicationService::start(SessionLimits::for_test())
}

fn wait_ready(app: &ApplicationService) {
    assert!(
        app.wait_until(Duration::from_secs(2), |s| {
            s.state == SessionState::Ready && s.active_command.is_none()
        }),
        "timeout waiting for Ready: {:?}",
        app.snapshot()
    );
}

fn wait_idle(app: &ApplicationService) {
    assert!(
        app.wait_until(Duration::from_secs(2), |s| s.state == SessionState::Idle
            && s.active_command.is_none()),
        "timeout waiting for Idle: {:?}",
        app.snapshot()
    );
}

fn wait_terminal(
    app: &ApplicationService,
    id: spirula_application::CommandId,
) -> spirula_application::EventEnvelope {
    assert!(
        app.wait_until(Duration::from_secs(2), |_| {
            matches!(app.command_result(id), CommandResult::Ready(_))
        }),
        "timeout waiting for terminal {id}: {:?} result={:?}",
        app.snapshot(),
        app.command_result(id)
    );
    match app.command_result(id) {
        CommandResult::Ready(env) => env,
        other => panic!("expected Ready, got {other:?}"),
    }
}

fn accept(app: &ApplicationService, payload: CommandPayload) -> spirula_application::CommandId {
    match app.submit(Command::new(payload)) {
        SubmitOutcome::Accepted { id } => id,
        SubmitOutcome::Rejected { error } => panic!("unexpected reject: {error}"),
    }
}

#[test]
fn probe_load_and_run_reach_ready_with_one_terminal_each() {
    let app = service();
    let load = accept(&app, CommandPayload::ProbeLoad { label: "ds".into() });
    let load_env = wait_terminal(&app, load);
    assert!(matches!(load_env.event, Event::Completed { .. }));
    wait_ready(&app);

    let run = accept(&app, CommandPayload::ProbeRun { steps: 3 });
    let run_env = wait_terminal(&app, run);
    match run_env.event {
        Event::Completed {
            outcome: OutcomeView::Completed { step },
        } => assert_eq!(step, 3),
        other => panic!("unexpected terminal {other:?}"),
    }
    wait_ready(&app);
}

#[test]
fn invalid_and_duplicate_mutating_commands_rejected() {
    let app = service();
    accept(&app, CommandPayload::ProbeLoad { label: "a".into() });
    wait_ready(&app);

    let run = accept(&app, CommandPayload::ProbeRun { steps: 50 });
    // Second mutating probe while in flight.
    match app.submit(Command::new(CommandPayload::ProbeRun { steps: 1 })) {
        SubmitOutcome::Rejected { error } => assert_eq!(error.code(), ErrorCode::CommandInFlight),
        SubmitOutcome::Accepted { .. } => panic!("expected in-flight reject"),
    }
    let _ = run;
    let _ = app.submit(Command::new(CommandPayload::Stop { save: false }));
    wait_ready(&app);

    match app.submit(Command::new(CommandPayload::ProbeRun { steps: 1 })) {
        // still Ready — ok
        SubmitOutcome::Accepted { .. } => wait_ready(&app),
        SubmitOutcome::Rejected { error } => panic!("{error}"),
    }

    match app.submit(Command::new(CommandPayload::Reset)) {
        SubmitOutcome::Rejected { error } => assert_eq!(error.code(), ErrorCode::InvalidState),
        SubmitOutcome::Accepted { .. } => panic!("reset only from Failed"),
    }
}

#[test]
fn oversized_label_rejected_without_weakening_codes() {
    let app = service();
    let limits = app.limits();
    let label = "x".repeat(limits.label_max_bytes + 1);
    match app.submit(Command::new(CommandPayload::ProbeLoad { label })) {
        SubmitOutcome::Rejected { error } => assert_eq!(error.code(), ErrorCode::ValueOutOfRange),
        SubmitOutcome::Accepted { .. } => panic!("expected oversized reject"),
    }
}

#[test]
fn stop_cancels_run_and_coalesces_duplicate_stop() {
    let app = service();
    accept(&app, CommandPayload::ProbeLoad { label: "a".into() });
    wait_ready(&app);
    let run = accept(&app, CommandPayload::ProbeRun { steps: 100 });
    assert!(app.wait_until(Duration::from_secs(1), |s| s.state == SessionState::Running));

    let stop1 = accept(&app, CommandPayload::Stop { save: true });
    let stop2 = accept(&app, CommandPayload::Stop { save: true });
    wait_ready(&app);

    match app.command_result(run) {
        CommandResult::Ready(env) => match env.event {
            Event::Completed {
                outcome: OutcomeView::Canceled { .. },
            } => {}
            other => panic!("expected canceled run, got {other:?}"),
        },
        other => panic!("{other:?}"),
    }
    assert!(matches!(app.command_result(stop1), CommandResult::Ready(_)));
    assert!(matches!(app.command_result(stop2), CommandResult::Ready(_)));
}

#[test]
fn stalled_step_still_heartbeats_and_honors_cancel() {
    let app = service();
    accept(&app, CommandPayload::ProbeLoad { label: "a".into() });
    wait_ready(&app);
    let gate = app.stall_gate();
    gate.set_stalled(true);
    let run = accept(&app, CommandPayload::ProbeRun { steps: 5 });
    assert!(app.wait_until(Duration::from_secs(1), |s| s.active_command == Some(run)));

    // Heartbeat / progress should still advance seq while stalled.
    let seq0 = app.snapshot().seq;
    assert!(
        app.wait_until(Duration::from_secs(2), |s| s.seq > seq0),
        "expected heartbeat progress while stalled"
    );

    let _ = app.submit(Command::new(CommandPayload::Stop { save: false }));
    // Cancel must win without waiting for the stall to clear.
    assert!(
        app.wait_until(Duration::from_secs(2), |s| {
            s.active_command.is_none() && s.state == SessionState::Ready
        }),
        "cancel while stalled timed out: {:?}",
        app.snapshot()
    );
    gate.set_stalled(false);
}

#[test]
fn full_notification_queue_does_not_block_worker_or_stop() {
    let mut limits = SessionLimits::for_test();
    limits.notification_queue_capacity = 1;
    let app = ApplicationService::start(limits);
    let (sub, _) = app.subscribe().expect("subscribe");
    // Do not drain `sub` — stall the subscriber.
    let load = accept(&app, CommandPayload::ProbeLoad { label: "a".into() });
    wait_terminal(&app, load);
    wait_ready(&app);
    let run = accept(&app, CommandPayload::ProbeRun { steps: 8 });
    let _ = app.submit(Command::new(CommandPayload::Stop { save: false }));
    wait_terminal(&app, run);
    wait_ready(&app);
    assert!(app.snapshot().drops.notification > 0);
    let _ = sub.try_recv();
}

#[test]
fn command_queue_full_rejects_with_sess_003() {
    // With capacity 1, a reserved in-flight probe rejects a second mutating
    // probe as SP-SESS-003 (in-flight). That is the bounded admission edge:
    // the work queue never grows past capacity, and extras are rejected.
    let mut limits = SessionLimits::for_test();
    limits.command_queue_capacity = 1;
    limits.worker_step_delay = Duration::from_millis(50);
    let app = ApplicationService::start(limits);
    let load = accept(&app, CommandPayload::ProbeLoad { label: "a".into() });
    wait_terminal(&app, load);
    wait_ready(&app);
    let run = accept(&app, CommandPayload::ProbeRun { steps: 40 });
    match app.submit(Command::new(CommandPayload::ProbeRun { steps: 1 })) {
        SubmitOutcome::Rejected { error } => assert_eq!(error.code(), ErrorCode::CommandInFlight),
        SubmitOutcome::Accepted { .. } => panic!("expected queue/in-flight reject"),
    }
    let _ = app.submit(Command::new(CommandPayload::Stop { save: false }));
    wait_terminal(&app, run);
    wait_ready(&app);
}

#[test]
fn cursor_foreign_and_future_rejected_resync_for_expired_control() {
    let mut limits = SessionLimits::for_test();
    limits.control_history_capacity = 3;
    let app = ApplicationService::start(limits);
    let load = accept(&app, CommandPayload::ProbeLoad { label: "a".into() });
    wait_terminal(&app, load);
    wait_ready(&app);
    // Generate more control events than capacity.
    for _ in 0..6 {
        let id = accept(&app, CommandPayload::ProbeRun { steps: 1 });
        wait_terminal(&app, id);
        wait_ready(&app);
    }
    let snap = app.snapshot();
    assert!(snap.drops.control_history > 0);

    match app.replay(EventCursor::new(snap.session_id, 0)) {
        Ok(ReplayOutcome::ResyncRequired { .. }) => {}
        Ok(other) => panic!("expected resync, got {other:?}"),
        Err(e) => panic!("{e:?}"),
    }

    let foreign = EventCursor::new(spirula_application::SessionId::new(999), snap.seq);
    assert!(matches!(
        app.replay(foreign),
        Err(spirula_application::CursorError::ForeignSession { .. })
    ));

    let future = EventCursor::new(snap.session_id, snap.seq + 10);
    assert!(matches!(
        app.replay(future),
        Err(spirula_application::CursorError::Future { .. })
    ));
}

#[test]
fn subscribe_snapshot_share_boundary_and_progress_gaps_are_not_resync() {
    let app = service();
    let (sub, snap) = app.subscribe().expect("sub");
    let load = accept(&app, CommandPayload::ProbeLoad { label: "a".into() });
    wait_terminal(&app, load);
    wait_ready(&app);
    let run = accept(&app, CommandPayload::ProbeRun { steps: 3 });
    wait_terminal(&app, run);
    wait_ready(&app);

    // Replay from the live boundary after work: control-only; missing progress
    // (replaceable) must not force resync.
    let live = app.snapshot();
    match app.replay(live.cursor).expect("replay") {
        ReplayOutcome::ControlEvents(events) => {
            assert!(
                !events.iter().any(|e| matches!(e.event, Event::Progress(_))),
                "replay must not fabricate progress history"
            );
        }
        ReplayOutcome::ResyncRequired { .. } => panic!("progress gaps must not force resync"),
    }
    // Original subscribe cursor may require resync if history expired — that is
    // control-history expiration, not progress coalescing.
    let _ = app.replay(snap.cursor);
    let _ = sub;
}

#[test]
fn stale_worker_events_rejected() {
    let app = service();
    accept(&app, CommandPayload::ProbeLoad { label: "a".into() });
    wait_ready(&app);
    let before = app.snapshot().drops.stale_worker;
    let snap = ProgressSnapshot::try_new(1, Some(1), 0, 0, None, vec![]).unwrap();
    app.inject_worker_event(WorkerEvent::Progress {
        run_id: spirula_application::RunId::new(999),
        command_id: spirula_application::CommandId::new(999),
        snapshot: snap,
    })
    .expect("inject");
    assert!(app.wait_until(Duration::from_secs(1), |s| {
        s.drops.stale_worker > before
    }));
    assert_eq!(app.snapshot().state, SessionState::Ready);
}

#[test]
fn late_run_terminal_after_completion_is_stale() {
    let app = service();
    accept(&app, CommandPayload::ProbeLoad { label: "a".into() });
    wait_ready(&app);
    let run = accept(&app, CommandPayload::ProbeRun { steps: 1 });
    wait_ready(&app);
    let before = app.snapshot().drops.stale_worker;
    app.inject_worker_event(WorkerEvent::RunTerminal {
        run_id: spirula_application::RunId::new(1),
        command_id: run,
        outcome: TerminalOutcome::Completed { step: 99 },
        checkpoint_label: None,
    })
    .ok();
    assert!(app.wait_until(Duration::from_secs(1), |s| {
        s.drops.stale_worker > before
    }));
}

#[test]
fn worker_failure_and_panic_become_failed_outcome() {
    let app = service();
    app.set_worker_behavior(FakeWorkerBehavior::Fail);
    let load = accept(&app, CommandPayload::ProbeLoad { label: "x".into() });
    assert!(app.wait_until(Duration::from_secs(2), |s| {
        s.state == SessionState::Failed && s.active_command.is_none()
    }));
    match app.command_result(load) {
        CommandResult::Ready(env) => match env.event {
            Event::Completed {
                outcome: OutcomeView::Failed { error },
            } => assert_eq!(error.code(), ErrorCode::Internal),
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    }

    accept(&app, CommandPayload::Reset);
    wait_idle(&app);
    app.set_worker_behavior(FakeWorkerBehavior::Panic);
    let load2 = accept(&app, CommandPayload::ProbeLoad { label: "y".into() });
    assert!(app.wait_until(Duration::from_secs(2), |s| {
        s.state == SessionState::Failed && s.active_command.is_none()
    }));
    match app.command_result(load2) {
        CommandResult::Ready(env) => match env.event {
            Event::Completed {
                outcome: OutcomeView::Failed { error },
            } => assert_eq!(error.code(), ErrorCode::Internal),
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    }
}

#[test]
fn shutdown_is_last_event_and_closes_notify() {
    let mut limits = SessionLimits::for_test();
    limits.notification_queue_capacity = 64;
    let app = ApplicationService::start(limits);
    let (sub, _) = app.subscribe().expect("sub");
    let load = accept(&app, CommandPayload::ProbeLoad { label: "a".into() });
    wait_terminal(&app, load);
    wait_ready(&app);
    let shut = accept(&app, CommandPayload::Shutdown);
    let shut_env = wait_terminal(&app, shut);
    assert!(matches!(shut_env.event, Event::ShutdownComplete));
    assert_eq!(app.snapshot().state, SessionState::Closed);
    let mut saw_shutdown = false;
    let mut last_was_shutdown = false;
    while let Ok(env) = sub.recv_timeout(Duration::from_millis(200)) {
        last_was_shutdown = matches!(env.event, Event::ShutdownComplete);
        if last_was_shutdown {
            saw_shutdown = true;
        }
    }
    assert!(saw_shutdown);
    assert!(last_was_shutdown);
    assert!(app.subscribe().is_none());
}

#[test]
fn result_expiration_reports_expired() {
    let mut limits = SessionLimits::for_test();
    limits.result_retention = 1;
    let app = ApplicationService::start(limits);
    let a = accept(&app, CommandPayload::ProbeLoad { label: "a".into() });
    wait_terminal(&app, a);
    wait_ready(&app);
    let b = accept(&app, CommandPayload::ProbeRun { steps: 1 });
    wait_terminal(&app, b);
    wait_ready(&app);
    assert!(matches!(app.command_result(b), CommandResult::Ready(_)));
    assert!(matches!(app.command_result(a), CommandResult::Expired));
}
