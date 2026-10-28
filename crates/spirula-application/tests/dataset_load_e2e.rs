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
