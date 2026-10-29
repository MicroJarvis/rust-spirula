//! End-to-end: application LoadDataset → spirula-io COLMAP (P3-F03).

use std::path::PathBuf;
use std::time::Duration;

use spirula_application::{
    ApplicationService, Command, CommandPayload, CommandResult, Event, SessionLimits, SubmitOutcome,
};
use spirula_core::{ErrorCode, OutcomeView};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/datasets/tiny-colmap")
}

fn wait_terminal(
    app: &ApplicationService,
    id: spirula_application::CommandId,
) -> spirula_application::EventEnvelope {
    let ok = app.wait_until(Duration::from_secs(5), |_| {
        matches!(app.command_result(id), CommandResult::Ready(_))
    });
    assert!(ok, "terminal timeout for {id:?}");
    match app.command_result(id) {
        CommandResult::Ready(env) => env,
        other => panic!("expected Ready, got {other:?}"),
    }
}

#[test]
fn session_load_dataset_e2e() {
    let app = ApplicationService::start(SessionLimits::for_test());
    let path = fixtures().display().to_string();
    let id = match app.submit(Command::new(CommandPayload::LoadDataset {
        path: path.clone(),
        recon_dir: String::new(),
        eval_mode: "all".into(),
        split: "train".into(),
    })) {
        SubmitOutcome::Accepted { id } => id,
        SubmitOutcome::Rejected { error } => panic!("reject: {error}"),
    };
    let terminal = wait_terminal(&app, id);
    match terminal.event {
        Event::Completed {
            outcome: OutcomeView::Completed { step },
        } => assert_eq!(step, 0),
        other => panic!("unexpected terminal {other:?}"),
    }
    let snap = app.snapshot();
    let replay = app
        .replay(spirula_application::EventCursor::new(snap.session_id, 0))
        .expect("replay");
    let spirula_application::ReplayOutcome::ControlEvents(events) = replay else {
        panic!("resync unexpected");
    };
    let loaded = events.iter().find_map(|e| match &e.event {
        Event::DatasetLoaded {
            num_images,
            num_points,
            num_train,
            ..
        } if e.command_id == Some(id) => Some((*num_images, *num_points, *num_train)),
        _ => None,
    });
    assert_eq!(loaded, Some((1, 4, 1)));
}

#[test]
fn cancel_load_never_publishes_dataset_loaded() {
    let app = ApplicationService::start(SessionLimits::for_test());
    app.stall_gate().set_stalled(true);
    let path = fixtures().display().to_string();
    let id = match app.submit(Command::new(CommandPayload::LoadDataset {
        path,
        recon_dir: String::new(),
        eval_mode: "all".into(),
        split: "train".into(),
    })) {
        SubmitOutcome::Accepted { id } => id,
        SubmitOutcome::Rejected { error } => panic!("reject: {error}"),
    };
    let stop = match app.submit(Command::new(CommandPayload::Stop { save: false })) {
        SubmitOutcome::Accepted { id } => id,
        SubmitOutcome::Rejected { error } => panic!("stop reject: {error}"),
    };
    app.stall_gate().set_stalled(false);
    let _ = wait_terminal(&app, id);
    let _ = wait_terminal(&app, stop);
    let snap = app.snapshot();
    let replay = app
        .replay(spirula_application::EventCursor::new(snap.session_id, 0))
        .expect("replay");
    let spirula_application::ReplayOutcome::ControlEvents(events) = replay else {
        panic!("resync");
    };
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.event, Event::DatasetLoaded { .. }) && e.command_id == Some(id)),
        "no DatasetLoaded after cancel"
    );
    match app.command_result(id) {
        CommandResult::Ready(env) => match env.event {
            Event::Completed {
                outcome: OutcomeView::Canceled { .. },
            } => {}
            other => panic!("expected Canceled, got {other:?}"),
        },
        other => panic!("{other:?}"),
    }
}

#[test]
fn sync_helper_loads_tiny() {
    let app = ApplicationService::start(SessionLimits::for_test());
    let ds = app
        .load_colmap_dataset_file(&fixtures(), None, "all", "train")
        .expect("load");
    assert_eq!(ds.frames.len(), 1);
    assert_eq!(ds.points.num(), 4);
}

#[test]
fn bad_path_fails_typed() {
    let app = ApplicationService::start(SessionLimits::for_test());
    let id = match app.submit(Command::new(CommandPayload::LoadDataset {
        path: "/no/such/dataset/path".into(),
        recon_dir: String::new(),
        eval_mode: "all".into(),
        split: "train".into(),
    })) {
        SubmitOutcome::Accepted { id } => id,
        SubmitOutcome::Rejected { error } => panic!("reject: {error}"),
    };
    let terminal = wait_terminal(&app, id);
    match terminal.event {
        Event::Completed {
            outcome: OutcomeView::Failed { error },
        } => {
            assert!(matches!(
                error.code(),
                ErrorCode::NotFound | ErrorCode::UnsupportedFormat
            ));
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn load_dataset_rejects_multi_mb_recon_and_enum() {
    // Contrast tip 6bca1fc: 2MB recon was admitted=true.
    let app = ApplicationService::start(SessionLimits::for_test());
    let big = "r".repeat(2 * 1024 * 1024);
    match app.submit(Command::new(CommandPayload::LoadDataset {
        path: fixtures().display().to_string(),
        recon_dir: big,
        eval_mode: "all".into(),
        split: "train".into(),
    })) {
        SubmitOutcome::Accepted { .. } => panic!("2MB recon_dir must reject at admission"),
        SubmitOutcome::Rejected { error } => {
            assert_eq!(error.code(), ErrorCode::ValueOutOfRange);
            assert!(!error.message().contains(&"r".repeat(1000)));
        }
    }
    let giant_enum = "x".repeat(64 * 1024);
    match app.submit(Command::new(CommandPayload::LoadDataset {
        path: fixtures().display().to_string(),
        recon_dir: String::new(),
        eval_mode: giant_enum.clone(),
        split: "train".into(),
    })) {
        SubmitOutcome::Accepted { .. } => panic!("giant eval_mode must reject"),
        SubmitOutcome::Rejected { error } => {
            assert_eq!(error.code(), ErrorCode::ValueOutOfRange);
            assert!(!error.message().contains(&"x".repeat(100)));
        }
    }
    // Legitimate ~4096 boundary still admits.
    let path_ok = "p".repeat(4096);
    match app.submit(Command::new(CommandPayload::LoadDataset {
        path: path_ok,
        recon_dir: String::new(),
        eval_mode: "all".into(),
        split: "train".into(),
    })) {
        SubmitOutcome::Accepted { id } => {
            let _ = wait_terminal(&app, id); // will fail NotFound; admission OK
        }
        SubmitOutcome::Rejected { error } => panic!("4096 path must admit: {error}"),
    }
}

#[test]
fn dataset_panic_closes_command_and_recovers() {
    // Contrast tip 6bca1fc: panic Failed via RunTerminal left Pending=true.
    use spirula_application::FakeWorkerBehavior;
    let app = ApplicationService::start(SessionLimits::for_test());
    app.set_worker_behavior(FakeWorkerBehavior::Panic);
    let id = match app.submit(Command::new(CommandPayload::LoadDataset {
        path: fixtures().display().to_string(),
        recon_dir: String::new(),
        eval_mode: "all".into(),
        split: "train".into(),
    })) {
        SubmitOutcome::Accepted { id } => id,
        SubmitOutcome::Rejected { error } => panic!("reject: {error}"),
    };
    let terminal = wait_terminal(&app, id);
    match terminal.event {
        Event::Completed {
            outcome: OutcomeView::Failed { error },
        } => assert_eq!(error.code(), ErrorCode::Internal),
        other => panic!("expected Failed Internal, got {other:?}"),
    }
    assert!(matches!(app.command_result(id), CommandResult::Ready(_)));
    // Reset / reload recovery.
    app.set_worker_behavior(FakeWorkerBehavior::Succeed);
    let reset = match app.submit(Command::new(CommandPayload::Reset)) {
        SubmitOutcome::Accepted { id } => id,
        SubmitOutcome::Rejected { error } => panic!("reset reject: {error}"),
    };
    let _ = wait_terminal(&app, reset);
    let id2 = match app.submit(Command::new(CommandPayload::LoadDataset {
        path: fixtures().display().to_string(),
        recon_dir: String::new(),
        eval_mode: "all".into(),
        split: "train".into(),
    })) {
        SubmitOutcome::Accepted { id } => id,
        SubmitOutcome::Rejected { error } => {
            panic!("reject: {error} (phase={:?})", app.snapshot().state)
        }
    };
    let terminal = wait_terminal(&app, id2);
    assert!(matches!(
        terminal.event,
        Event::Completed {
            outcome: OutcomeView::Completed { .. }
        }
    ));
}

#[test]
fn dataset_panic_racing_stop_one_terminal() {
    use spirula_application::FakeWorkerBehavior;
    let app = ApplicationService::start(SessionLimits::for_test());
    app.stall_gate().set_stalled(true);
    app.set_worker_behavior(FakeWorkerBehavior::Panic);
    let id = match app.submit(Command::new(CommandPayload::LoadDataset {
        path: fixtures().display().to_string(),
        recon_dir: String::new(),
        eval_mode: "all".into(),
        split: "train".into(),
    })) {
        SubmitOutcome::Accepted { id } => id,
        SubmitOutcome::Rejected { error } => panic!("reject: {error}"),
    };
    let stop = match app.submit(Command::new(CommandPayload::Stop { save: false })) {
        SubmitOutcome::Accepted { id } => id,
        SubmitOutcome::Rejected { error } => panic!("stop reject: {error}"),
    };
    app.stall_gate().set_stalled(false);
    let term = wait_terminal(&app, id);
    let _ = wait_terminal(&app, stop);
    // Exactly one terminal for the load command; Stop arbitration → Canceled
    // or Failed Internal if panic wins first — never stuck Pending.
    assert!(matches!(
        term.event,
        Event::Completed {
            outcome: OutcomeView::Canceled { .. } | OutcomeView::Failed { .. }
        }
    ));
    assert!(matches!(app.command_result(id), CommandResult::Ready(_)));
}

#[test]
fn slow_subscriber_queue_still_admits_bounded_reject() {
    let mut limits = SessionLimits::for_test();
    limits.notification_queue_capacity = 1;
    limits.label_max_bytes = 32;
    let app = ApplicationService::start(limits);
    let (_sub, _) = app.subscribe().expect("sub");
    // Fill / pressure the notify queue with rejects that must stay bounded.
    for _ in 0..8 {
        match app.submit(Command::new(CommandPayload::LoadDataset {
            path: fixtures().display().to_string(),
            recon_dir: "r".repeat(2 * 1024 * 1024),
            eval_mode: "all".into(),
            split: "train".into(),
        })) {
            SubmitOutcome::Rejected { error } => {
                assert!(error.message().len() < 512);
            }
            SubmitOutcome::Accepted { .. } => panic!("must reject"),
        }
    }
}

#[test]
fn slow_subscriber_does_not_block_dataset_cancel_recovery() {
    let mut limits = SessionLimits::for_test();
    limits.notification_queue_capacity = 1;
    let app = ApplicationService::start(limits);
    let (_sub, _) = app.subscribe().expect("subscribe");
    // Do not drain — fill the live subscriber queue.
    app.stall_gate().set_stalled(true);
    let path = fixtures().display().to_string();
    let id = match app.submit(Command::new(CommandPayload::LoadDataset {
        path,
        recon_dir: String::new(),
        eval_mode: "all".into(),
        split: "train".into(),
    })) {
        SubmitOutcome::Accepted { id } => id,
        SubmitOutcome::Rejected { error } => panic!("reject: {error}"),
    };
    assert!(app.wait_until(Duration::from_secs(1), |s| s.active_command == Some(id)));
    let _ = app.submit(Command::new(CommandPayload::Stop { save: false }));
    let terminal = wait_terminal(&app, id);
    assert!(matches!(
        terminal.event,
        Event::Completed {
            outcome: OutcomeView::Canceled { .. }
        }
    ));
    app.stall_gate().set_stalled(false);
    // Recovery load must complete (same pattern as image_ops slow-subscriber).
    let id2 = match app.submit(Command::new(CommandPayload::LoadDataset {
        path: fixtures().display().to_string(),
        recon_dir: String::new(),
        eval_mode: "all".into(),
        split: "train".into(),
    })) {
        SubmitOutcome::Accepted { id } => id,
        SubmitOutcome::Rejected { error } => {
            panic!("reject: {error} (state={:?})", app.snapshot().state)
        }
    };
    let terminal2 = wait_terminal(&app, id2);
    assert!(matches!(
        terminal2.event,
        Event::Completed {
            outcome: OutcomeView::Completed { .. }
        }
    ));
}

#[test]
fn stop_and_shutdown_race_one_terminal_per_accepted_load() {
    let app = ApplicationService::start(SessionLimits::for_test());
    app.stall_gate().set_stalled(true);
    let path = fixtures().display().to_string();
    let id = match app.submit(Command::new(CommandPayload::LoadDataset {
        path,
        recon_dir: String::new(),
        eval_mode: "all".into(),
        split: "train".into(),
    })) {
        SubmitOutcome::Accepted { id } => id,
        SubmitOutcome::Rejected { error } => panic!("reject: {error}"),
    };
    let _ = app.submit(Command::new(CommandPayload::Stop { save: false }));
    let _ = app.submit(Command::new(CommandPayload::Shutdown));
    app.stall_gate().set_stalled(false);
    let terminal = wait_terminal(&app, id);
    match &terminal.event {
        Event::Completed {
            outcome: OutcomeView::Canceled { .. },
        } => {}
        other => panic!("expected single Canceled terminal, got {other:?}"),
    }
    // Exactly one Ready result for the load command (no DatasetLoaded).
    match app.command_result(id) {
        CommandResult::Ready(env) => assert!(matches!(
            env.event,
            Event::Completed {
                outcome: OutcomeView::Canceled { .. }
            }
        )),
        other => panic!("{other:?}"),
    }
    // Shutdown may close the session (replay → Resync); command_result above
    // is the authoritative exactly-once terminal proof. When history is still
    // readable, also assert no DatasetLoaded for this command.
    let snap = app.snapshot();
    if let Ok(spirula_application::ReplayOutcome::ControlEvents(events)) =
        app.replay(spirula_application::EventCursor::new(snap.session_id, 0))
    {
        let loaded = events
            .iter()
            .filter(|e| matches!(e.event, Event::DatasetLoaded { .. }) && e.command_id == Some(id))
            .count();
        assert_eq!(loaded, 0, "no DatasetLoaded after cancel race");
        let canceled = events
            .iter()
            .filter(|e| {
                e.command_id == Some(id)
                    && matches!(
                        e.event,
                        Event::Completed {
                            outcome: OutcomeView::Canceled { .. }
                        }
                    )
            })
            .count();
        assert_eq!(canceled, 1, "exactly one terminal Canceled for load");
    }
}
