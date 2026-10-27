//! P2-F05: GPU-local diagnostic events (not application session history).

use std::collections::VecDeque;

use spirula_core::{ContextKey, DomainError, ErrorCode, UserAction};

use crate::context::{ContextGeneration, ContextId};
use crate::resource::ResourceLabel;

/// Default retained diagnostic events per live context generation.
pub const DEFAULT_DIAGNOSTIC_CAPACITY: usize = 64;
/// Per-field UTF-8 byte ceilings for retained diagnostic strings.
pub const MAX_DIAGNOSTIC_LABEL_BYTES: usize = 256;
pub const MAX_DIAGNOSTIC_CODE_BYTES: usize = 64;
pub const MAX_DIAGNOSTIC_MESSAGE_BYTES: usize = 1024;
/// Soft cap on label+code+message bytes after per-field truncation.
pub const MAX_DIAGNOSTIC_PAYLOAD_BYTES: usize = 1280;

/// Truncate `s` to at most `max_bytes` on a UTF-8 char boundary.
/// Returns `(truncated, was_truncated)`.
pub fn utf8_truncate(s: &str, max_bytes: usize) -> (String, bool) {
    if s.len() <= max_bytes {
        return (s.to_owned(), false);
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    (s[..end].to_owned(), true)
}

/// GPU operation tag carried on errors and diagnostic events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GpuOperation {
    Open,
    Create,
    Upload,
    Dispatch,
    Readback,
    Submit,
    Wait,
    Map,
    Destroy,
    Close,
    Reopen,
    Inject,
    Other,
}

impl GpuOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Create => "create",
            Self::Upload => "upload",
            Self::Dispatch => "dispatch",
            Self::Readback => "readback",
            Self::Submit => "submit",
            Self::Wait => "wait",
            Self::Map => "map",
            Self::Destroy => "destroy",
            Self::Close => "close",
            Self::Reopen => "reopen",
            Self::Inject => "inject",
            Self::Other => "other",
        }
    }
}

/// Coarse diagnostic kind for filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagnosticKind {
    Error,
    DeviceLost,
    Warning,
    Info,
}

impl DiagnosticKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::DeviceLost => "device_lost",
            Self::Warning => "warning",
            Self::Info => "info",
        }
    }
}

/// How wgpu error-scope validation maps to catalog codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeClass {
    /// Resource create/upload path: validation → `SP-INP-003` (unchanged F03).
    Resource,
    /// Submit/wait path: validation → `SP-GPU-003`.
    Submit,
}

/// Deterministic synthetic inject for the next error-scope observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeErrorInject {
    Validation,
    OutOfMemory {
        requested_bytes: Option<u64>,
        current_bytes: Option<u64>,
        budget_bytes: Option<u64>,
    },
}

impl ScopeErrorInject {
    /// Known OOM byte fields when this inject is [`Self::OutOfMemory`].
    pub fn oom_meta(&self) -> (Option<u64>, Option<u64>, Option<u64>) {
        match self {
            Self::OutOfMemory {
                requested_bytes,
                current_bytes,
                budget_bytes,
            } => (*requested_bytes, *current_bytes, *budget_bytes),
            Self::Validation => (None, None, None),
        }
    }

    pub fn into_domain_error(self, class: ScopeClass) -> DomainError {
        match self {
            Self::Validation => match class {
                ScopeClass::Resource => DomainError::try_new(
                    ErrorCode::ValueOutOfRange,
                    "injected wgpu validation scope failure",
                )
                .expect("non-empty")
                .with_user_action(UserAction::CorrectInput),
                ScopeClass::Submit => DomainError::try_new(
                    ErrorCode::SubmitFailed,
                    "injected wgpu validation scope failure on submit path",
                )
                .expect("non-empty")
                .with_user_action(UserAction::Retry),
            },
            Self::OutOfMemory {
                requested_bytes,
                current_bytes,
                budget_bytes,
            } => {
                let mut msg = String::from("injected wgpu out-of-memory scope failure");
                if let Some(b) = budget_bytes {
                    msg.push_str(&format!(" (budget_bytes={b})"));
                }
                let mut err = DomainError::try_new(ErrorCode::AllocationFailed, msg)
                    .expect("non-empty")
                    .with_user_action(UserAction::Retry);
                // Preserve known byte metadata without setting Command (caller
                // attaches operation via fail_gpu / SubmissionEntry latch).
                if let Some(n) = requested_bytes {
                    let expected = match budget_bytes {
                        Some(b) => format!("requested_bytes={n};budget_bytes={b}"),
                        None => n.to_string(),
                    };
                    let _ = err.insert_context(ContextKey::Expected, expected);
                } else if let Some(b) = budget_bytes {
                    let _ = err.insert_context(ContextKey::Expected, format!("budget_bytes={b}"));
                }
                if let Some(n) = current_bytes {
                    let _ = err.insert_context(ContextKey::Actual, n.to_string());
                }
                err
            }
        }
    }
}

/// One GPU diagnostic record (immutable snapshot for subscribers).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuDiagnosticEvent {
    pub seq: u64,
    pub context_id: ContextId,
    pub generation: ContextGeneration,
    pub kind: DiagnosticKind,
    pub operation: GpuOperation,
    pub label: Option<String>,
    pub code: Option<String>,
    pub message: String,
    pub requested_bytes: Option<u64>,
    pub current_bytes: Option<u64>,
    pub budget_bytes: Option<u64>,
    /// True when label/code/message were UTF-8-truncated for retention caps.
    pub truncated: bool,
}

/// Observable ring counters (immutable copy).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiagnosticStats {
    pub capacity: usize,
    pub len: usize,
    pub next_seq: u64,
    pub dropped_oldest: u64,
}

/// Bounded diagnostic ring: overflow drops **oldest** and increments counter.
#[derive(Debug)]
pub struct DiagnosticRing {
    capacity: usize,
    events: VecDeque<GpuDiagnosticEvent>,
    next_seq: u64,
    dropped_oldest: u64,
    context_id: ContextId,
    generation: ContextGeneration,
}

impl DiagnosticRing {
    pub fn new(context_id: ContextId, generation: ContextGeneration, capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            events: VecDeque::new(),
            next_seq: 1,
            dropped_oldest: 0,
            context_id,
            generation,
        }
    }

    pub fn clear_for_generation(&mut self, context_id: ContextId, generation: ContextGeneration) {
        self.events.clear();
        self.next_seq = 1;
        self.dropped_oldest = 0;
        self.context_id = context_id;
        self.generation = generation;
    }

    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity.max(1);
        while self.events.len() > self.capacity {
            self.events.pop_front();
            self.dropped_oldest = self.dropped_oldest.saturating_add(1);
        }
    }

    pub fn stats(&self) -> DiagnosticStats {
        DiagnosticStats {
            capacity: self.capacity,
            len: self.events.len(),
            next_seq: self.next_seq,
            dropped_oldest: self.dropped_oldest,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        kind: DiagnosticKind,
        operation: GpuOperation,
        label: Option<&str>,
        code: Option<&str>,
        message: impl Into<String>,
        requested_bytes: Option<u64>,
        current_bytes: Option<u64>,
        budget_bytes: Option<u64>,
    ) -> u64 {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.saturating_add(1);
        let (label, t_label) = match label {
            Some(s) => {
                let (s, t) = utf8_truncate(s, MAX_DIAGNOSTIC_LABEL_BYTES);
                (Some(s), t)
            }
            None => (None, false),
        };
        let (code, t_code) = match code {
            Some(s) => {
                let (s, t) = utf8_truncate(s, MAX_DIAGNOSTIC_CODE_BYTES);
                (Some(s), t)
            }
            None => (None, false),
        };
        let (mut message, mut truncated) =
            utf8_truncate(&message.into(), MAX_DIAGNOSTIC_MESSAGE_BYTES);
        truncated |= t_label || t_code;
        let payload = label.as_ref().map_or(0, String::len)
            + code.as_ref().map_or(0, String::len)
            + message.len();
        if payload > MAX_DIAGNOSTIC_PAYLOAD_BYTES {
            let keep = MAX_DIAGNOSTIC_PAYLOAD_BYTES
                .saturating_sub(label.as_ref().map_or(0, String::len))
                .saturating_sub(code.as_ref().map_or(0, String::len));
            let (msg, t) = utf8_truncate(&message, keep);
            message = msg;
            truncated |= t;
        }
        let event = GpuDiagnosticEvent {
            seq,
            context_id: self.context_id,
            generation: self.generation,
            kind,
            operation,
            label,
            code,
            message,
            requested_bytes,
            current_bytes,
            budget_bytes,
            truncated,
        };
        if self.events.len() >= self.capacity {
            self.events.pop_front();
            self.dropped_oldest = self.dropped_oldest.saturating_add(1);
        }
        self.events.push_back(event);
        seq
    }

    /// Copy of retained events with `seq > since_seq` (exclusive), in order.
    pub fn since(&self, since_seq: u64) -> Vec<GpuDiagnosticEvent> {
        self.events
            .iter()
            .filter(|e| e.seq > since_seq)
            .cloned()
            .collect()
    }

    pub fn filter(
        &self,
        since_seq: u64,
        kind: Option<DiagnosticKind>,
        label: Option<&str>,
    ) -> Vec<GpuDiagnosticEvent> {
        self.since(since_seq)
            .into_iter()
            .filter(|e| kind.is_none_or(|k| e.kind == k))
            .filter(|e| label.is_none_or(|want| e.label.as_deref() == Some(want)))
            .collect()
    }
}

/// Rebuild `err` without `drop_key`, preserving code/message/action/retry/other context.
fn rebuild_without_context_key(err: DomainError, drop_key: ContextKey) -> DomainError {
    let kept: Vec<(ContextKey, String)> = err
        .context()
        .iter()
        .filter(|(k, _)| *k != drop_key)
        .cloned()
        .collect();
    let mut rebuilt = DomainError::try_new(err.code(), err.message()).expect("non-empty");
    if let Some(action) = err.user_action() {
        rebuilt = rebuilt.with_user_action(action);
    }
    if let Some(r) = err.retryable_override() {
        rebuilt = rebuilt.with_retryable(r);
    }
    for (k, v) in kept {
        rebuilt = rebuilt
            .with_context(k, v)
            .expect("context keys unique after filter");
    }
    rebuilt
}

/// Attach operation / resource / budget context to a GPU error (idempotent keys).
///
/// When `label` is provided, it wins over the P1-F08 tracker placeholder
/// `accounted_work_buffer` so GPU resource failures carry the spirula label.
pub fn enrich_gpu_error(
    err: DomainError,
    operation: GpuOperation,
    label: Option<&ResourceLabel>,
    requested_bytes: Option<u64>,
    current_bytes: Option<u64>,
    budget_bytes: Option<u64>,
) -> DomainError {
    let mut out = err;
    if out.context_value(ContextKey::Command).is_none() {
        let _ = out.insert_context(ContextKey::Command, operation.as_str());
    }
    if let Some(label) = label {
        match out.context_value(ContextKey::Resource) {
            None => {
                let _ = out.insert_context(ContextKey::Resource, label.as_str());
            }
            Some("accounted_work_buffer") => {
                out = rebuild_without_context_key(out, ContextKey::Resource);
                let _ = out.insert_context(ContextKey::Resource, label.as_str());
            }
            Some(_) => {}
        }
    }
    if out.context_value(ContextKey::Expected).is_none() {
        if let Some(n) = requested_bytes {
            let expected = match budget_bytes {
                Some(b) => format!("requested_bytes={n};budget_bytes={b}"),
                None => n.to_string(),
            };
            let _ = out.insert_context(ContextKey::Expected, expected);
        } else if let Some(b) = budget_bytes {
            let _ = out.insert_context(ContextKey::Expected, format!("budget_bytes={b}"));
        }
    }
    if let Some(n) = current_bytes
        && out.context_value(ContextKey::Actual).is_none()
    {
        let _ = out.insert_context(ContextKey::Actual, n.to_string());
    }
    out
}

pub fn kind_for_code(code: ErrorCode) -> DiagnosticKind {
    match code {
        ErrorCode::DeviceLost => DiagnosticKind::DeviceLost,
        ErrorCode::SubmitFailed
        | ErrorCode::AllocationFailed
        | ErrorCode::ShaderFailed
        | ErrorCode::NoAdapter => DiagnosticKind::Error,
        _ => DiagnosticKind::Warning,
    }
}

pub fn map_wgpu_scope_error(err: wgpu::Error, class: ScopeClass) -> DomainError {
    match err {
        wgpu::Error::OutOfMemory { .. } => DomainError::try_new(
            ErrorCode::AllocationFailed,
            "wgpu out of memory during GPU op",
        )
        .expect("non-empty")
        .with_user_action(UserAction::Retry),
        wgpu::Error::Validation { description, .. } | wgpu::Error::Internal { description, .. } => {
            match class {
                ScopeClass::Resource => DomainError::try_new(
                    ErrorCode::ValueOutOfRange,
                    format!("wgpu validation: {description}"),
                )
                .expect("non-empty")
                .with_user_action(UserAction::CorrectInput),
                ScopeClass::Submit => DomainError::try_new(
                    ErrorCode::SubmitFailed,
                    format!("wgpu validation on submit path: {description}"),
                )
                .expect("non-empty")
                .with_user_action(UserAction::Retry),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_drops_oldest_and_reports_counter() {
        let mut ring =
            DiagnosticRing::new(ContextId::for_test(1), ContextGeneration::for_test(1), 2);
        ring.push(
            DiagnosticKind::Info,
            GpuOperation::Other,
            None,
            None,
            "a",
            None,
            None,
            None,
        );
        ring.push(
            DiagnosticKind::Info,
            GpuOperation::Other,
            None,
            None,
            "b",
            None,
            None,
            None,
        );
        ring.push(
            DiagnosticKind::Info,
            GpuOperation::Other,
            None,
            None,
            "c",
            None,
            None,
            None,
        );
        let stats = ring.stats();
        assert_eq!(stats.len, 2);
        assert_eq!(stats.dropped_oldest, 1);
        let ev = ring.since(0);
        assert_eq!(ev[0].message, "b");
        assert_eq!(ev[1].message, "c");
    }

    #[test]
    fn generation_clear_resets_seq() {
        let mut ring =
            DiagnosticRing::new(ContextId::for_test(1), ContextGeneration::for_test(1), 8);
        ring.push(
            DiagnosticKind::Error,
            GpuOperation::Submit,
            Some("spirula:gpu:buffer:x"),
            Some("SP-GPU-003"),
            "fail",
            None,
            None,
            None,
        );
        ring.clear_for_generation(ContextId::for_test(1), ContextGeneration::for_test(2));
        assert_eq!(ring.stats().next_seq, 1);
        assert_eq!(ring.stats().dropped_oldest, 0);
        assert!(ring.since(0).is_empty());
    }

    #[test]
    fn inject_validation_maps_by_scope_class() {
        assert_eq!(
            ScopeErrorInject::Validation
                .into_domain_error(ScopeClass::Submit)
                .code(),
            ErrorCode::SubmitFailed
        );
        assert_eq!(
            ScopeErrorInject::Validation
                .into_domain_error(ScopeClass::Resource)
                .code(),
            ErrorCode::ValueOutOfRange
        );
    }

    #[test]
    fn utf8_truncate_respects_char_boundaries() {
        let (s, t) = utf8_truncate("aéï", 2);
        assert!(t);
        assert_eq!(s, "a");
        assert!(s.is_char_boundary(s.len()));
    }

    #[test]
    fn ring_marks_truncated_and_honors_payload_cap() {
        let mut ring =
            DiagnosticRing::new(ContextId::for_test(1), ContextGeneration::for_test(1), 4);
        let long_msg = "m".repeat(MAX_DIAGNOSTIC_MESSAGE_BYTES + 64);
        let seq = ring.push(
            DiagnosticKind::Error,
            GpuOperation::Submit,
            Some(&"L".repeat(MAX_DIAGNOSTIC_LABEL_BYTES + 8)),
            Some("SP-GPU-003"),
            long_msg,
            None,
            None,
            None,
        );
        assert_eq!(seq, 1);
        let ev = &ring.since(0)[0];
        assert!(ev.truncated);
        assert!(ev.label.as_ref().unwrap().len() <= MAX_DIAGNOSTIC_LABEL_BYTES);
        assert!(ev.message.len() <= MAX_DIAGNOSTIC_MESSAGE_BYTES);
        let payload = ev.label.as_ref().map_or(0, String::len)
            + ev.code.as_ref().map_or(0, String::len)
            + ev.message.len();
        assert!(payload <= MAX_DIAGNOSTIC_PAYLOAD_BYTES);
    }

    #[test]
    fn inject_oom_carries_expected_actual_bytes() {
        let err = ScopeErrorInject::OutOfMemory {
            requested_bytes: Some(128),
            current_bytes: Some(64),
            budget_bytes: Some(1024),
        }
        .into_domain_error(ScopeClass::Submit);
        assert_eq!(err.code(), ErrorCode::AllocationFailed);
        assert_eq!(
            err.context_value(ContextKey::Expected),
            Some("requested_bytes=128;budget_bytes=1024")
        );
        assert_eq!(err.context_value(ContextKey::Actual), Some("64"));
    }
}
