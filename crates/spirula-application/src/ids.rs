//! Opaque session, command, and run identities.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

/// One application/session instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SessionId(u64);

impl SessionId {
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "session-{}", self.0)
    }
}

/// Accepted command identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CommandId(u64);

impl CommandId {
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for CommandId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cmd-{}", self.0)
    }
}

/// Active operation generation. Stale worker events with a mismatched id
/// must be rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RunId(u64);

impl RunId {
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "run-{}", self.0)
    }
}

#[derive(Debug)]
pub(crate) struct IdGen {
    next: AtomicU64,
}

impl IdGen {
    pub(crate) fn new(start: u64) -> Self {
        Self {
            next: AtomicU64::new(start),
        }
    }

    pub(crate) fn next(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }
}
