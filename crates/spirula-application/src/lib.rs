//! Unique application/session owner for Spirula Studio.
//!
//! P1-F07 owns command admission, cancel/control, recoverable control
//! history, bounded progress/log/notify fanout, and a fake worker that
//! exercises lifecycle contracts. It depends only on `spirula-core`.
//! Binding decisions: `docs/phase-1/application.md`.

mod bound;
mod command;
mod event;
mod history;
mod ids;
mod limits;
mod notify;
mod session;
mod state;
mod worker;

pub use command::{Command, CommandPayload, SubmitOutcome};
pub use event::{
    CommandResult, CursorError, DropCounters, Event, EventCursor, EventEnvelope, LogLevel,
    ReplayOutcome, RunOutcome, Snapshot,
};
pub use ids::{CommandId, RunId, SessionId};
pub use limits::SessionLimits;
pub use notify::Subscription;
pub use session::ApplicationService;
pub use state::SessionState;
pub use worker::{FakeWorkerBehavior, StallGate, WorkerEvent};
