//! Cooperative cancellation token.
//!
//! The rules are in `docs/phase-1/cancellation.md`. This module does not
//! install a process-global flag or publish session events.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::{ContextKey, DomainError, ErrorCode};

const CLEAR: u8 = 0;

/// Why the first successful cancel request won.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CancelReason {
    User,
    Shutdown,
    Timeout,
    Superseded,
}

impl CancelReason {
    /// Stable context spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Shutdown => "shutdown",
            Self::Timeout => "timeout",
            Self::Superseded => "superseded",
        }
    }

    const fn tag(self) -> u8 {
        match self {
            Self::User => 1,
            Self::Shutdown => 2,
            Self::Timeout => 3,
            Self::Superseded => 4,
        }
    }

    fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            1 => Some(Self::User),
            2 => Some(Self::Shutdown),
            3 => Some(Self::Timeout),
            4 => Some(Self::Superseded),
            _ => None,
        }
    }
}

#[derive(Debug)]
struct TokenState {
    state: AtomicU8,
}

/// Shared cancel flag. Clones observe the same first reason.
#[derive(Clone, Debug)]
pub struct CancelToken {
    inner: Arc<TokenState>,
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}

impl CancelToken {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(TokenState {
                state: AtomicU8::new(CLEAR),
            }),
        }
    }

    /// Install `reason` if no request has won yet.
    ///
    /// Returns whether this call won. A later call is a no-op and returns
    /// false, even when it repeats the winning reason.
    pub fn request(&self, reason: CancelReason) -> bool {
        self.inner
            .state
            .compare_exchange(CLEAR, reason.tag(), Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Whether [`Self::reason`] is set. Both answers come from the same word.
    pub fn is_cancelled(&self) -> bool {
        self.reason().is_some()
    }

    /// The first installed reason, if any.
    pub fn reason(&self) -> Option<CancelReason> {
        CancelReason::from_tag(self.inner.state.load(Ordering::Acquire))
    }

    /// `Ok(())` until the first request, then `SP-CAN-001`.
    pub fn check(&self) -> Result<(), DomainError> {
        match self.reason() {
            None => Ok(()),
            Some(reason) => Err(canceled_error(reason)),
        }
    }
}

fn canceled_error(reason: CancelReason) -> DomainError {
    DomainError::try_new(ErrorCode::Canceled, "cooperative cancellation completed")
        .expect("cancel message is not empty")
        .with_context(ContextKey::Actual, reason.as_str())
        .expect("actual")
}
