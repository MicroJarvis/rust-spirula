//! Independent lifecycle review. Injected commit results test owner arbitration;
//! real PNG production is covered independently by the CLI review subprocesses.
use std::time::Duration;

use spirula_application::{
    ApplicationService, Command, CommandId, CommandPayload, CommandResult, Event, EventCursor,
    PreviewSummary, ReplayOutcome, SessionLimits, SubmitOutcome, WorkerEvent,
};
use spirula_core::OutcomeView;

fn submit(app: &ApplicationService, payload: CommandPayload) -> CommandId {
    match app.submit(Command::new(payload)) {
        SubmitOutcome::Accepted { id } => id,
        SubmitOutcome::Rejected { error } => panic!("admission failed: {error}"),
    }
}

fn preview(app: &ApplicationService) -> CommandId {
    let id = submit(
        app,
        CommandPayload::Preview {
            request_path: "held-request.json".into(),
            output_path: "held-output.png".into(),
        },
    );
    assert!(app.wait_until(Duration::from_secs(2), |s| {
        s.active_command == Some(id) && s.active_run.is_some()
    }));
    id
}

fn terminal(app: &ApplicationService, id: CommandId) -> Event {
    assert!(app.wait_until(Duration::from_secs(2), |_| {
        matches!(app.command_result(id), CommandResult::Ready(_))
    }));
    match app.command_result(id) {
        CommandResult::Ready(envelope) => envelope.event,
        other => panic!("missing terminal: {other:?}"),
    }
}

fn history(app: &ApplicationService) -> Vec<spirula_application::EventEnvelope> {
    match app
        .replay(EventCursor::new(app.snapshot().session_id, 0))
        .unwrap()
    {
        ReplayOutcome::ControlEvents(events) => events,
        other => panic!("unexpected replay: {other:?}"),
    }
}

#[test]
fn stop_before_work_cancels_preview_and_stop_has_its_own_terminal() {
    let app = ApplicationService::start(SessionLimits::for_test());
    let gate = app.stall_gate();
    gate.set_stalled(true);
    let id = preview(&app);
    let stop = submit(&app, CommandPayload::Stop { save: false });
    assert!(matches!(
        terminal(&app, id),
        Event::Completed {
            outcome: OutcomeView::Canceled { .. }
        }
    ));
    let _ = terminal(&app, stop);
    gate.set_stalled(false);
    let events = history(&app);
    assert!(!events.iter().any(|e| {
        e.command_id == Some(id) && matches!(e.event, Event::PreviewPublished { .. })
    }));
    for command_id in [id, stop] {
        assert_eq!(
            events
                .iter()
                .filter(|e| {
                    e.command_id == Some(command_id) && matches!(e.event, Event::Completed { .. })
                })
                .count(),
            1,
            "exactly one terminal for {command_id:?}"
        );
    }
}

#[test]
fn shutdown_before_work_cancels_preview_without_published_event() {
    let app = ApplicationService::start(SessionLimits::for_test());
    let gate = app.stall_gate();
    gate.set_stalled(true);
    let id = preview(&app);
    let shutdown = submit(&app, CommandPayload::Shutdown);
    assert!(matches!(
        terminal(&app, id),
        Event::Completed {
            outcome: OutcomeView::Canceled { .. }
        }
    ));
    let _ = terminal(&app, shutdown);
    gate.set_stalled(false);
    assert!(!history(&app).iter().any(|e| {
        e.command_id == Some(id) && matches!(e.event, Event::PreviewPublished { .. })
    }));
}

#[test]
fn published_commit_wins_late_stop_and_duplicate_result_keeps_one_terminal() {
    let app = ApplicationService::start(SessionLimits::for_test());
    let gate = app.stall_gate();
    gate.set_stalled(true);
    let id = preview(&app);
    let run_id = app.snapshot().active_run.unwrap();
    let summary = PreviewSummary {
        path: "committed.png".into(),
        width: 3,
        height: 2,
        splat_count: 1,
        rgb8_bytes: 18,
        png_bytes: 80,
        wall_load_ns: 1,
        wall_render_ns: 2,
        wall_encode_ns: 3,
        wall_publish_ns: 4,
    };
    let committed = WorkerEvent::PreviewDone {
        run_id,
        command_id: id,
        summary: summary.clone(),
        error: None,
        published: true,
    };
    // Whether the owner sees Stop or the result first, a committed publication
    // must retain Completed and each command retains its own identity.
    app.inject_worker_event(committed).unwrap();
    let stop = submit(&app, CommandPayload::Stop { save: false });
    assert!(matches!(
        terminal(&app, id),
        Event::Completed {
            outcome: OutcomeView::Completed { .. }
        }
    ));
    let _ = terminal(&app, stop);
    app.inject_worker_event(WorkerEvent::PreviewDone {
        run_id,
        command_id: id,
        summary,
        error: None,
        published: true,
    })
    .unwrap();
    gate.set_stalled(false);
    // A fresh admitted command creates an owner-side processing barrier.
    let retry = submit(
        &app,
        CommandPayload::Preview {
            request_path: "nonexistent-review-request.json".into(),
            output_path: "unused-review-output.png".into(),
        },
    );
    assert!(matches!(
        terminal(&app, retry),
        Event::Completed {
            outcome: OutcomeView::Failed { .. }
        }
    ));
    let events = history(&app);
    for command_id in [id, stop, retry] {
        assert_eq!(
            events
                .iter()
                .filter(|e| {
                    e.command_id == Some(command_id) && matches!(e.event, Event::Completed { .. })
                })
                .count(),
            1
        );
    }
    assert_eq!(
        events
            .iter()
            .filter(|e| {
                e.command_id == Some(id)
                    && e.run_id == Some(run_id)
                    && matches!(e.event, Event::PreviewPublished { .. })
            })
            .count(),
        1
    );
}
