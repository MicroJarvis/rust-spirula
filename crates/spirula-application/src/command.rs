//! Command payloads admitted by the P1-F07 session owner.

use crate::bound::check_label;
use crate::ids::CommandId;
use crate::limits::SessionLimits;
use spirula_core::{ContextKey, DomainError, ErrorCode};

/// Caller-facing command before admission assigns an id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub payload: CommandPayload,
}

impl Command {
    pub fn new(payload: CommandPayload) -> Self {
        Self { payload }
    }
}

/// P1-F07 command surface. Real load/train names stay out of this enum.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandPayload {
    /// Fake dataset load. `label` is a non-empty probe tag, not a path read.
    ProbeLoad {
        label: String,
    },
    /// Fake multi-step work. Does not claim training capability.
    ProbeRun {
        steps: u64,
    },
    Pause,
    Resume,
    Stop {
        save: bool,
    },
    Shutdown,
    /// Clear `Failed` back to `Idle`.
    Reset,
}

impl CommandPayload {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::ProbeLoad { .. } => "probe_load",
            Self::ProbeRun { .. } => "probe_run",
            Self::Pause => "pause",
            Self::Resume => "resume",
            Self::Stop { .. } => "stop",
            Self::Shutdown => "shutdown",
            Self::Reset => "reset",
        }
    }

    pub(crate) fn is_stop(&self) -> bool {
        matches!(self, Self::Stop { .. })
    }

    pub(crate) fn is_shutdown(&self) -> bool {
        matches!(self, Self::Shutdown)
    }

    pub(crate) fn is_mutating_probe(&self) -> bool {
        matches!(self, Self::ProbeLoad { .. } | Self::ProbeRun { .. })
    }

    pub(crate) fn validate(&self, limits: &SessionLimits) -> Result<(), DomainError> {
        match self {
            Self::ProbeLoad { label } => check_label(label, limits),
            Self::ProbeRun { steps } => {
                if *steps == 0 {
                    return Err(DomainError::try_new(
                        ErrorCode::ValueOutOfRange,
                        "probe run steps must be > 0",
                    )
                    .expect("message")
                    .with_context(ContextKey::Field, "steps")
                    .expect("field")
                    .with_context(ContextKey::Expected, "> 0")
                    .expect("expected")
                    .with_context(ContextKey::Actual, "0")
                    .expect("actual"));
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

/// Immediate admission result. Callers never block on work.
#[derive(Debug)]
pub enum SubmitOutcome {
    Accepted { id: CommandId },
    Rejected { error: DomainError },
}
