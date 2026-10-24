//! Stable domain contracts shared by the Spirula Studio rewrite.
//!
//! This crate must remain independent of Slint, `wgpu`, filesystem formats,
//! and application process state. P1-F01 owns the error catalog.

mod error;

pub use error::{
    ContextKey, DomainError, DuplicateContextKey, EXIT_CANCELED, EXIT_FAILURE, EXIT_SUCCESS,
    EXIT_USAGE, ErrorCategory, ErrorCode, InvalidMessage, ProcessStage, UnknownContextKey,
    UnknownErrorCode, UserAction, process_exit_code,
};
