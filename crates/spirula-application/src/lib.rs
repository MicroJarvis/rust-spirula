//! Unique logical application/session owner for Spirula Studio.
//!
//! P1-F07 owns command admission, cancel/control, recoverable control
//! history, bounded progress/log/notify fanout, and a fake worker that
//! exercises lifecycle contracts. P1-F09 adds an optional rotated disk
//! diagnostics sink owned here (not in CLI). Session state is serialized
//! under one owner mutex (not a single exclusive writer thread). Binding
//! decisions: `docs/phase-1/application.md`, `docs/phase-1/cli.md`.

mod bound;
mod command;
mod disk_log;
mod event;
mod history;
mod ids;
mod image_ops;
mod limits;
mod notify;
mod presentation;
mod session;
mod state;
mod worker;

pub use command::{Command, CommandPayload, SubmitOutcome};
pub use disk_log::{
    DiskLogConfig, DiskLogCounters, DiskLogHandle, DiskLogLimits, validate_file_stem,
};
pub use event::{
    CommandResult, CursorError, DropCounters, Event, EventCursor, EventEnvelope, LogLevel,
    ReplayOutcome, RunOutcome, Snapshot,
};
pub use ids::{CommandId, RunId, SessionId};
pub use image_ops::{NormalizedMask, OrientedRgb, PrepareMaskArgs};
pub use limits::SessionLimits;
pub use notify::{SubscribeError, Subscription};
pub use presentation::{
    DeliveredFrame, PresentationOwner, PresentationStats, PresentationStatus,
    produce_presentation_frame_once,
};
pub use session::{ApplicationService, SubscribeHandoffError};
pub use state::SessionState;
pub use worker::{FakeWorkerBehavior, StallGate, WorkerEvent};
