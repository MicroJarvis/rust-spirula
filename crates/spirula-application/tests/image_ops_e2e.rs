//! End-to-end: application/session command → spirula-io image orient/mask (P3-F02).

use std::path::PathBuf;
use std::time::Duration;

use spirula_application::{
    ApplicationService, Command, CommandPayload, CommandResult, Event, ImagePrepareOk,
    PrepareMaskArgs, SessionLimits, SubmitOutcome, WorkerEvent,
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
        boundary_offset_milli: 0,
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
                boundary_offset_frac: 0.0,
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
fn stop_cancels_image_prepare_without_prepared_event() {
    // StallGate holds decode so Stop is admitted and cancel wins first.
    let app = ApplicationService::start(SessionLimits::for_test());
    let gate = app.stall_gate();
    gate.set_stalled(true);
    let path = fixtures()
        .join("nonsquare_4x2.png")
        .to_string_lossy()
        .into_owned();
    let id = match app.submit(Command::new(CommandPayload::PrepareOrientedImage {
        path,
        orientation: Some(1),
        turns_only: true,
    })) {
        SubmitOutcome::Accepted { id } => id,
        SubmitOutcome::Rejected { error } => panic!("reject: {error}"),
    };
    assert!(app.wait_until(Duration::from_secs(1), |s| s.active_command == Some(id)));
    let _stop = match app.submit(Command::new(CommandPayload::Stop { save: false })) {
        SubmitOutcome::Accepted { id } => id,
        SubmitOutcome::Rejected { error } => panic!("stop reject: {error}"),
    };
    let terminal = wait_terminal(&app, id);
    match terminal.event {
        Event::Completed {
            outcome: OutcomeView::Canceled { .. },
        } => {}
        other => panic!("expected Canceled, got {other:?}"),
    }
    gate.set_stalled(false);
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
            .any(|e| matches!(e.event, Event::ImagePrepared { .. }) && e.command_id == Some(id)),
        "no ImagePrepared after cancel"
    );
}

#[test]
fn shutdown_cancels_held_image_prepare() {
    let app = ApplicationService::start(SessionLimits::for_test());
    let gate = app.stall_gate();
    gate.set_stalled(true);
    let id = match app.submit(Command::new(CommandPayload::PrepareNormalizedMask {
        path: fixtures()
            .join("mask_4x2.png")
            .to_string_lossy()
            .into_owned(),
        dst_w: 2,
        dst_h: 2,
        orientation: 1,
        turns_only: true,
        flip: false,
        alpha: false,
        boundary_offset_milli: 0,
    })) {
        SubmitOutcome::Accepted { id } => id,
        SubmitOutcome::Rejected { error } => panic!("reject: {error}"),
    };
    assert!(app.wait_until(Duration::from_secs(1), |s| s.active_command == Some(id)));
    let _ = app.submit(Command::new(CommandPayload::Shutdown));
    let terminal = wait_terminal(&app, id);
    assert!(matches!(
        terminal.event,
        Event::Completed {
            outcome: OutcomeView::Canceled { .. }
        }
    ));
    gate.set_stalled(false);
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
            .any(|e| matches!(e.event, Event::MaskPrepared { .. }) && e.command_id == Some(id))
    );
}

#[test]
fn slow_subscriber_does_not_block_image_cancel_recovery() {
    let mut limits = SessionLimits::for_test();
    limits.notification_queue_capacity = 1;
    let app = ApplicationService::start(limits);
    let (_sub, _) = app.subscribe().expect("subscribe");
    // Do not drain — stall the live subscriber.
    let gate = app.stall_gate();
    gate.set_stalled(true);
    let id = match app.submit(Command::new(CommandPayload::PrepareOrientedImage {
        path: fixtures()
            .join("nonsquare_4x2.png")
            .to_string_lossy()
            .into_owned(),
        orientation: Some(1),
        turns_only: true,
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
    gate.set_stalled(false);
    // Recovery: a fresh prepare must complete with Prepared after cancel.
    let id2 = match app.submit(Command::new(CommandPayload::PrepareOrientedImage {
        path: fixtures()
            .join("nonsquare_4x2.png")
            .to_string_lossy()
            .into_owned(),
        orientation: Some(1),
        turns_only: true,
    })) {
        SubmitOutcome::Accepted { id } => id,
        SubmitOutcome::Rejected { error } => panic!("reject: {error}"),
    };
    let terminal2 = wait_terminal(&app, id2);
    assert!(matches!(
        terminal2.event,
        Event::Completed {
            outcome: OutcomeView::Completed { .. }
        }
    ));
}

fn assert_no_prepared(app: &ApplicationService, id: spirula_application::CommandId) {
    let snap = app.snapshot();
    let replay = app
        .replay(spirula_application::EventCursor::new(snap.session_id, 0))
        .expect("replay");
    let spirula_application::ReplayOutcome::ControlEvents(events) = replay else {
        panic!("resync");
    };
    assert!(
        !events.iter().any(|e| {
            e.command_id == Some(id)
                && matches!(
                    e.event,
                    Event::ImagePrepared { .. } | Event::MaskPrepared { .. }
                )
        }),
        "Prepared must not publish after cancel"
    );
}

fn assert_canceled(app: &ApplicationService, id: spirula_application::CommandId) {
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
fn owner_arbitrates_stop_over_queued_image_prepare_err() {
    // Independent cancel_race harness: Stop wins, then a late ImagePrepareDone
    // Err must not become Failed — owner cancels under the state lock.
    let app = ApplicationService::start(SessionLimits::for_test());
    let gate = app.stall_gate();
    gate.set_stalled(true);
    let id = match app.submit(Command::new(CommandPayload::PrepareOrientedImage {
        path: fixtures()
            .join("nonsquare_4x2.png")
            .to_string_lossy()
            .into_owned(),
        orientation: Some(1),
        turns_only: true,
    })) {
        SubmitOutcome::Accepted { id } => id,
        SubmitOutcome::Rejected { error } => panic!("reject: {error}"),
    };
    assert!(app.wait_until(Duration::from_secs(1), |s| {
        s.active_command == Some(id) && s.active_run.is_some()
    }));
    let rid = app.snapshot().active_run.expect("active_run");
    let _ = app.submit(Command::new(CommandPayload::Stop { save: false }));
    app.inject_worker_event(WorkerEvent::ImagePrepareDone {
        run_id: rid,
        command_id: id,
        result: Err(spirula_core::DomainError::try_new(
            ErrorCode::ReadFailed,
            "queued result after stop",
        )
        .expect("msg")),
    })
    .expect("inject result mailbox");
    assert!(app.wait_until(Duration::from_secs(2), |_| {
        matches!(app.command_result(id), CommandResult::Ready(_))
    }));
    assert_canceled(&app, id);
    assert_no_prepared(&app, id);
    gate.set_stalled(false);
    // Late duplicate result after terminal is ignored.
    let _ = app.inject_worker_event(WorkerEvent::ImagePrepareDone {
        run_id: rid,
        command_id: id,
        result: Ok(ImagePrepareOk::Oriented {
            path: "late".into(),
            width: 1,
            height: 1,
            orientation: 1,
            turns_only: true,
        }),
    });
    assert_canceled(&app, id);
    assert_no_prepared(&app, id);
    // Recovery: fresh prepare completes.
    let id2 = match app.submit(Command::new(CommandPayload::PrepareOrientedImage {
        path: fixtures()
            .join("nonsquare_4x2.png")
            .to_string_lossy()
            .into_owned(),
        orientation: Some(1),
        turns_only: true,
    })) {
        SubmitOutcome::Accepted { id } => id,
        SubmitOutcome::Rejected { error } => panic!("reject: {error}"),
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
fn owner_arbitrates_shutdown_over_queued_image_prepare_ok() {
    let app = ApplicationService::start(SessionLimits::for_test());
    let gate = app.stall_gate();
    gate.set_stalled(true);
    let id = match app.submit(Command::new(CommandPayload::PrepareNormalizedMask {
        path: fixtures()
            .join("mask_4x2.png")
            .to_string_lossy()
            .into_owned(),
        dst_w: 2,
        dst_h: 2,
        orientation: 1,
        turns_only: true,
        flip: false,
        alpha: false,
        boundary_offset_milli: 0,
    })) {
        SubmitOutcome::Accepted { id } => id,
        SubmitOutcome::Rejected { error } => panic!("reject: {error}"),
    };
    assert!(app.wait_until(Duration::from_secs(1), |s| {
        s.active_command == Some(id) && s.active_run.is_some()
    }));
    let rid = app.snapshot().active_run.expect("active_run");
    let _ = app.submit(Command::new(CommandPayload::Shutdown));
    app.inject_worker_event(WorkerEvent::ImagePrepareDone {
        run_id: rid,
        command_id: id,
        result: Ok(ImagePrepareOk::Mask {
            path: "should-not-publish".into(),
            width: 2,
            height: 2,
            kept: 4,
            alpha: false,
        }),
    })
    .expect("inject");
    assert!(app.wait_until(Duration::from_secs(2), |_| {
        matches!(app.command_result(id), CommandResult::Ready(_))
    }));
    assert_canceled(&app, id);
    assert_no_prepared(&app, id);
    gate.set_stalled(false);
}

#[test]
fn wrong_image_prepare_ok_kind_does_not_close_command() {
    let app = ApplicationService::start(SessionLimits::for_test());
    let gate = app.stall_gate();
    gate.set_stalled(true);
    let id = match app.submit(Command::new(CommandPayload::PrepareOrientedImage {
        path: fixtures()
            .join("nonsquare_4x2.png")
            .to_string_lossy()
            .into_owned(),
        orientation: Some(1),
        turns_only: true,
    })) {
        SubmitOutcome::Accepted { id } => id,
        SubmitOutcome::Rejected { error } => panic!("reject: {error}"),
    };
    assert!(app.wait_until(Duration::from_secs(1), |s| {
        s.active_command == Some(id) && s.active_run.is_some()
    }));
    let rid = app.snapshot().active_run.expect("active_run");
    // Mask Ok while Oriented is active must be dropped as stale.
    app.inject_worker_event(WorkerEvent::ImagePrepareDone {
        run_id: rid,
        command_id: id,
        result: Ok(ImagePrepareOk::Mask {
            path: "wrong-kind".into(),
            width: 1,
            height: 1,
            kept: 0,
            alpha: false,
        }),
    })
    .expect("inject");
    std::thread::sleep(Duration::from_millis(50));
    assert!(matches!(app.command_result(id), CommandResult::Pending));
    let _ = app.submit(Command::new(CommandPayload::Stop { save: false }));
    assert!(app.wait_until(Duration::from_secs(2), |_| {
        matches!(app.command_result(id), CommandResult::Ready(_))
    }));
    assert_canceled(&app, id);
    assert_no_prepared(&app, id);
    gate.set_stalled(false);
}

#[test]
fn oversized_mask_dst_rejected_at_admission() {
    let app = ApplicationService::start(SessionLimits::for_test());
    match app.submit(Command::new(CommandPayload::PrepareNormalizedMask {
        path: fixtures()
            .join("mask_4x2.png")
            .to_string_lossy()
            .into_owned(),
        dst_w: u32::MAX,
        dst_h: u32::MAX,
        orientation: 1,
        turns_only: true,
        flip: false,
        alpha: false,
        boundary_offset_milli: 0,
    })) {
        SubmitOutcome::Rejected { error } => {
            assert_eq!(error.code(), ErrorCode::ValueOutOfRange);
        }
        SubmitOutcome::Accepted { .. } => panic!("u32::MAX dst must reject before alloc"),
    }
}

#[test]
fn sync_and_admitted_prepare_tga_preserves_ext_hint() {
    // TGA has no reliable magic; cancelable byte decode must keep path hint.
    let app = ApplicationService::start(SessionLimits::for_test());
    let path = fixtures().join("tiny_2x2.tga");
    let oriented = app
        .prepare_oriented_rgb_file(&path, Some(1), ApplyPolicy::TurnsOnly)
        .expect("sync TGA prepare");
    assert_eq!((oriented.width, oriented.height), (2, 2));
    assert_eq!(oriented.rgb.len(), 2 * 2 * 3);

    let path_s = path.to_string_lossy().into_owned();
    let id = match app.submit(Command::new(CommandPayload::PrepareOrientedImage {
        path: path_s.clone(),
        orientation: Some(1),
        turns_only: true,
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
        panic!("resync");
    };
    let prepared = events.iter().find_map(|e| match &e.event {
        Event::ImagePrepared {
            path: p,
            width,
            height,
            ..
        } if e.command_id == Some(id) => Some((p.clone(), *width, *height)),
        _ => None,
    });
    let (p, w, h) = prepared.expect("ImagePrepared for TGA");
    assert_eq!(p, path_s);
    assert_eq!((w, h), (2, 2));
}

#[test]
fn sync_and_admitted_prepare_tga_mask_preserves_ext_hint() {
    let app = ApplicationService::start(SessionLimits::for_test());
    let path = fixtures().join("tiny_2x2.tga");
    let args = PrepareMaskArgs {
        dst_w: 2,
        dst_h: 2,
        kind: MaskSourceKind::NonZero,
        orientation: 1,
        policy: ApplyPolicy::TurnsOnly,
        flip: false,
        boundary_offset_frac: 0.0,
    };
    let mask = app
        .prepare_normalized_mask_file(&path, args)
        .expect("sync TGA mask prepare");
    assert_eq!((mask.width, mask.height), (2, 2));
    assert_eq!(mask.samples.len(), 4);

    let path_s = path.to_string_lossy().into_owned();
    let id = match app.submit(Command::new(CommandPayload::PrepareNormalizedMask {
        path: path_s.clone(),
        dst_w: 2,
        dst_h: 2,
        orientation: 1,
        turns_only: true,
        flip: false,
        alpha: false,
        boundary_offset_milli: 0,
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
        panic!("resync");
    };
    let prepared = events.iter().find_map(|e| match &e.event {
        Event::MaskPrepared {
            path: p,
            width,
            height,
            ..
        } if e.command_id == Some(id) => Some((p.clone(), *width, *height)),
        _ => None,
    });
    let (p, w, h) = prepared.expect("MaskPrepared for TGA");
    assert_eq!(p, path_s);
    assert_eq!((w, h), (2, 2));
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
