//! Admission and retention size checks for variable payloads.

use spirula_core::{ContextKey, DomainError, ErrorCode, ProgressSnapshot, TerminalOutcome};

use crate::limits::SessionLimits;
use crate::worker::WorkerEvent;

pub(crate) fn reject_oversized(field: &str, actual: usize, max: usize) -> DomainError {
    DomainError::try_new(
        ErrorCode::ValueOutOfRange,
        format!("{field} exceeds size limit"),
    )
    .expect("message")
    .with_context(ContextKey::Field, field)
    .expect("field")
    .with_context(ContextKey::Expected, format!("<= {max} bytes"))
    .expect("expected")
    .with_context(ContextKey::Actual, actual.to_string())
    .expect("actual")
}

pub(crate) fn check_label(label: &str, limits: &SessionLimits) -> Result<(), DomainError> {
    if label.trim().is_empty() {
        return Err(DomainError::try_new(
            ErrorCode::InvalidArgument,
            "probe load label must be non-empty",
        )
        .expect("message")
        .with_context(ContextKey::Field, "label")
        .expect("field"));
    }
    if label.len() > limits.label_max_bytes {
        return Err(reject_oversized(
            "label",
            label.len(),
            limits.label_max_bytes,
        ));
    }
    Ok(())
}

pub(crate) fn check_log_message(message: &str, limits: &SessionLimits) -> Result<(), DomainError> {
    if message.len() > limits.log_message_max_bytes {
        return Err(reject_oversized(
            "log_message",
            message.len(),
            limits.log_message_max_bytes,
        ));
    }
    Ok(())
}

pub(crate) fn check_reason(reason: &str, limits: &SessionLimits) -> Result<(), DomainError> {
    if reason.len() > limits.reason_max_bytes {
        return Err(reject_oversized(
            "reason",
            reason.len(),
            limits.reason_max_bytes,
        ));
    }
    Ok(())
}

pub(crate) fn progress_approx_bytes(snapshot: &ProgressSnapshot) -> usize {
    64 + snapshot
        .metrics
        .iter()
        .map(|m| m.name.len() + 16)
        .sum::<usize>()
}

/// Core progress rules (step≤total, finite unique non-blank metrics) **and**
/// the session byte cap. Callers must use this before enqueue or retention.
pub(crate) fn check_progress(
    snapshot: &ProgressSnapshot,
    limits: &SessionLimits,
) -> Result<(), DomainError> {
    snapshot.validate()?;
    let size = progress_approx_bytes(snapshot);
    if size > limits.progress_max_bytes {
        return Err(reject_oversized(
            "progress",
            size,
            limits.progress_max_bytes,
        ));
    }
    Ok(())
}

/// Mirror `spirula_core::progress::validate_outcome` (private there).
pub(crate) fn validate_worker_terminal(outcome: &TerminalOutcome) -> Result<(), DomainError> {
    if let TerminalOutcome::Failed { error } = outcome
        && error.code() == ErrorCode::Canceled
    {
        return Err(DomainError::try_new(
            ErrorCode::InvalidArgument,
            "cancellation is not a failed outcome",
        )
        .expect("message")
        .with_context(ContextKey::Field, "outcome")
        .expect("field"));
    }
    if let TerminalOutcome::Canceled {
        saved_checkpoint: Some(path),
        ..
    } = outcome
        && path.trim().is_empty()
    {
        return Err(DomainError::try_new(
            ErrorCode::InvalidArgument,
            "canceled outcome has an empty checkpoint path",
        )
        .expect("message")
        .with_context(ContextKey::Field, "checkpoint")
        .expect("field"));
    }
    Ok(())
}

/// Event-visible error copy that fits retention caps, or a bounded substitute.
///
/// The substitute keeps the original stable [`ErrorCode`] (hence category),
/// plus any `retryable_override` and `user_action`. Only message/context are
/// replaced with a short overflow description.
pub(crate) fn bound_error(error: &DomainError, limits: &SessionLimits) -> DomainError {
    let visible = error.without_source();
    if error_fits(&visible, limits) {
        return visible;
    }
    let mut substitute =
        DomainError::try_new(visible.code(), "error payload exceeds retention limit")
            .expect("message");
    if let Some(retryable) = visible.retryable_override() {
        substitute = substitute.with_retryable(retryable);
    }
    if let Some(action) = visible.user_action() {
        substitute = substitute.with_user_action(action);
    }
    substitute
        .with_context(ContextKey::Field, "error_payload")
        .expect("field")
        .with_context(
            ContextKey::Expected,
            format!("<= {} bytes", limits.error_event_max_bytes),
        )
        .expect("expected")
        .with_context(ContextKey::Actual, error_approx_bytes(&visible).to_string())
        .expect("actual")
}

/// Shrink command payloads before control-history retention (reject events).
pub(crate) fn bound_command_payload(
    command: crate::command::CommandPayload,
    limits: &SessionLimits,
) -> crate::command::CommandPayload {
    use crate::command::CommandPayload;
    match command {
        CommandPayload::ProbeLoad { label } if label.len() > limits.label_max_bytes => {
            CommandPayload::ProbeLoad {
                label: omit_label(label.len()),
            }
        }
        other => other,
    }
}

/// Retain a label under the byte cap, or a short omission marker.
pub(crate) fn bound_retained_label(label: String, limits: &SessionLimits) -> String {
    if label.len() > limits.label_max_bytes {
        omit_label(label.len())
    } else {
        label
    }
}

fn omit_label(original_len: usize) -> String {
    format!("[omitted {original_len} bytes]")
}

/// Validate and bound a worker event **before** it enters a mailbox.
///
/// Returns `None` when the event must be dropped (malformed progress/terminal)
/// without closing the active operation. Oversized labels/errors are shrunk
/// in place so the mailbox never retains the original huge payload.
pub(crate) fn sanitize_worker_event(
    event: WorkerEvent,
    limits: &SessionLimits,
) -> Option<WorkerEvent> {
    match event {
        WorkerEvent::Progress {
            run_id,
            command_id,
            snapshot,
        } => {
            check_progress(&snapshot, limits).ok()?;
            Some(WorkerEvent::Progress {
                run_id,
                command_id,
                snapshot,
            })
        }
        WorkerEvent::Log {
            run_id,
            command_id,
            message,
        } => {
            let message = if check_log_message(&message, limits).is_ok() {
                message
            } else {
                omit_label(message.len())
            };
            Some(WorkerEvent::Log {
                run_id,
                command_id,
                message,
            })
        }
        WorkerEvent::LoadDone {
            run_id,
            command_id,
            label,
        } => Some(WorkerEvent::LoadDone {
            run_id,
            command_id,
            label: bound_retained_label(label, limits),
        }),
        WorkerEvent::RunTerminal {
            run_id,
            command_id,
            outcome,
            checkpoint_label,
            save_failed,
            final_progress,
        } => {
            validate_worker_terminal(&outcome).ok()?;
            let outcome = bound_terminal_outcome(outcome, limits);
            let checkpoint_label =
                checkpoint_label.map(|label| bound_retained_label(label, limits));
            let final_progress = final_progress
                .and_then(|snapshot| check_progress(&snapshot, limits).ok().map(|_| snapshot));
            Some(WorkerEvent::RunTerminal {
                run_id,
                command_id,
                outcome,
                checkpoint_label,
                save_failed,
                final_progress,
            })
        }
    }
}

fn bound_terminal_outcome(outcome: TerminalOutcome, limits: &SessionLimits) -> TerminalOutcome {
    match outcome {
        TerminalOutcome::Completed { step } => TerminalOutcome::Completed { step },
        TerminalOutcome::Canceled {
            step,
            reason,
            saved_checkpoint,
        } => TerminalOutcome::Canceled {
            step,
            reason,
            saved_checkpoint: saved_checkpoint.map(|label| bound_retained_label(label, limits)),
        },
        TerminalOutcome::Failed { error } => TerminalOutcome::Failed {
            error: bound_error(&error, limits),
        },
    }
}

fn error_fits(error: &DomainError, limits: &SessionLimits) -> bool {
    if error.message().len() > limits.error_message_max_bytes {
        return false;
    }
    for (_, value) in error.context() {
        if value.len() > limits.context_value_max_bytes {
            return false;
        }
    }
    error_approx_bytes(error) <= limits.error_event_max_bytes
}

pub(crate) fn error_approx_bytes(error: &DomainError) -> usize {
    64 + error.message().len()
        + error
            .context()
            .iter()
            .map(|(k, v)| k.as_str().len() + v.len() + 8)
            .sum::<usize>()
}
