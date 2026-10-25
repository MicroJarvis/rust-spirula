//! Typed progress and one terminal outcome for a single piece of work.
//!
//! This is not a session and does not own a channel. See
//! `docs/phase-1/cancellation.md`.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use crate::cancel::CancelReason;
use crate::{ContextKey, DomainError, ErrorCode};

/// One named finite metric on a progress snapshot.
#[derive(Clone, Debug, PartialEq)]
pub struct ProgressMetric {
    pub name: String,
    pub value: f64,
}

/// Frozen `Progress` payload.
///
/// `total_steps: None` is an indeterminate total. `eta_ms: None` means the
/// estimate is unknown. A known total requires `step <= total_steps`.
#[derive(Clone, Debug, PartialEq)]
pub struct ProgressSnapshot {
    pub step: u64,
    pub total_steps: Option<u64>,
    pub splats: u64,
    pub elapsed_ms: u64,
    pub eta_ms: Option<u64>,
    pub metrics: Vec<ProgressMetric>,
}

impl ProgressSnapshot {
    pub fn try_new(
        step: u64,
        total_steps: Option<u64>,
        splats: u64,
        elapsed_ms: u64,
        eta_ms: Option<u64>,
        metrics: Vec<ProgressMetric>,
    ) -> Result<Self, DomainError> {
        let snapshot = Self {
            step,
            total_steps,
            splats,
            elapsed_ms,
            eta_ms,
            metrics,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    /// Reject a step past a known total and illegal metrics.
    pub fn validate(&self) -> Result<(), DomainError> {
        if let Some(total) = self.total_steps
            && self.step > total
        {
            return Err(range_error(
                "step",
                &format!("<= {total}"),
                &self.step.to_string(),
                "progress step exceeds total_steps",
            ));
        }
        let mut names = BTreeSet::new();
        for metric in &self.metrics {
            if metric.name.trim().is_empty() {
                return Err(argument_error(
                    "metrics",
                    "non-empty name",
                    &metric.name,
                    "progress metric name is empty",
                ));
            }
            if !metric.value.is_finite() {
                return Err(range_error(
                    "metrics",
                    "finite number",
                    &metric.value.to_string(),
                    "progress metric is not finite",
                ));
            }
            if !names.insert(metric.name.as_str()) {
                return Err(argument_error(
                    "metrics",
                    "unique name",
                    &metric.name,
                    "progress metric name is duplicated",
                ));
            }
        }
        Ok(())
    }
}

/// Terminal result of one piece of work. Not a session state.
#[derive(Debug)]
pub enum TerminalOutcome {
    Completed {
        step: u64,
    },
    Canceled {
        step: u64,
        reason: CancelReason,
        saved_checkpoint: Option<String>,
    },
    Failed {
        error: DomainError,
    },
}

/// Observable copy of a published outcome. `Failed` keeps code and message.
#[derive(Clone, Debug, PartialEq)]
pub enum OutcomeView {
    Completed {
        step: u64,
    },
    Canceled {
        step: u64,
        reason: CancelReason,
        saved_checkpoint: Option<String>,
    },
    Failed {
        code: ErrorCode,
        message: String,
    },
}

struct PublisherInner {
    latest: Option<ProgressSnapshot>,
    outcome: Option<TerminalOutcome>,
}

/// Publishes progress until exactly one terminal outcome.
#[derive(Clone)]
pub struct ProgressPublisher {
    inner: Arc<Mutex<PublisherInner>>,
}

impl Default for ProgressPublisher {
    fn default() -> Self {
        Self::new()
    }
}

impl ProgressPublisher {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(PublisherInner {
                latest: None,
                outcome: None,
            })),
        }
    }

    pub fn publish_progress(&self, snapshot: ProgressSnapshot) -> Result<(), DomainError> {
        snapshot.validate()?;
        let mut guard = self.lock();
        if guard.outcome.is_some() {
            return Err(closed_error("progress"));
        }
        guard.latest = Some(snapshot);
        Ok(())
    }

    pub fn publish_terminal(&self, outcome: TerminalOutcome) -> Result<(), DomainError> {
        validate_outcome(&outcome)?;
        let mut guard = self.lock();
        if guard.outcome.is_some() {
            return Err(closed_error("terminal"));
        }
        guard.outcome = Some(outcome);
        Ok(())
    }

    pub fn latest(&self) -> Option<ProgressSnapshot> {
        self.lock().latest.clone()
    }

    pub fn outcome(&self) -> Option<OutcomeView> {
        self.lock().outcome.as_ref().map(outcome_view)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PublisherInner> {
        self.inner
            .lock()
            .expect("progress publisher lock is not poisoned")
    }
}

fn outcome_view(outcome: &TerminalOutcome) -> OutcomeView {
    match outcome {
        TerminalOutcome::Completed { step } => OutcomeView::Completed { step: *step },
        TerminalOutcome::Canceled {
            step,
            reason,
            saved_checkpoint,
        } => OutcomeView::Canceled {
            step: *step,
            reason: *reason,
            saved_checkpoint: saved_checkpoint.clone(),
        },
        TerminalOutcome::Failed { error } => OutcomeView::Failed {
            code: error.code(),
            message: error.message().to_owned(),
        },
    }
}

fn validate_outcome(outcome: &TerminalOutcome) -> Result<(), DomainError> {
    if let TerminalOutcome::Canceled {
        saved_checkpoint: Some(path),
        ..
    } = outcome
        && path.trim().is_empty()
    {
        return Err(argument_error(
            "checkpoint",
            "path or none",
            path,
            "canceled outcome has an empty checkpoint path",
        ));
    }
    Ok(())
}

fn argument_error(field: &str, expected: &str, actual: &str, message: &str) -> DomainError {
    domain(ErrorCode::InvalidArgument, message)
        .with_context(ContextKey::Field, field)
        .expect("field")
        .with_context(ContextKey::Expected, expected)
        .expect("expected")
        .with_context(ContextKey::Actual, actual)
        .expect("actual")
}

fn range_error(field: &str, expected: &str, actual: &str, message: &str) -> DomainError {
    domain(ErrorCode::ValueOutOfRange, message)
        .with_context(ContextKey::Field, field)
        .expect("field")
        .with_context(ContextKey::Expected, expected)
        .expect("expected")
        .with_context(ContextKey::Actual, actual)
        .expect("actual")
}

fn closed_error(field: &str) -> DomainError {
    let message = format!("{field} cannot be published after a terminal outcome");
    domain(ErrorCode::InvalidState, &message)
        .with_context(ContextKey::Field, field)
        .expect("field")
}

fn domain(code: ErrorCode, message: &str) -> DomainError {
    DomainError::try_new(code, message).expect("progress message is not empty")
}
