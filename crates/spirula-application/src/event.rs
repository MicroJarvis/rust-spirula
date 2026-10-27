//! Event envelope, snapshot, cursor, and replay outcomes.

use crate::command::CommandPayload;
use crate::ids::{CommandId, RunId, SessionId};
use crate::state::SessionState;
use spirula_core::{DomainError, OutcomeView, ProgressSnapshot, TerminalOutcome};

/// Drop / coalesce counters exposed on snapshots.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DropCounters {
    pub control_history: u64,
    pub progress: u64,
    pub log: u64,
    pub notification: u64,
    pub stale_worker: u64,
    pub worker_mailbox: u64,
    pub subscriber_disconnect: u64,
    pub subscribe_rejected: u64,
}

/// Canonical run/command outcome. Event-visible via core [`OutcomeView`].
pub type RunOutcome = OutcomeView;

pub(crate) fn outcome_from_terminal(outcome: TerminalOutcome) -> RunOutcome {
    match outcome {
        TerminalOutcome::Completed { step } => OutcomeView::Completed { step },
        TerminalOutcome::Canceled {
            step,
            reason,
            saved_checkpoint,
        } => OutcomeView::Canceled {
            step,
            reason,
            saved_checkpoint,
        },
        TerminalOutcome::Failed { error } => OutcomeView::Failed {
            error: error.without_source(),
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

/// Control and progress event bodies.
#[derive(Debug)]
pub enum Event {
    StateChanged {
        from: SessionState,
        to: SessionState,
        reason: String,
    },
    Progress(ProgressSnapshot),
    Log {
        level: LogLevel,
        message: String,
    },
    Warning {
        error: DomainError,
    },
    CommandAccepted {
        command: CommandPayload,
    },
    CommandRejected {
        command: CommandPayload,
        error: DomainError,
    },
    Completed {
        outcome: RunOutcome,
    },
    ProbeLoaded {
        label: String,
    },
    ProbeStarted {
        steps: u64,
    },
    /// Real P3-F02 orient bake finished (before command terminal).
    ImagePrepared {
        path: String,
        width: u32,
        height: u32,
        orientation: u8,
        turns_only: bool,
    },
    /// Real P3-F02 mask normalize finished (before command terminal).
    MaskPrepared {
        path: String,
        width: u32,
        height: u32,
        kept: u64,
        alpha: bool,
    },
    CheckpointSaved {
        step: u64,
        label: String,
    },
    ShutdownComplete,
}

impl Clone for Event {
    fn clone(&self) -> Self {
        match self {
            Self::StateChanged { from, to, reason } => Self::StateChanged {
                from: *from,
                to: *to,
                reason: reason.clone(),
            },
            Self::Progress(p) => Self::Progress(p.clone()),
            Self::Log { level, message } => Self::Log {
                level: *level,
                message: message.clone(),
            },
            Self::Warning { error } => Self::Warning {
                error: error.without_source(),
            },
            Self::CommandAccepted { command } => Self::CommandAccepted {
                command: command.clone(),
            },
            Self::CommandRejected { command, error } => Self::CommandRejected {
                command: command.clone(),
                error: error.without_source(),
            },
            Self::Completed { outcome } => Self::Completed {
                outcome: outcome.clone(),
            },
            Self::ProbeLoaded { label } => Self::ProbeLoaded {
                label: label.clone(),
            },
            Self::ProbeStarted { steps } => Self::ProbeStarted { steps: *steps },
            Self::ImagePrepared {
                path,
                width,
                height,
                orientation,
                turns_only,
            } => Self::ImagePrepared {
                path: path.clone(),
                width: *width,
                height: *height,
                orientation: *orientation,
                turns_only: *turns_only,
            },
            Self::MaskPrepared {
                path,
                width,
                height,
                kept,
                alpha,
            } => Self::MaskPrepared {
                path: path.clone(),
                width: *width,
                height: *height,
                kept: *kept,
                alpha: *alpha,
            },
            Self::CheckpointSaved { step, label } => Self::CheckpointSaved {
                step: *step,
                label: label.clone(),
            },
            Self::ShutdownComplete => Self::ShutdownComplete,
        }
    }
}

impl Event {
    pub(crate) fn is_control(&self) -> bool {
        !matches!(self, Self::Progress(_) | Self::Log { .. })
    }

    pub(crate) fn is_terminal_for_command(&self) -> bool {
        matches!(self, Self::Completed { .. } | Self::ShutdownComplete)
    }

    pub(crate) fn approx_bytes(&self) -> usize {
        match self {
            Self::StateChanged { reason, .. } => 64 + reason.len(),
            Self::Progress(p) => crate::bound::progress_approx_bytes(p),
            Self::Log { message, .. } => 32 + message.len(),
            Self::Warning { error } => crate::bound::error_approx_bytes(error),
            Self::CommandAccepted { command } => 48 + command.approx_bytes(),
            Self::CommandRejected { command, error } => {
                48 + command.approx_bytes() + crate::bound::error_approx_bytes(error)
            }
            Self::Completed { outcome } => match outcome {
                OutcomeView::Failed { error } => 64 + crate::bound::error_approx_bytes(error),
                OutcomeView::Canceled {
                    saved_checkpoint, ..
                } => 64 + saved_checkpoint.as_ref().map(|s| s.len()).unwrap_or(0),
                OutcomeView::Completed { .. } => 48,
            },
            Self::ProbeLoaded { label } => 32 + label.len(),
            Self::ProbeStarted { .. } => 32,
            Self::ImagePrepared { path, .. } => 48 + path.len(),
            Self::MaskPrepared { path, .. } => 48 + path.len(),
            Self::CheckpointSaved { label, .. } => 48 + label.len(),
            Self::ShutdownComplete => 24,
        }
    }
}

/// Source-ordered envelope. Delivered streams may omit replaceable progress/log.
#[derive(Clone, Debug)]
pub struct EventEnvelope {
    pub event_version: u32,
    pub session_id: SessionId,
    pub run_id: Option<RunId>,
    pub seq: u64,
    pub command_id: Option<CommandId>,
    pub monotonic_ms: u64,
    pub event: Event,
}

/// Replay / subscribe cursor. Always carries session identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EventCursor {
    pub session_id: SessionId,
    pub seq: u64,
}

impl EventCursor {
    pub fn new(session_id: SessionId, seq: u64) -> Self {
        Self { session_id, seq }
    }
}

/// Why a cursor was rejected before replay.
#[derive(Debug)]
pub enum CursorError {
    ForeignSession {
        expected: SessionId,
        actual: SessionId,
        error: DomainError,
    },
    Future {
        snapshot_seq: u64,
        cursor_seq: u64,
        error: DomainError,
    },
}

/// Point-in-time owner view for reconnect / resync.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub session_id: SessionId,
    pub seq: u64,
    pub state: SessionState,
    pub active_command: Option<CommandId>,
    pub active_run: Option<RunId>,
    pub last_outcome: Option<RunOutcome>,
    pub drops: DropCounters,
    pub loaded_label: Option<String>,
    /// Cursor at this snapshot boundary (same seq).
    pub cursor: EventCursor,
}

/// Result of replaying control history from a cursor.
#[derive(Clone, Debug)]
pub enum ReplayOutcome {
    /// Retained control events after the cursor. Progress/log gaps are intentional.
    ControlEvents(Vec<EventEnvelope>),
    /// Cursor is behind expired control history. Not used for missing progress.
    ResyncRequired {
        snapshot: Box<Snapshot>,
        retained_terminals: Vec<EventEnvelope>,
    },
}

/// Lookup of a prior command's terminal.
#[derive(Clone, Debug)]
pub enum CommandResult {
    Pending,
    Ready(EventEnvelope),
    Expired,
    Unknown,
}
