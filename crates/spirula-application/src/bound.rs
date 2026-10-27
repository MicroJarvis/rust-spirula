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

/// Filesystem path for P3-F02 image commands (independent of probe label cap).
pub(crate) const IMAGE_PATH_MAX_BYTES: usize = 4096;

pub(crate) fn check_image_path(path: &str) -> Result<(), DomainError> {
    if path.trim().is_empty() {
        return Err(DomainError::try_new(
            ErrorCode::InvalidArgument,
            "image path must be non-empty",
        )
        .expect("message")
        .with_context(ContextKey::Field, "path")
        .expect("field"));
    }
    if path.len() > IMAGE_PATH_MAX_BYTES {
        return Err(reject_oversized("path", path.len(), IMAGE_PATH_MAX_BYTES));
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

/// Short overflow substitute message. Kept tiny so it always fits floors.
pub(crate) const OVERSIZED_ERROR_MESSAGE: &str = "oversized";

/// Minimum `error_event_max_bytes` that can hold a bare fallback (code + short
/// message, no context). Used by [`SessionLimits::normalized`].
pub(crate) fn min_fallback_error_event_bytes() -> usize {
    64 + OVERSIZED_ERROR_MESSAGE.len()
}

/// Event-visible error copy that fits retention caps, or a bounded substitute.
///
/// The substitute keeps the original stable [`ErrorCode`] (hence category),
/// plus any `retryable_override` and `user_action`. Message/context shrink
/// until the view fits `error_event_max_bytes` (never retain an oversized
/// fallback).
pub(crate) fn bound_error(error: &DomainError, limits: &SessionLimits) -> DomainError {
    let visible = error.without_source();
    if error_fits(&visible, limits) {
        return visible;
    }
    let mut substitute =
        DomainError::try_new(visible.code(), OVERSIZED_ERROR_MESSAGE).expect("message");
    if let Some(retryable) = visible.retryable_override() {
        substitute = substitute.with_retryable(retryable);
    }
    if let Some(action) = visible.user_action() {
        substitute = substitute.with_user_action(action);
    }
    // Prefer preserving an original Field (e.g. disk_log non-recursive marker),
    // then a generic overflow marker — only while the result still fits.
    let mut field_candidates: Vec<&str> = Vec::with_capacity(2);
    if let Some(existing) = visible.context_value(ContextKey::Field) {
        field_candidates.push(existing);
    }
    field_candidates.push("error_payload");
    for field in field_candidates {
        if let Ok(with_field) = DomainError::try_new(visible.code(), OVERSIZED_ERROR_MESSAGE)
            .expect("message")
            .with_context(ContextKey::Field, field)
        {
            let mut with_field = with_field;
            if let Some(retryable) = visible.retryable_override() {
                with_field = with_field.with_retryable(retryable);
            }
            if let Some(action) = visible.user_action() {
                with_field = with_field.with_user_action(action);
            }
            if error_fits(&with_field, limits) {
                return with_field;
            }
        }
    }
    debug_assert!(
        error_approx_bytes(&substitute) <= limits.error_event_max_bytes
            || limits.error_event_max_bytes < min_fallback_error_event_bytes(),
        "fallback must fit normalized error_event_max_bytes"
    );
    substitute
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
        CommandPayload::PrepareOrientedImage {
            path,
            orientation,
            turns_only,
        } if path.len() > limits.label_max_bytes => CommandPayload::PrepareOrientedImage {
            path: omit_label(path.len()),
            orientation,
            turns_only,
        },
        CommandPayload::PrepareNormalizedMask {
            path,
            dst_w,
            dst_h,
            orientation,
            turns_only,
            flip,
            alpha,
        } if path.len() > limits.label_max_bytes => CommandPayload::PrepareNormalizedMask {
            path: omit_label(path.len()),
            dst_w,
            dst_h,
            orientation,
            turns_only,
            flip,
            alpha,
        },
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
            // Present final_progress must validate; never strip-and-close.
            if let Some(ref snapshot) = final_progress {
                check_progress(snapshot, limits).ok()?;
            }
            // `save_failed` only applies to Canceled (§5). Ignore it on Failed
            // so a Failed terminal cannot be forced into Ready.
            let save_failed = save_failed && matches!(outcome, TerminalOutcome::Canceled { .. });
            // One canonical checkpoint label for CheckpointSaved + terminal.
            let (outcome, checkpoint_label) =
                align_canceled_checkpoint(outcome, checkpoint_label, limits);
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

/// Owner-side check: present `final_progress` must validate (never
/// strip-and-close). `save_failed` with non-Canceled is ignored upstream.
pub(crate) fn validate_run_terminal_bundle(
    outcome: &TerminalOutcome,
    save_failed: bool,
    final_progress: Option<&ProgressSnapshot>,
    limits: &SessionLimits,
) -> Result<(), DomainError> {
    let _ = (outcome, save_failed);
    if let Some(snapshot) = final_progress {
        check_progress(snapshot, limits)?;
    }
    Ok(())
}

/// Prefer `saved_checkpoint` when both labels are present; keep CheckpointSaved
/// and the terminal outcome on the same string.
fn align_canceled_checkpoint(
    outcome: TerminalOutcome,
    checkpoint_label: Option<String>,
    limits: &SessionLimits,
) -> (TerminalOutcome, Option<String>) {
    match outcome {
        TerminalOutcome::Canceled {
            step,
            reason,
            saved_checkpoint,
        } => {
            let canonical = match (saved_checkpoint, checkpoint_label) {
                (Some(saved), Some(label)) if saved == label => Some(saved),
                (Some(saved), Some(_)) => Some(saved), // disagree → outcome wins
                (Some(saved), None) => Some(saved),
                (None, Some(label)) => Some(label),
                (None, None) => None,
            }
            .map(|label| bound_retained_label(label, limits));
            (
                TerminalOutcome::Canceled {
                    step,
                    reason,
                    saved_checkpoint: canonical.clone(),
                },
                canonical,
            )
        }
        TerminalOutcome::Failed { error } => (
            TerminalOutcome::Failed {
                error: bound_error(&error, limits),
            },
            None,
        ),
        other => (other, None),
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
