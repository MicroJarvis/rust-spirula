//! Cooperative cancel observation for long CPU loops (P3-F02 / runtime §5).
//!
//! Contract: observe cancel at least every [`CANCEL_CHECK_INTERVAL`] inside
//! long owned GPU/CPU loops. Callers must invoke [`CancelCheckpoint::tick`]
//! (or [`poll`] / [`poll_now`]) at **bounded work chunks** so a silent inner
//! loop cannot exceed that wall-clock gap. [`poll`] alone only checks when
//! *called* after the interval; it cannot insert calls into an unchecked
//! wide row or area accumulation.
//!
//! Third-party `image` crate decode remains non-preemptible; callers must
//! checkpoint around it. Runtime §5 also lists per-image decode as a
//! declared checkpoint boundary (distinct from owned-loop 250 ms).

use std::time::{Duration, Instant};

use spirula_core::{CancelToken, DomainError};

/// Maximum gap between cancel observations in owned CPU loops.
pub const CANCEL_CHECK_INTERVAL: Duration = Duration::from_millis(250);

/// Work units between forced cancel observations in owned loops.
///
/// Sized so typical per-unit CPU work stays well under
/// [`CANCEL_CHECK_INTERVAL`] even for dense bilinear / area samples.
pub const CANCEL_WORK_CHUNK: usize = 65_536;

/// Polls [`CancelToken::check`] on bounded work chunks and at stage boundaries.
pub struct CancelCheckpoint<'a> {
    cancel: Option<&'a CancelToken>,
    last: Instant,
    pending: usize,
}

impl<'a> CancelCheckpoint<'a> {
    pub fn new(cancel: Option<&'a CancelToken>) -> Self {
        Self {
            cancel,
            last: Instant::now(),
            pending: 0,
        }
    }

    /// Account `units` of loop work; observe cancel every [`CANCEL_WORK_CHUNK`].
    pub fn tick(&mut self, units: usize) -> Result<(), DomainError> {
        let Some(cancel) = self.cancel else {
            return Ok(());
        };
        self.pending = self.pending.saturating_add(units);
        while self.pending >= CANCEL_WORK_CHUNK {
            self.pending -= CANCEL_WORK_CHUNK;
            self.last = Instant::now();
            cancel.check()?;
        }
        // Also fire if the wall clock already exceeded the interval between
        // chunk boundaries (slow units / scheduling stalls).
        if self.last.elapsed() >= CANCEL_CHECK_INTERVAL {
            self.last = Instant::now();
            cancel.check()?;
        }
        Ok(())
    }

    /// Observe cancel when the interval has elapsed (caller must still invoke
    /// this frequently enough — prefer [`tick`] inside dense loops).
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
        self.pending = 0;
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
    fn tick_observes_canceled_token_after_chunk() {
        use spirula_core::CancelReason;
        let cancel = CancelToken::new();
        assert!(cancel.request(CancelReason::User));
        let mut cp = CancelCheckpoint::new(Some(&cancel));
        let e = cp.tick(CANCEL_WORK_CHUNK).unwrap_err();
        assert_eq!(e.code(), ErrorCode::Canceled);
    }

    #[test]
    fn poll_without_token_is_noop() {
        let mut cp = CancelCheckpoint::new(None);
        cp.poll().unwrap();
        cp.poll_now().unwrap();
        cp.tick(CANCEL_WORK_CHUNK * 2).unwrap();
    }
}
