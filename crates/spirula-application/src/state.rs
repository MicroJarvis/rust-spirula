//! Session state machine names for P1-F07.
//!
//! `Running` is the Rust spelling of the contract `Training` state. This
//! slice drives it with a fake probe, not a real trainer.

use std::fmt;

/// Authoritative session states used by the owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SessionState {
    Closed,
    Idle,
    Loading,
    Ready,
    Preparing,
    /// Contract name: Training. Fake probe only in P1-F07.
    Running,
    Paused,
    Stopping,
    Saving,
    Evaluating,
    Failed,
}

impl SessionState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Idle => "idle",
            Self::Loading => "loading",
            Self::Ready => "ready",
            Self::Preparing => "preparing",
            Self::Running => "training",
            Self::Paused => "paused",
            Self::Stopping => "stopping",
            Self::Saving => "saving",
            Self::Evaluating => "evaluating",
            Self::Failed => "failed",
        }
    }
}

impl fmt::Display for SessionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
