//! Cooperative cancel observation for long CPU loops (P3-F02 / runtime §5).
//!
//! Contract: observe cancel at least every 250 ms inside long GPU/CPU loops.
//! Third-party `image` crate decode remains non-preemptible; callers must
//! checkpoint around it and document that limit.

use std::time::{Duration, Instant};

use spirula_core::{CancelToken, DomainError};

/// Maximum gap between cancel observations in owned CPU loops.
pub const CANCEL_CHECK_INTERVAL: Duration = Duration::from_millis(250);

/// Polls [`CancelToken::check`] at least every [`CANCEL_CHECK_INTERVAL`].
pub struct CancelCheckpoint<'a> {
    cancel: Option<&'a CancelToken>,
    last: Instant,
}

impl<'a> CancelCheckpoint<'a> {
    pub fn new(cancel: Option<&'a CancelToken>) -> Self {
        Self {
            cancel,
            last: Instant::now(),
        }
    }

    /// Observe cancel when the interval has elapsed (or immediately if due).
    pub fn poll(&mut self) -> Result<(), DomainError> {
        let Some(cancel) = self.cancel else {
            return Ok(());
        };
        if self.last.elapsed() >= CANCEL_CHECK_INTERVAL {
            self.last = Instant::now();
            cancel.check()?;
        }
        Ok(())
    }

    /// Force an immediate cancel check (stage boundary).
    pub fn poll_now(&mut self) -> Result<(), DomainError> {
        let Some(cancel) = self.cancel else {
            return Ok(());
        };
        self.last = Instant::now();
        cancel.check()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spirula_core::ErrorCode;

    #[test]
    fn poll_now_observes_canceled_token() {
        use spirula_core::CancelReason;
        let cancel = CancelToken::new();
        assert!(cancel.request(CancelReason::User));
        let mut cp = CancelCheckpoint::new(Some(&cancel));
        let e = cp.poll_now().unwrap_err();
        assert_eq!(e.code(), ErrorCode::Canceled);
    }

    #[test]
    fn poll_without_token_is_noop() {
        let mut cp = CancelCheckpoint::new(None);
        cp.poll().unwrap();
        cp.poll_now().unwrap();
    }
}
