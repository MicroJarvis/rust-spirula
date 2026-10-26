//! P2-F05: GPU-local diagnostic events (not application session history).

use std::collections::VecDeque;

use spirula_core::{ContextKey, DomainError, ErrorCode, UserAction};

use crate::context::{ContextGeneration, ContextId};
use crate::resource::ResourceLabel;

/// Default retained diagnostic events per live context generation.
pub const DEFAULT_DIAGNOSTIC_CAPACITY: usize = 64;

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
                requested_bytes: _,
                current_bytes: _,
                budget_bytes,
            } => {
                let mut msg = String::from("injected wgpu out-of-memory scope failure");
                if let Some(b) = budget_bytes {
                    msg.push_str(&format!(" (budget_bytes={b})"));
                }
                // Operation/label/budget context is attached by the caller via
                // `GpuContext::fail_gpu` (create/submit/…), not here.
                DomainError::try_new(ErrorCode::AllocationFailed, msg)
                    .expect("non-empty")
                    .with_user_action(UserAction::Retry)
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
        let event = GpuDiagnosticEvent {
            seq,
            context_id: self.context_id,
            generation: self.generation,
            kind,
            operation,
            label: label.map(str::to_owned),
            code: code.map(str::to_owned),
            message: message.into(),
            requested_bytes,
            current_bytes,
            budget_bytes,
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
}
