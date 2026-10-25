//! P1-F07 session owner lifecycle and backpressure tests.

use std::time::Duration;

use spirula_application::{
    ApplicationService, Command, CommandPayload, CommandResult, Event, EventCursor,
    FakeWorkerBehavior, ReplayOutcome, SessionLimits, SessionState, SubmitOutcome, WorkerEvent,
};
use spirula_core::{ErrorCode, OutcomeView, ProgressSnapshot, TerminalOutcome, UserAction};

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
        save_failed: false,
        final_progress: None,
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
    assert!(matches!(
        app.subscribe(),
        Err(spirula_application::SubscribeError::Closed)
    ));
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

#[test]
fn max_subscribers_rejects_global_fanout() {
    let mut limits = SessionLimits::for_test();
    limits.max_subscribers = 2;
    let app = ApplicationService::start(limits);
    let _a = app.subscribe().expect("a");
    let _b = app.subscribe().expect("b");
    assert!(matches!(
        app.subscribe(),
        Err(spirula_application::SubscribeError::SubscriberLimit)
    ));
    assert!(app.snapshot().drops.subscribe_rejected > 0);
}

#[test]
fn pause_does_not_overwrite_run_last_outcome() {
    let app = service();
    let load = accept(&app, CommandPayload::ProbeLoad { label: "a".into() });
    wait_terminal(&app, load);
    wait_ready(&app);
    let after_load = app.snapshot().last_outcome.clone();
    assert!(matches!(after_load, Some(OutcomeView::Completed { .. })));
    let run = accept(&app, CommandPayload::ProbeRun { steps: 30 });
    assert!(app.wait_until(Duration::from_secs(1), |s| s.state == SessionState::Running));
    let pause = accept(&app, CommandPayload::Pause);
    wait_terminal(&app, pause);
    // Run still active; pause terminal must not replace the probe last_outcome.
    assert!(matches!(app.command_result(run), CommandResult::Pending));
    assert_eq!(
        format!("{:?}", app.snapshot().last_outcome),
        format!("{after_load:?}")
    );
    let _ = app.submit(Command::new(CommandPayload::Stop { save: false }));
    let run_env = wait_terminal(&app, run);
    match run_env.event {
        Event::Completed {
            outcome: OutcomeView::Canceled { .. },
        } => {}
        other => panic!("expected canceled run, got {other:?}"),
    }
    match &app.snapshot().last_outcome {
        Some(OutcomeView::Canceled { .. }) => {}
        other => panic!("last_outcome must be run cancel, got {other:?}"),
    }
}

#[test]
fn save_on_cancel_failure_keeps_canceled_with_warning() {
    let app = service();
    app.set_worker_behavior(FakeWorkerBehavior::CancelSaveFail);
    let load = accept(&app, CommandPayload::ProbeLoad { label: "a".into() });
    wait_terminal(&app, load);
    wait_ready(&app);
    let run = accept(&app, CommandPayload::ProbeRun { steps: 40 });
    assert!(app.wait_until(Duration::from_secs(1), |s| s.state == SessionState::Running));
    let stop = accept(&app, CommandPayload::Stop { save: true });
    let run_env = wait_terminal(&app, run);
    wait_terminal(&app, stop);
    match run_env.event {
        Event::Completed {
            outcome:
                OutcomeView::Canceled {
                    saved_checkpoint: None,
                    ..
                },
        } => {}
        other => panic!("§5 requires Canceled without new checkpoint, got {other:?}"),
    }
    assert_eq!(app.snapshot().state, SessionState::Ready);
    match &app.snapshot().last_outcome {
        Some(OutcomeView::Canceled { .. }) => {}
        other => panic!("must not become Failed: {other:?}"),
    }
}

#[test]
fn subscribe_and_replay_handoff_keeps_control_events() {
    let mut limits = SessionLimits::for_test();
    limits.notification_queue_capacity = 64;
    limits.control_history_capacity = 64;
    let app = ApplicationService::start(limits);
    let load = accept(
        &app,
        CommandPayload::ProbeLoad {
            label: "pre".into(),
        },
    );
    wait_terminal(&app, load);
    wait_ready(&app);

    let (sub, snap, replay) = app.subscribe_and_replay().expect("handoff");
    match &replay {
        ReplayOutcome::ControlEvents(events) => {
            assert!(events.iter().all(|e| e.seq > snap.seq) || events.is_empty());
        }
        ReplayOutcome::ResyncRequired { .. } => panic!("fresh subscribe should not resync"),
    }

    // Publish more control after handoff registration.
    let run = accept(&app, CommandPayload::ProbeRun { steps: 2 });
    wait_terminal(&app, run);
    wait_ready(&app);

    let mut live_control = Vec::new();
    while let Ok(env) = sub.recv_timeout(Duration::from_millis(20)) {
        if env.event.is_control() {
            live_control.push(env);
        }
    }
    assert!(
        live_control
            .iter()
            .any(|e| matches!(e.event, Event::CommandAccepted { .. })),
        "live handoff must see post-snapshot control; got {live_control:?}"
    );

    // Retained history from the handoff cursor must include the run terminal
    // (no lost control between atomic subscribe and later replay).
    match app.replay(snap.cursor).expect("replay") {
        ReplayOutcome::ControlEvents(events) => {
            assert!(
                events.iter().any(
                    |e| matches!(e.event, Event::Completed { .. }) && e.command_id == Some(run)
                ),
                "retained control after cursor must include run terminal: {events:?}"
            );
        }
        ReplayOutcome::ResyncRequired { .. } => panic!("history should still retain events"),
    }
}

trait EventIsControl {
    fn is_control(&self) -> bool;
}
impl EventIsControl for Event {
    fn is_control(&self) -> bool {
        !matches!(self, Event::Progress(_) | Event::Log { .. })
    }
}

#[test]
fn session_identity_is_unique_per_service() {
    let a = service();
    let b = service();
    assert_ne!(a.session_id(), b.session_id());
}

#[test]
fn concurrent_admission_preserves_all_accepted_terminals() {
    use std::sync::{Arc, Barrier};
    for round in 0..50 {
        let app = Arc::new(service());
        let barrier = Arc::new(Barrier::new(5));
        let mut handles = Vec::new();
        for _ in 0..4 {
            let app = Arc::clone(&app);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                app.submit(Command::new(CommandPayload::ProbeLoad {
                    label: "ds".into(),
                }))
            }));
        }
        barrier.wait();
        let ids: Vec<_> = handles
            .into_iter()
            .filter_map(|h| match h.join().unwrap() {
                SubmitOutcome::Accepted { id } => Some(id),
                _ => None,
            })
            .collect();
        assert!(
            app.wait_until(Duration::from_millis(500), |_| {
                ids.iter()
                    .all(|id| matches!(app.command_result(*id), CommandResult::Ready(_)))
            }),
            "round {round} accepted {ids:?} results {:?}",
            ids.iter()
                .map(|id| app.command_result(*id))
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn queued_shutdown_drains_accepted_work_with_terminals() {
    let mut limits = SessionLimits::for_test();
    limits.command_queue_capacity = 4;
    limits.worker_step_delay = Duration::from_millis(30);
    let app = ApplicationService::start(limits);
    let load = accept(&app, CommandPayload::ProbeLoad { label: "ds".into() });
    wait_terminal(&app, load);
    wait_ready(&app);
    app.stall_gate().set_stalled(true);
    let run = accept(&app, CommandPayload::ProbeRun { steps: 20 });
    assert!(app.wait_until(Duration::from_secs(1), |s| s.state == SessionState::Running));
    let shut = accept(&app, CommandPayload::Shutdown);
    app.stall_gate().set_stalled(false);
    let run_env = wait_terminal(&app, run);
    let shut_env = wait_terminal(&app, shut);
    assert!(matches!(
        run_env.event,
        Event::Completed {
            outcome: OutcomeView::Canceled { .. }
        }
    ));
    assert!(matches!(shut_env.event, Event::ShutdownComplete));
    assert_eq!(app.snapshot().state, SessionState::Closed);
}

#[test]
fn fresh_run_after_stop_does_not_inherit_cancel() {
    let app = service();
    let load = accept(&app, CommandPayload::ProbeLoad { label: "ds".into() });
    wait_terminal(&app, load);
    wait_ready(&app);
    app.stall_gate().set_stalled(true);
    let run = accept(&app, CommandPayload::ProbeRun { steps: 10 });
    assert!(app.wait_until(Duration::from_secs(1), |s| s.state == SessionState::Running));
    let stop = accept(&app, CommandPayload::Stop { save: true });
    wait_terminal(&app, run);
    wait_terminal(&app, stop);
    wait_ready(&app);
    app.stall_gate().set_stalled(false);
    let run2 = accept(&app, CommandPayload::ProbeRun { steps: 1 });
    let env = wait_terminal(&app, run2);
    assert!(
        matches!(
            env.event,
            Event::Completed {
                outcome: OutcomeView::Completed { step: 1 }
            }
        ),
        "{env:?}"
    );
}

#[test]
fn panic_recovery_allows_behavior_reset() {
    let app = service();
    app.set_worker_behavior(FakeWorkerBehavior::Panic);
    let load = accept(
        &app,
        CommandPayload::ProbeLoad {
            label: "panic".into(),
        },
    );
    wait_terminal(&app, load);
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            app.set_worker_behavior(FakeWorkerBehavior::Succeed)
        }))
        .is_ok(),
        "behavior mutex must not stay poisoned"
    );
    accept(&app, CommandPayload::Reset);
    wait_idle(&app);
    let load2 = accept(&app, CommandPayload::ProbeLoad { label: "ok".into() });
    let env = wait_terminal(&app, load2);
    assert!(matches!(
        env.event,
        Event::Completed {
            outcome: OutcomeView::Completed { .. }
        }
    ));
}

#[test]
fn final_progress_precedes_terminal_on_live_stream() {
    let mut limits = SessionLimits::for_test();
    limits.notification_queue_capacity = 64;
    let app = ApplicationService::start(limits);
    let load = accept(&app, CommandPayload::ProbeLoad { label: "ds".into() });
    wait_terminal(&app, load);
    wait_ready(&app);
    let (sub, _) = app.subscribe().expect("sub");
    let run = accept(&app, CommandPayload::ProbeRun { steps: 1 });
    wait_terminal(&app, run);
    let mut saw_final = false;
    while let Some(env) = sub.try_recv() {
        if env.command_id == Some(run) {
            match env.event {
                Event::Progress(p) if p.step == 1 => saw_final = true,
                Event::Completed { .. } => {
                    assert!(saw_final, "terminal preceded final progress");
                }
                _ => {}
            }
        }
    }
}

/// Panic after the worker computed/sent final progress must still surface
/// that actual last step on the live stream before Failed (result mailbox
/// handoff; do not invent a configured final step for arbitrary panics).
#[test]
fn panic_run_final_progress_precedes_failed() {
    let mut limits = SessionLimits::for_test();
    limits.notification_queue_capacity = 128;
    limits.control_history_capacity = 128;
    limits.control_history_max_bytes = 65_536;
    let app = ApplicationService::start(limits);
    let load = accept(&app, CommandPayload::ProbeLoad { label: "ds".into() });
    wait_terminal(&app, load);
    wait_ready(&app);
    let (sub, _) = app.subscribe().expect("sub");
    app.set_worker_behavior(FakeWorkerBehavior::Panic);
    let run = accept(&app, CommandPayload::ProbeRun { steps: 1 });
    let terminal = wait_terminal(&app, run);
    assert!(
        matches!(
            terminal.event,
            Event::Completed {
                outcome: OutcomeView::Failed { .. }
            }
        ),
        "{terminal:?}"
    );
    let mut final_seen = false;
    while let Some(env) = sub.try_recv() {
        if env.command_id == Some(run) {
            match env.event {
                Event::Progress(p) if p.step == 1 => final_seen = true,
                Event::Completed { .. } => {
                    assert!(
                        final_seen,
                        "panic Failed emitted before final progress; envelope={env:?}"
                    );
                }
                _ => {}
            }
        }
    }
    assert!(final_seen, "expected Progress step=1 before Failed");
}

#[test]
fn progress_only_gaps_do_not_force_resync() {
    let mut limits = SessionLimits::for_test();
    limits.control_history_capacity = 2;
    limits.notification_queue_capacity = 64;
    let app = ApplicationService::start(limits);
    let load = accept(&app, CommandPayload::ProbeLoad { label: "ds".into() });
    wait_terminal(&app, load);
    wait_ready(&app);
    app.stall_gate().set_stalled(true);
    let run = accept(&app, CommandPayload::ProbeRun { steps: 10 });
    assert!(app.wait_until(Duration::from_secs(1), |s| s.state == SessionState::Running));
    let cursor = app.snapshot().cursor;
    std::thread::sleep(Duration::from_millis(90));
    let snap = app.snapshot();
    app.inject_worker_event(WorkerEvent::RunTerminal {
        run_id: snap.active_run.unwrap(),
        command_id: run,
        outcome: TerminalOutcome::Completed { step: 0 },
        checkpoint_label: None,
        save_failed: false,
        final_progress: None,
    })
    .unwrap();
    wait_terminal(&app, run);
    assert!(
        matches!(app.replay(cursor), Ok(ReplayOutcome::ControlEvents(_))),
        "{:?} cursor={cursor:?}",
        app.replay(cursor)
    );
}

#[test]
fn oldest_result_stays_expired_not_unknown() {
    let mut limits = SessionLimits::for_test();
    limits.result_retention = 1;
    let app = ApplicationService::start(limits);
    let first = accept(&app, CommandPayload::ProbeLoad { label: "a".into() });
    wait_terminal(&app, first);
    wait_ready(&app);
    for i in 0..8 {
        let id = accept(&app, CommandPayload::ProbeRun { steps: 1 });
        wait_terminal(&app, id);
        wait_ready(&app);
        let _ = i;
    }
    assert!(
        matches!(app.command_result(first), CommandResult::Expired),
        "{:?}",
        app.command_result(first)
    );
}

#[test]
fn oversized_reject_label_not_retained_verbatim() {
    let app = service();
    let _ = app.submit(Command::new(CommandPayload::ProbeLoad {
        label: "x".repeat(1_000_000),
    }));
    let cursor = EventCursor::new(app.session_id(), 0);
    match app.replay(cursor).unwrap() {
        ReplayOutcome::ControlEvents(events) => {
            for env in events {
                if let Event::CommandRejected {
                    command: CommandPayload::ProbeLoad { label },
                    ..
                } = env.event
                {
                    assert!(
                        label.len() <= app.limits().label_max_bytes,
                        "retained {} bytes",
                        label.len()
                    );
                }
            }
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn stop_save_false_reaches_running_worker() {
    let app = service();
    let load = accept(&app, CommandPayload::ProbeLoad { label: "ds".into() });
    wait_terminal(&app, load);
    wait_ready(&app);
    app.stall_gate().set_stalled(true);
    let run = accept(&app, CommandPayload::ProbeRun { steps: 10 });
    assert!(app.wait_until(Duration::from_secs(1), |s| s.state == SessionState::Running));
    let stop = accept(&app, CommandPayload::Stop { save: false });
    let result = wait_terminal(&app, run);
    wait_terminal(&app, stop);
    assert!(
        matches!(
            result.event,
            Event::Completed {
                outcome: OutcomeView::Canceled {
                    saved_checkpoint: None,
                    ..
                }
            }
        ),
        "{result:?}"
    );
}

#[test]
fn load_done_racing_stop_clears_stop_terminal() {
    let mut limits = SessionLimits::for_test();
    limits.worker_step_delay = Duration::from_millis(200);
    let app = ApplicationService::start(limits);
    let load = accept(&app, CommandPayload::ProbeLoad { label: "ds".into() });
    assert!(app.wait_until(Duration::from_secs(1), |s| s.state == SessionState::Loading));
    let snap = app.snapshot();
    let stop = accept(&app, CommandPayload::Stop { save: false });
    app.inject_worker_event(WorkerEvent::LoadDone {
        run_id: snap.active_run.unwrap(),
        command_id: load,
        label: "ds".into(),
    })
    .unwrap();
    wait_terminal(&app, load);
    assert!(
        app.wait_until(Duration::from_secs(1), |_| {
            matches!(app.command_result(stop), CommandResult::Ready(_))
        }),
        "{:?}",
        app.command_result(stop)
    );
}

#[test]
fn malformed_progress_and_terminal_rejected_then_valid_still_works() {
    let app = service();
    let load = accept(&app, CommandPayload::ProbeLoad { label: "ds".into() });
    wait_terminal(&app, load);
    wait_ready(&app);
    app.stall_gate().set_stalled(true);
    let (sub, _) = app.subscribe().expect("sub");
    let run = accept(&app, CommandPayload::ProbeRun { steps: 10 });
    assert!(app.wait_until(Duration::from_secs(1), |s| s.state == SessionState::Running));
    let snap = app.snapshot();
    let run_id = snap.active_run.unwrap();

    // step > total
    let step_past = ProgressSnapshot {
        step: 9,
        total_steps: Some(1),
        splats: 0,
        elapsed_ms: 0,
        eta_ms: None,
        metrics: vec![],
    };
    assert!(step_past.validate().is_err());
    app.inject_worker_event(WorkerEvent::Progress {
        run_id,
        command_id: run,
        snapshot: step_past,
    })
    .unwrap();

    // blank + nonfinite + would-be-duplicate metrics
    let malformed = ProgressSnapshot {
        step: 1,
        total_steps: Some(10),
        splats: 0,
        elapsed_ms: 0,
        eta_ms: None,
        metrics: vec![
            spirula_core::ProgressMetric {
                name: "".into(),
                value: f64::NAN,
            },
            spirula_core::ProgressMetric {
                name: "loss".into(),
                value: 1.0,
            },
            spirula_core::ProgressMetric {
                name: "loss".into(),
                value: 2.0,
            },
        ],
    };
    assert!(malformed.validate().is_err());
    app.inject_worker_event(WorkerEvent::Progress {
        run_id,
        command_id: run,
        snapshot: malformed,
    })
    .unwrap();

    app.inject_worker_event(WorkerEvent::RunTerminal {
        run_id,
        command_id: run,
        outcome: TerminalOutcome::Failed {
            error: spirula_core::DomainError::try_new(ErrorCode::Canceled, "wrong").unwrap(),
        },
        checkpoint_label: None,
        save_failed: false,
        final_progress: None,
    })
    .unwrap();
    app.inject_worker_event(WorkerEvent::RunTerminal {
        run_id,
        command_id: run,
        outcome: TerminalOutcome::Canceled {
            step: 0,
            reason: spirula_core::CancelReason::User,
            saved_checkpoint: Some(" ".into()),
        },
        checkpoint_label: None,
        save_failed: false,
        final_progress: None,
    })
    .unwrap();

    std::thread::sleep(Duration::from_millis(40));
    assert!(matches!(app.command_result(run), CommandResult::Pending));
    while let Some(env) = sub.try_recv() {
        if let Event::Progress(p) = env.event {
            assert!(p.validate().is_ok(), "malformed progress published: {p:?}");
        }
    }

    // Matching active identity still accepts a valid terminal afterward.
    app.inject_worker_event(WorkerEvent::RunTerminal {
        run_id,
        command_id: run,
        outcome: TerminalOutcome::Completed { step: 3 },
        checkpoint_label: None,
        save_failed: false,
        final_progress: Some(ProgressSnapshot {
            step: 3,
            total_steps: Some(10),
            splats: 0,
            elapsed_ms: 1,
            eta_ms: None,
            metrics: vec![],
        }),
    })
    .unwrap();
    let env = wait_terminal(&app, run);
    assert!(matches!(
        env.event,
        Event::Completed {
            outcome: OutcomeView::Completed { step: 3 }
        }
    ));
}

#[test]
fn wrong_kind_load_done_leaves_probe_run_intact() {
    let app = service();
    let load = accept(&app, CommandPayload::ProbeLoad { label: "ds".into() });
    wait_terminal(&app, load);
    wait_ready(&app);
    app.stall_gate().set_stalled(true);
    let run = accept(&app, CommandPayload::ProbeRun { steps: 10 });
    assert!(app.wait_until(Duration::from_secs(1), |s| s.state == SessionState::Running));
    let snap = app.snapshot();
    let run_id = snap.active_run.unwrap();
    app.inject_worker_event(WorkerEvent::LoadDone {
        run_id,
        command_id: run,
        label: "foreign dataset".into(),
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(40));
    assert!(matches!(app.command_result(run), CommandResult::Pending));
    assert_eq!(app.snapshot().state, SessionState::Running);
    app.inject_worker_event(WorkerEvent::RunTerminal {
        run_id,
        command_id: run,
        outcome: TerminalOutcome::Completed { step: 2 },
        checkpoint_label: None,
        save_failed: false,
        final_progress: None,
    })
    .unwrap();
    let env = wait_terminal(&app, run);
    assert!(matches!(
        env.event,
        Event::Completed {
            outcome: OutcomeView::Completed { step: 2 }
        }
    ));
}

#[test]
fn bound_error_preserves_code_category_retryable_user_action() {
    let mut limits = SessionLimits::for_test();
    limits.error_message_max_bytes = 32;
    limits.error_event_max_bytes = 64;
    let app = ApplicationService::start(limits);
    let load = accept(&app, CommandPayload::ProbeLoad { label: "ds".into() });
    wait_terminal(&app, load);
    wait_ready(&app);
    app.stall_gate().set_stalled(true);
    let run = accept(&app, CommandPayload::ProbeRun { steps: 10 });
    assert!(app.wait_until(Duration::from_secs(1), |s| s.state == SessionState::Running));
    let snap = app.snapshot();
    let oversized = spirula_core::DomainError::try_new(ErrorCode::Internal, "x".repeat(10_000))
        .unwrap()
        .with_retryable(true)
        .with_user_action(UserAction::Retry);
    assert_eq!(oversized.code(), ErrorCode::Internal);
    assert_eq!(oversized.category(), ErrorCode::Internal.category());
    assert!(oversized.retryable());
    assert_eq!(oversized.user_action(), Some(UserAction::Retry));

    app.inject_worker_event(WorkerEvent::RunTerminal {
        run_id: snap.active_run.unwrap(),
        command_id: run,
        outcome: TerminalOutcome::Failed { error: oversized },
        checkpoint_label: None,
        save_failed: false,
        final_progress: None,
    })
    .unwrap();
    let env = wait_terminal(&app, run);
    match env.event {
        Event::Completed {
            outcome: OutcomeView::Failed { error },
        } => {
            assert_eq!(error.code(), ErrorCode::Internal);
            assert_eq!(error.category(), ErrorCode::Internal.category());
            assert!(error.retryable());
            assert_eq!(error.user_action(), Some(UserAction::Retry));
            assert!(error.message().len() <= app.limits().error_message_max_bytes);
        }
        other => panic!("expected failed terminal, got {other:?}"),
    }
}

#[test]
fn dropped_subscription_releases_fanout_slot() {
    let mut limits = SessionLimits::for_test();
    limits.max_subscribers = 1;
    let app = ApplicationService::start(limits);
    drop(app.subscribe().unwrap());
    assert!(
        app.subscribe().is_ok(),
        "dropped subscriber must release capacity"
    );
}

#[test]
fn session_limits_normalize_zero_and_slow_poll() {
    let raw = SessionLimits {
        command_queue_capacity: 0,
        label_max_bytes: 1,
        cancel_poll_interval: Duration::from_millis(500),
        heartbeat_interval: Duration::from_millis(100),
        teardown_deadline: Duration::from_millis(1),
        control_history_max_bytes: 8,
        ..SessionLimits::default()
    };
    let app = ApplicationService::start(raw);
    let effective = app.limits();
    assert!(effective.command_queue_capacity >= 1);
    assert!(effective.label_max_bytes >= 32);
    assert!(effective.cancel_poll_interval <= Duration::from_millis(250));
    assert!(effective.cancel_poll_interval <= effective.heartbeat_interval);
    // Teardown floor covers cancel observation; 1ms cannot silently win.
    assert!(effective.teardown_deadline >= effective.cancel_poll_interval.saturating_mul(4));
    // History floor fits one legal max control event (label+error+overhead).
    let legal = SessionLimits::max_legal_control_event_bytes(
        effective.label_max_bytes,
        effective.error_event_max_bytes,
        effective.reason_max_bytes,
    );
    assert!(effective.control_history_max_bytes >= legal);
}

#[test]
fn tiny_error_caps_fit_bound_error_fallback() {
    let raw = SessionLimits {
        error_message_max_bytes: 0,
        context_value_max_bytes: 0,
        error_event_max_bytes: 0,
        control_history_max_bytes: 65536,
        ..SessionLimits::for_test()
    };
    let app = ApplicationService::start(raw);
    let (sub, _) = app.subscribe().unwrap();
    let _ = app.submit(Command::new(CommandPayload::ProbeRun { steps: 0 }));
    let env = sub.try_recv().expect("reject event");
    match env.event {
        Event::CommandRejected { error, .. } => {
            let n = 64
                + error.message().len()
                + error
                    .context()
                    .iter()
                    .map(|(k, v)| k.as_str().len() + v.len() + 8)
                    .sum::<usize>();
            assert!(
                n <= app.limits().error_event_max_bytes,
                "retained error {n} > cap {}: {error:?}",
                app.limits().error_event_max_bytes
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn checkpoint_saved_matches_canceled_terminal_label() {
    let app = service();
    let load = accept(&app, CommandPayload::ProbeLoad { label: "ds".into() });
    wait_terminal(&app, load);
    wait_ready(&app);
    let (sub, _) = app.subscribe().unwrap();
    app.stall_gate().set_stalled(true);
    let run = accept(&app, CommandPayload::ProbeRun { steps: 10 });
    assert!(app.wait_until(Duration::from_secs(1), |s| s.state == SessionState::Running));
    let run_id = app.snapshot().active_run.unwrap();
    app.inject_worker_event(WorkerEvent::RunTerminal {
        run_id,
        command_id: run,
        outcome: TerminalOutcome::Canceled {
            step: 0,
            reason: spirula_core::CancelReason::User,
            saved_checkpoint: Some("checkpoint-A".into()),
        },
        checkpoint_label: Some("checkpoint-B".into()),
        save_failed: false,
        final_progress: None,
    })
    .unwrap();
    let result = wait_terminal(&app, run);
    let terminal_label = match result.event {
        Event::Completed {
            outcome: OutcomeView::Canceled {
                saved_checkpoint, ..
            },
        } => saved_checkpoint,
        other => panic!("{other:?}"),
    };
    let mut saw = false;
    while let Some(env) = sub.try_recv() {
        if let Event::CheckpointSaved { label, .. } = env.event {
            assert_eq!(Some(label), terminal_label);
            saw = true;
        }
    }
    assert!(saw || terminal_label.as_deref() == Some("checkpoint-A"));
}

#[test]
fn save_failed_does_not_force_failed_outcome_to_ready() {
    let app = service();
    let load = accept(&app, CommandPayload::ProbeLoad { label: "ds".into() });
    wait_terminal(&app, load);
    wait_ready(&app);
    app.stall_gate().set_stalled(true);
    let run = accept(&app, CommandPayload::ProbeRun { steps: 10 });
    assert!(app.wait_until(Duration::from_secs(1), |s| s.state == SessionState::Running));
    let run_id = app.snapshot().active_run.unwrap();
    app.inject_worker_event(WorkerEvent::RunTerminal {
        run_id,
        command_id: run,
        outcome: TerminalOutcome::Failed {
            error: spirula_core::DomainError::try_new(ErrorCode::Internal, "real failure").unwrap(),
        },
        checkpoint_label: None,
        save_failed: true,
        final_progress: None,
    })
    .unwrap();
    let _ = wait_terminal(&app, run);
    assert_eq!(app.snapshot().state, SessionState::Failed);
}

#[test]
fn invalid_final_progress_keeps_run_pending_for_later_valid() {
    let app = service();
    let load = accept(&app, CommandPayload::ProbeLoad { label: "ds".into() });
    wait_terminal(&app, load);
    wait_ready(&app);
    app.stall_gate().set_stalled(true);
    let run = accept(&app, CommandPayload::ProbeRun { steps: 10 });
    assert!(app.wait_until(Duration::from_secs(1), |s| s.state == SessionState::Running));
    let run_id = app.snapshot().active_run.unwrap();
    app.inject_worker_event(WorkerEvent::RunTerminal {
        run_id,
        command_id: run,
        outcome: TerminalOutcome::Completed { step: 1 },
        checkpoint_label: None,
        save_failed: false,
        final_progress: Some(ProgressSnapshot {
            step: 9,
            total_steps: Some(1),
            splats: 0,
            elapsed_ms: 0,
            eta_ms: None,
            metrics: vec![],
        }),
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(30));
    assert!(matches!(app.command_result(run), CommandResult::Pending));
    app.inject_worker_event(WorkerEvent::RunTerminal {
        run_id,
        command_id: run,
        outcome: TerminalOutcome::Completed { step: 1 },
        checkpoint_label: None,
        save_failed: false,
        final_progress: None,
    })
    .unwrap();
    let env = wait_terminal(&app, run);
    assert!(matches!(
        env.event,
        Event::Completed {
            outcome: OutcomeView::Completed { step: 1 }
        }
    ));
}

#[test]
fn long_lifetime_accepted_ids_stay_expired_under_budget() {
    let mut limits = SessionLimits::for_test();
    limits.result_retention = 2;
    limits.control_history_capacity = 4;
    limits.control_history_max_bytes = 1024;
    let app = ApplicationService::start(limits);
    let first = accept(&app, CommandPayload::ProbeLoad { label: "a".into() });
    wait_terminal(&app, first);
    wait_ready(&app);
    for i in 0..64 {
        let id = accept(&app, CommandPayload::ProbeRun { steps: 1 });
        wait_terminal(&app, id);
        wait_ready(&app);
        let _ = i;
        // Rejects also exercise Accepted/Rejected byte accounting.
        let _ = app.submit(Command::new(CommandPayload::ProbeLoad {
            label: "z".repeat(app.limits().label_max_bytes + 8),
        }));
    }
    assert!(
        matches!(app.command_result(first), CommandResult::Expired),
        "{:?}",
        app.command_result(first)
    );
    // Unknown only for never-accepted future-looking ids.
    assert!(matches!(
        app.command_result(spirula_application::CommandId::new(u64::MAX)),
        CommandResult::Unknown
    ));
}

#[test]
fn reset_clears_failed_last_outcome() {
    let app = service();
    app.set_worker_behavior(FakeWorkerBehavior::Fail);
    let id = accept(
        &app,
        CommandPayload::ProbeLoad {
            label: "fail".into(),
        },
    );
    wait_terminal(&app, id);
    assert!(matches!(
        app.snapshot().last_outcome,
        Some(OutcomeView::Failed { .. })
    ));
    let reset = accept(&app, CommandPayload::Reset);
    wait_terminal(&app, reset);
    assert!(
        !matches!(
            app.snapshot().last_outcome,
            Some(OutcomeView::Failed { .. })
        ),
        "Reset must clear error snapshot: {:?}",
        app.snapshot()
    );
    assert_eq!(app.snapshot().state, SessionState::Idle);
}

#[test]
fn saved_stop_enters_saving_then_ready() {
    let mut limits = SessionLimits::for_test();
    limits.notification_queue_capacity = 64;
    let app = ApplicationService::start(limits);
    let load = accept(&app, CommandPayload::ProbeLoad { label: "ds".into() });
    wait_terminal(&app, load);
    wait_ready(&app);
    let (sub, _) = app.subscribe().expect("sub");
    app.stall_gate().set_stalled(true);
    let run = accept(&app, CommandPayload::ProbeRun { steps: 10 });
    assert!(app.wait_until(Duration::from_secs(1), |s| s.state == SessionState::Running));
    let stop = accept(&app, CommandPayload::Stop { save: true });
    wait_terminal(&app, run);
    wait_terminal(&app, stop);
    let mut saw_saving = false;
    let mut saw_checkpoint = false;
    while let Some(env) = sub.try_recv() {
        match env.event {
            Event::StateChanged {
                to: SessionState::Saving,
                ..
            } => saw_saving = true,
            Event::CheckpointSaved { .. } => saw_checkpoint = true,
            _ => {}
        }
    }
    assert!(saw_saving, "Stop{{save:true}} must enter Saving");
    assert!(saw_checkpoint, "saved stop must emit CheckpointSaved");
    assert_eq!(app.snapshot().state, SessionState::Ready);
}

#[test]
fn forced_teardown_joins_and_ends_with_shutdown_complete() {
    let mut limits = SessionLimits::for_test();
    limits.cancel_poll_interval = Duration::from_millis(250);
    limits.heartbeat_interval = Duration::from_secs(1);
    limits.worker_step_delay = Duration::from_millis(500);
    limits.teardown_deadline = Duration::from_millis(1);
    limits.notification_queue_capacity = 128;
    let app = ApplicationService::start(limits);
    // Effective teardown must have been raised for cancel budget.
    assert!(app.limits().teardown_deadline >= Duration::from_millis(1000));
    let load = accept(&app, CommandPayload::ProbeLoad { label: "ds".into() });
    wait_terminal(&app, load);
    wait_ready(&app);
    let (sub, _) = app.subscribe().expect("sub");
    app.stall_gate().set_stalled(true);
    let _run = accept(&app, CommandPayload::ProbeRun { steps: 10 });
    assert!(app.wait_until(Duration::from_secs(1), |s| s.state == SessionState::Running));
    drop(app);
    let mut saw_shutdown = false;
    let mut after_shutdown = Vec::new();
    while let Some(env) = sub.try_recv() {
        if matches!(env.event, Event::ShutdownComplete) {
            saw_shutdown = true;
        } else if saw_shutdown {
            after_shutdown.push(env.event);
        }
    }
    assert!(saw_shutdown, "forced teardown must emit ShutdownComplete");
    assert!(
        after_shutdown.iter().all(|e| {
            !matches!(
                e,
                Event::CommandAccepted { .. } | Event::Completed { .. } | Event::ShutdownComplete
            )
        }),
        "no canonical Accepted/terminal after ShutdownComplete: {after_shutdown:?}"
    );
}

#[test]
fn resync_retained_terminals_include_result_cache() {
    let mut limits = SessionLimits::for_test();
    limits.control_history_capacity = 2;
    limits.control_history_max_bytes = 512;
    limits.result_retention = 8;
    limits.label_max_bytes = 64;
    let app = ApplicationService::start(limits);
    let first = accept(
        &app,
        CommandPayload::ProbeLoad {
            label: "keep".into(),
        },
    );
    wait_terminal(&app, first);
    wait_ready(&app);
    // Overflow the control ring with rejections while the load terminal stays
    // in the result cache.
    for i in 0..12 {
        let _ = app.submit(Command::new(CommandPayload::ProbeRun { steps: 0 }));
        let _ = i;
    }
    let cursor = EventCursor::new(app.session_id(), 0);
    match app.replay(cursor).expect("replay") {
        ReplayOutcome::ResyncRequired {
            retained_terminals, ..
        } => {
            assert!(
                retained_terminals
                    .iter()
                    .any(|e| e.command_id == Some(first)),
                "result-cache terminal must appear in resync: {retained_terminals:?}"
            );
        }
        other => panic!("expected resync after control overflow, got {other:?}"),
    }
    assert!(matches!(app.command_result(first), CommandResult::Ready(_)));
}
