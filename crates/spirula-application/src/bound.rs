//! Admission and retention size checks for variable payloads.

use spirula_core::{ContextKey, DomainError, ErrorCode, ProgressSnapshot};

use crate::limits::SessionLimits;

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

pub(crate) fn check_progress(
    snapshot: &ProgressSnapshot,
    limits: &SessionLimits,
) -> Result<(), DomainError> {
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

/// Event-visible error copy that fits retention caps, or a bounded substitute.
///
/// The substitute **keeps the original stable [`ErrorCode`]**; only the message
/// and context are replaced with a short overflow description.
pub(crate) fn bound_error(error: &DomainError, limits: &SessionLimits) -> DomainError {
    let visible = error.without_source();
    if error_fits(&visible, limits) {
        return visible;
    }
    DomainError::try_new(visible.code(), "error payload exceeds retention limit")
        .expect("message")
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
