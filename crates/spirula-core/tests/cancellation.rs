use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use spirula_core::{
    CancelReason, CancelToken, ContextKey, DomainError, ErrorCode, OutcomeView, ProgressMetric,
    ProgressPublisher, ProgressSnapshot, TerminalOutcome,
};

fn snapshot(
    step: u64,
    total: Option<u64>,
    metrics: Vec<ProgressMetric>,
) -> Result<ProgressSnapshot, DomainError> {
    ProgressSnapshot::try_new(step, total, 3, 10, None, metrics)
}

#[test]
fn first_reason_is_kept_and_check_is_stable() {
    let token = CancelToken::new();
    assert!(token.check().is_ok());
    assert!(!token.is_cancelled());
    assert!(token.request(CancelReason::User));
    assert!(!token.request(CancelReason::Shutdown));
    assert!(!token.request(CancelReason::User));
    assert!(token.is_cancelled());
    assert_eq!(token.reason(), Some(CancelReason::User));
    let error = token.check().unwrap_err();
    assert_eq!(error.code(), ErrorCode::Canceled);
    assert!(!error.retryable());
    assert_eq!(error.context_value(ContextKey::Actual), Some("user"));
    assert_eq!(token.check().unwrap_err().code(), error.code());
}

#[test]
fn cloned_token_observes_another_thread() {
    let token = CancelToken::new();
    let worker = token.clone();
    thread::spawn(move || {
        assert!(worker.request(CancelReason::Shutdown));
    })
    .join()
    .unwrap();
    assert_eq!(token.reason(), Some(CancelReason::Shutdown));
    assert_eq!(
        token.check().unwrap_err().context_value(ContextKey::Actual),
        Some("shutdown")
    );
}

#[test]
fn concurrent_requests_publish_one_reason_atomically() {
    let token = CancelToken::new();
    let start = Arc::new(Barrier::new(9));
    let mut handles = Vec::new();
    for reason in [
        CancelReason::User,
        CancelReason::Shutdown,
        CancelReason::Timeout,
        CancelReason::Superseded,
    ] {
        for _ in 0..2 {
            let token = token.clone();
            let start = start.clone();
            handles.push(thread::spawn(move || {
                start.wait();
                token.request(reason)
            }));
        }
    }
    let observer_token = token.clone();
    let observer_start = start.clone();
    let violation = Arc::new(AtomicBool::new(false));
    let observer_violation = violation.clone();
    let observer = thread::spawn(move || {
        observer_start.wait();
        for _ in 0..10_000 {
            if observer_token.is_cancelled() && observer_token.reason().is_none() {
                observer_violation.store(true, Ordering::Relaxed);
                return;
            }
        }
    });
    let wins = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .filter(|won| *won)
        .count();
    observer.join().unwrap();
    assert!(!violation.load(Ordering::Relaxed));
    assert_eq!(wins, 1);
    let reason = token.reason().unwrap();
    assert!(token.is_cancelled());
    assert_eq!(
        token.check().unwrap_err().context_value(ContextKey::Actual),
        Some(reason.as_str())
    );
}

#[test]
fn progress_accepts_indeterminate_totals_and_rejects_bad_bounds() {
    let open = snapshot(0, None, Vec::new()).unwrap();
    assert!(open.validate().is_ok());
    assert!(snapshot(0, Some(0), Vec::new()).is_ok());
    assert!(snapshot(4, Some(4), Vec::new()).is_ok());
    let past = snapshot(5, Some(4), Vec::new()).unwrap_err();
    assert_eq!(past.code(), ErrorCode::ValueOutOfRange);
    assert_eq!(past.context_value(ContextKey::Field), Some("step"));

    let blank = snapshot(
        1,
        Some(2),
        vec![ProgressMetric {
            name: " ".to_owned(),
            value: 1.0,
        }],
    )
    .unwrap_err();
    assert_eq!(blank.code(), ErrorCode::InvalidArgument);
    let infinite = snapshot(
        1,
        Some(2),
        vec![ProgressMetric {
            name: "loss".to_owned(),
            value: f64::INFINITY,
        }],
    )
    .unwrap_err();
    assert_eq!(infinite.code(), ErrorCode::ValueOutOfRange);
    let duplicate = snapshot(
        1,
        Some(2),
        vec![
            ProgressMetric {
                name: "loss".to_owned(),
                value: 1.0,
            },
            ProgressMetric {
                name: "loss".to_owned(),
                value: 2.0,
            },
        ],
    )
    .unwrap_err();
    assert_eq!(duplicate.code(), ErrorCode::InvalidArgument);
    assert_eq!(duplicate.context_value(ContextKey::Actual), Some("loss"));
}

#[test]
fn publisher_keeps_one_terminal_and_rejects_late_progress() {
    let publisher = ProgressPublisher::new();
    let first = snapshot(
        1,
        None,
        vec![ProgressMetric {
            name: "loss".to_owned(),
            value: 0.5,
        }],
    )
    .unwrap();
    publisher.publish_progress(first.clone()).unwrap();
    let second = snapshot(2, Some(3), Vec::new()).unwrap();
    publisher.publish_progress(second.clone()).unwrap();
    assert_eq!(publisher.latest().unwrap(), second);

    publisher
        .publish_terminal(TerminalOutcome::Completed { step: 3 })
        .unwrap();
    let late = publisher.publish_progress(second).unwrap_err();
    assert_eq!(late.code(), ErrorCode::InvalidState);
    assert_eq!(late.context_value(ContextKey::Field), Some("progress"));
    let duplicate = publisher
        .publish_terminal(TerminalOutcome::Failed {
            error: DomainError::try_new(ErrorCode::ReadFailed, "disk").unwrap(),
        })
        .unwrap_err();
    assert_eq!(duplicate.code(), ErrorCode::InvalidState);
    assert_eq!(
        publisher.outcome(),
        Some(OutcomeView::Completed { step: 3 })
    );
    assert_eq!(publisher.latest().unwrap().step, 2);

    let canceled = ProgressPublisher::new();
    canceled
        .publish_terminal(TerminalOutcome::Canceled {
            step: 0,
            reason: CancelReason::Timeout,
            saved_checkpoint: Some("step-000000000.ckpt".to_owned()),
        })
        .unwrap();
    assert!(canceled.publish_progress(first).is_err());
    assert_eq!(
        canceled.outcome(),
        Some(OutcomeView::Canceled {
            step: 0,
            reason: CancelReason::Timeout,
            saved_checkpoint: Some("step-000000000.ckpt".to_owned()),
        })
    );

    let failed = ProgressPublisher::new();
    failed
        .publish_terminal(TerminalOutcome::Failed {
            error: DomainError::try_new(ErrorCode::DecodeFailed, "bad image").unwrap(),
        })
        .unwrap();
    assert!(
        failed
            .publish_terminal(TerminalOutcome::Completed { step: 1 })
            .is_err()
    );
    assert_eq!(
        failed.outcome(),
        Some(OutcomeView::Failed {
            code: ErrorCode::DecodeFailed,
            message: "bad image".to_owned(),
        })
    );

    let blank_path = ProgressPublisher::new()
        .publish_terminal(TerminalOutcome::Canceled {
            step: 1,
            reason: CancelReason::Superseded,
            saved_checkpoint: Some("  ".to_owned()),
        })
        .unwrap_err();
    assert_eq!(blank_path.code(), ErrorCode::InvalidArgument);
    assert_eq!(
        blank_path.context_value(ContextKey::Field),
        Some("checkpoint")
    );
}
