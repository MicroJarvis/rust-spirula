//! End-to-end: application/session command → spirula-io image orient/mask (P3-F02).

use std::path::PathBuf;
use std::time::Duration;

use spirula_application::{
    ApplicationService, Command, CommandPayload, CommandResult, Event, PrepareMaskArgs,
    SessionLimits, SubmitOutcome,
};
use spirula_core::{ErrorCode, OutcomeView};
use spirula_io::{ApplyPolicy, MaskSourceKind};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/images")
}

fn wait_terminal(
    app: &ApplicationService,
    id: spirula_application::CommandId,
) -> spirula_application::EventEnvelope {
    let ok = app.wait_until(Duration::from_secs(2), |_| {
        matches!(app.command_result(id), CommandResult::Ready(_))
    });
    assert!(ok, "terminal timeout for {id:?}");
    match app.command_result(id) {
        CommandResult::Ready(env) => env,
        other => panic!("expected Ready, got {other:?}"),
    }
}

#[test]
fn session_command_prepare_oriented_image_e2e() {
    let app = ApplicationService::start(SessionLimits::for_test());
    let path = fixtures().join("nonsquare_4x2.png");
    let (w, h) = app.probe_image_file_size(&path).expect("probe");
    assert_eq!((w, h), (4, 2));

    let path_s = path.to_string_lossy().into_owned();
    let id = match app.submit(Command::new(CommandPayload::PrepareOrientedImage {
        path: path_s.clone(),
        orientation: Some(6),
        turns_only: true,
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

    // Control history must include the real ImagePrepared event for this command.
    let snap = app.snapshot();
    let replay = app
        .replay(spirula_application::EventCursor::new(snap.session_id, 0))
        .expect("replay");
    let spirula_application::ReplayOutcome::ControlEvents(events) = replay else {
        panic!("resync unexpected");
    };
    let prepared = events.iter().find_map(|e| match &e.event {
        Event::ImagePrepared {
            path: p,
            width,
            height,
            orientation,
            turns_only,
        } if e.command_id == Some(id) => {
            Some((p.clone(), *width, *height, *orientation, *turns_only))
        }
        _ => None,
    });
    let (p, ow, oh, o, turns) = prepared.expect("ImagePrepared");
    assert_eq!(p, path_s);
    assert_eq!((ow, oh), (2, 4));
    assert_eq!(o, 6);
    assert!(turns);

    // Sync helper still matches command path dimensions.
    let oriented = app
        .prepare_oriented_rgb_file(&path, Some(6), ApplyPolicy::TurnsOnly)
        .expect("orient");
    assert_eq!((oriented.width, oriented.height), (2, 4));
    assert_eq!(oriented.rgb.len(), 2 * 4 * 3);
}

#[test]
fn session_command_prepare_normalized_mask_e2e() {
    let app = ApplicationService::start(SessionLimits::for_test());
    let path = fixtures().join("mask_4x2.png");
    let path_s = path.to_string_lossy().into_owned();
    let id = match app.submit(Command::new(CommandPayload::PrepareNormalizedMask {
        path: path_s.clone(),
        dst_w: 2,
        dst_h: 4,
        orientation: 6,
        turns_only: true,
        flip: false,
        alpha: false,
    })) {
        SubmitOutcome::Accepted { id } => id,
        SubmitOutcome::Rejected { error } => panic!("reject: {error}"),
    };
    let terminal = wait_terminal(&app, id);
    assert!(matches!(
        terminal.event,
        Event::Completed {
            outcome: OutcomeView::Completed { .. }
        }
    ));

    let snap = app.snapshot();
    let replay = app
        .replay(spirula_application::EventCursor::new(snap.session_id, 0))
        .expect("replay");
    let spirula_application::ReplayOutcome::ControlEvents(events) = replay else {
        panic!("resync unexpected");
    };
    let prepared = events.iter().find_map(|e| match &e.event {
        Event::MaskPrepared {
            path: p,
            width,
            height,
            kept,
            alpha,
        } if e.command_id == Some(id) => Some((p.clone(), *width, *height, *kept, *alpha)),
        _ => None,
    });
    let (p, mw, mh, kept, alpha) = prepared.expect("MaskPrepared");
    assert_eq!(p, path_s);
    assert_eq!((mw, mh), (2, 4));
    assert_eq!(kept, 4);
    assert!(!alpha);

    let mask = app
        .prepare_normalized_mask_file(
            &path,
            PrepareMaskArgs {
                dst_w: 2,
                dst_h: 4,
                kind: MaskSourceKind::NonZero,
                orientation: 6,
                policy: ApplyPolicy::TurnsOnly,
                flip: false,
            },
        )
        .expect("mask");
    assert_eq!(mask.samples.iter().filter(|&&v| v == 1).count(), 4);
}

#[test]
fn session_command_jpeg_exif_orientation_none_override() {
    let app = ApplicationService::start(SessionLimits::for_test());
    let path = fixtures().join("orient6_1x1.jpg");
    let id = match app.submit(Command::new(CommandPayload::PrepareOrientedImage {
        path: path.to_string_lossy().into_owned(),
        orientation: None,
        turns_only: true,
    })) {
        SubmitOutcome::Accepted { id } => id,
        SubmitOutcome::Rejected { error } => panic!("reject: {error}"),
    };
    wait_terminal(&app, id);
    let snap = app.snapshot();
    let replay = app
        .replay(spirula_application::EventCursor::new(snap.session_id, 0))
        .expect("replay");
    let spirula_application::ReplayOutcome::ControlEvents(events) = replay else {
        panic!("resync");
    };
    let o = events.iter().find_map(|e| match &e.event {
        Event::ImagePrepared {
            orientation,
            width,
            height,
            ..
        } if e.command_id == Some(id) => Some((*orientation, *width, *height)),
        _ => None,
    });
    assert_eq!(o, Some((6, 1, 1)));
}

#[test]
fn session_command_missing_path_fails_terminal() {
    let app = ApplicationService::start(SessionLimits::for_test());
    let id = match app.submit(Command::new(CommandPayload::PrepareOrientedImage {
        path: fixtures().join("nope.png").to_string_lossy().into_owned(),
        orientation: Some(1),
        turns_only: true,
    })) {
        SubmitOutcome::Accepted { id } => id,
        SubmitOutcome::Rejected { error } => panic!("reject: {error}"),
    };
    let terminal = wait_terminal(&app, id);
    match terminal.event {
        Event::Completed {
            outcome: OutcomeView::Failed { error },
        } => {
            assert!(
                matches!(
                    error.code(),
                    ErrorCode::NotFound | ErrorCode::ReadFailed | ErrorCode::DecodeFailed
                ),
                "code {:?}",
                error.code()
            );
        }
        other => panic!("expected Failed terminal, got {other:?}"),
    }
}

#[test]
fn session_rejects_image_prepare_while_probe_in_flight() {
    let app = ApplicationService::start(SessionLimits::for_test());
    let load = match app.submit(Command::new(CommandPayload::ProbeLoad {
        label: "busy".into(),
    })) {
        SubmitOutcome::Accepted { id } => id,
        SubmitOutcome::Rejected { error } => panic!("reject: {error}"),
    };
    wait_terminal(&app, load);
    assert!(app.wait_until(Duration::from_secs(2), |s| {
        s.state == spirula_application::SessionState::Ready
    }));
    let _run = match app.submit(Command::new(CommandPayload::ProbeRun { steps: 200 })) {
        SubmitOutcome::Accepted { id } => id,
        SubmitOutcome::Rejected { error } => panic!("reject: {error}"),
    };
    match app.submit(Command::new(CommandPayload::PrepareOrientedImage {
        path: fixtures()
            .join("nonsquare_4x2.png")
            .to_string_lossy()
            .into_owned(),
        orientation: Some(1),
        turns_only: true,
    })) {
        SubmitOutcome::Rejected { error } => {
            assert_eq!(error.code(), ErrorCode::CommandInFlight);
        }
        SubmitOutcome::Accepted { .. } => panic!("expected in-flight reject"),
    }
    let _ = app.submit(Command::new(CommandPayload::Stop { save: false }));
}
