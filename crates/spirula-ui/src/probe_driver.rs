//! Windowless presentation-probe state driver (P6-F07 CUA path).
//!
//! Resize **only** when the wanted size differs from the last applied size.
//! While waiting for a generation, never bump generation again. Initial frame
//! at the start size is a single `request` (no resize).

use spirula_application::PresentationOwner;
use spirula_core::DomainError;

/// Probe UI intent vs applied presentation size / generation wait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeDriver {
    pub want: (u32, u32),
    /// Last size successfully applied to the owner (start size initially).
    pub applied: (u32, u32),
    /// When `Some`, waiting for a frame with `size_generation >=` this value.
    pub awaiting_generation: Option<u64>,
    /// Successful property-assignable takes through this driver.
    pub assigned_count: u64,
}

/// One timer-tick decision. Callers must not resize on Idle / ReRequest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeStep {
    Idle,
    /// Size changed — caller must `resize` once, then `request_frame` once.
    ResizeTo {
        width: u32,
        height: u32,
    },
    /// Same size, no wait yet (first frame) — `request_frame` only.
    RequestCurrent,
    /// Already awaiting this generation — may coalesce `request_frame`, never resize.
    ReRequestWhileAwaiting,
}

impl ProbeDriver {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            want: (width, height),
            applied: (width, height),
            awaiting_generation: None,
            assigned_count: 0,
        }
    }

    pub fn set_want(&mut self, width: u32, height: u32) {
        self.want = (width, height);
    }

    /// Pure plan: resize only when `want != applied`.
    pub fn plan(&self) -> ProbeStep {
        if self.want != self.applied {
            return ProbeStep::ResizeTo {
                width: self.want.0,
                height: self.want.1,
            };
        }
        match self.awaiting_generation {
            Some(_) => ProbeStep::ReRequestWhileAwaiting,
            None if self.assigned_count == 0 => ProbeStep::RequestCurrent,
            None => ProbeStep::Idle,
        }
    }

    /// After a successful resize to `want`.
    pub fn note_resized(&mut self, generation: u64) {
        self.applied = self.want;
        self.awaiting_generation = Some(generation);
    }

    /// After issuing the initial (or same-size) request without resizing.
    pub fn note_requested(&mut self, generation: u64) {
        if self.awaiting_generation.is_none() {
            self.awaiting_generation = Some(generation);
        }
    }

    /// Accept a taken frame when its generation/size match the wait.
    pub fn try_assign(&mut self, frame_generation: u64, width: u32, height: u32) -> bool {
        if (width, height) != self.applied {
            return false;
        }
        if let Some(need) = self.awaiting_generation
            && frame_generation < need
        {
            return false;
        }
        self.awaiting_generation = None;
        self.assigned_count = self.assigned_count.saturating_add(1);
        true
    }
}

/// Apply [`ProbeDriver::plan`] against a live owner (no window).
///
/// Returns the step that was executed. Resize happens at most once per size
/// change; awaiting ticks never call `resize`.
pub fn drive_owner(
    driver: &mut ProbeDriver,
    owner: &PresentationOwner,
) -> Result<ProbeStep, DomainError> {
    let step = driver.plan();
    match step {
        ProbeStep::Idle => Ok(step),
        ProbeStep::ResizeTo { width, height } => {
            owner.resize(width, height)?;
            let generation = owner.size_generation();
            driver.note_resized(generation);
            owner.request_frame()?;
            Ok(step)
        }
        ProbeStep::RequestCurrent => {
            let generation = owner.size_generation();
            driver.note_requested(generation);
            owner.request_frame()?;
            Ok(step)
        }
        ProbeStep::ReRequestWhileAwaiting => {
            // Coalesce only; do not touch size / generation.
            let _ = owner.request_frame();
            Ok(step)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spirula_application::PresentationStatus;
    use spirula_core::ErrorCode;
    use std::thread;
    use std::time::{Duration, Instant};

    fn skip_no_gpu(err: &DomainError) -> bool {
        matches!(
            err.code(),
            ErrorCode::NoAdapter | ErrorCode::DeviceLost | ErrorCode::AllocationFailed
        )
    }

    fn ready_owner(w: u32, h: u32) -> Option<PresentationOwner> {
        let owner = match PresentationOwner::start(w, h) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("skip: start failed ({e})");
                return None;
            }
        };
        match owner.wait_ready(Duration::from_secs(30)) {
            Ok(()) => Some(owner),
            Err(e) if skip_no_gpu(&e) => {
                eprintln!("skip: no GPU ({e})");
                owner.shutdown();
                None
            }
            Err(e) => panic!("wait_ready: {e}"),
        }
    }

    #[test]
    fn plan_resizes_only_on_size_change() {
        let mut d = ProbeDriver::new(64, 48);
        assert_eq!(d.plan(), ProbeStep::RequestCurrent);
        d.note_requested(1);
        assert_eq!(d.plan(), ProbeStep::ReRequestWhileAwaiting);
        assert!(d.try_assign(1, 64, 48));
        assert_eq!(d.plan(), ProbeStep::Idle);
        // Same size again: idle (no resize spin).
        d.set_want(64, 48);
        assert_eq!(d.plan(), ProbeStep::Idle);
        d.set_want(65, 3);
        assert_eq!(
            d.plan(),
            ProbeStep::ResizeTo {
                width: 65,
                height: 3
            }
        );
        d.note_resized(2);
        assert_eq!(d.applied, (65, 3));
        assert_eq!(d.awaiting_generation, Some(2));
        assert_eq!(d.plan(), ProbeStep::ReRequestWhileAwaiting);
    }

    #[test]
    fn waiting_to_assigned_taken_increments_gen_stable_or_skip() {
        let Some(owner) = ready_owner(64, 48) else {
            return;
        };
        assert!(matches!(owner.status(), PresentationStatus::Ready { .. }));

        let mut driver = ProbeDriver::new(64, 48);
        let gen0 = owner.size_generation();

        // Many ticks at same size must not bump generation.
        for _ in 0..80 {
            drive_owner(&mut driver, &owner).expect("drive");
        }
        assert_eq!(
            owner.size_generation(),
            gen0,
            "gen must not spin while awaiting first frame"
        );
        assert_eq!(driver.awaiting_generation, Some(gen0));

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut assigned = false;
        while Instant::now() < deadline {
            drive_owner(&mut driver, &owner).expect("drive");
            if let Some(delivered) = owner.take_latest() {
                assert!(
                    driver.try_assign(
                        delivered.frame.size_generation,
                        delivered.frame.width,
                        delivered.frame.height
                    ),
                    "assign {}x{} gen={}",
                    delivered.frame.width,
                    delivered.frame.height,
                    delivered.frame.size_generation
                );
                assigned = true;
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        assert!(assigned, "waiting → assigned");
        let taken_after_first = owner.stats().taken;
        assert!(
            taken_after_first >= 1,
            "taken must increment, got {taken_after_first}"
        );
        assert_eq!(driver.assigned_count, 1);
        assert!(driver.awaiting_generation.is_none());

        let gen_idle = owner.size_generation();
        for _ in 0..40 {
            assert_eq!(
                drive_owner(&mut driver, &owner).expect("idle"),
                ProbeStep::Idle
            );
        }
        assert_eq!(owner.size_generation(), gen_idle);

        // Size change once → one bump; further awaits must not spin.
        driver.set_want(65, 3);
        let before = owner.size_generation();
        assert_eq!(
            drive_owner(&mut driver, &owner).expect("resize"),
            ProbeStep::ResizeTo {
                width: 65,
                height: 3
            }
        );
        let after_resize = owner.size_generation();
        assert!(after_resize > before);
        for _ in 0..80 {
            drive_owner(&mut driver, &owner).expect("await");
        }
        assert_eq!(
            owner.size_generation(),
            after_resize,
            "gen must not spin after resize while awaiting"
        );

        let deadline2 = Instant::now() + Duration::from_secs(10);
        let mut assigned2 = false;
        while Instant::now() < deadline2 {
            drive_owner(&mut driver, &owner).expect("drive");
            if let Some(delivered) = owner.take_latest() {
                assert_eq!((delivered.frame.width, delivered.frame.height), (65, 3));
                assert!(driver.try_assign(
                    delivered.frame.size_generation,
                    delivered.frame.width,
                    delivered.frame.height
                ));
                assigned2 = true;
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        assert!(assigned2, "65×3 assigned");
        assert!(owner.stats().taken > taken_after_first);
        assert_eq!(driver.assigned_count, 2);

        owner.shutdown();
    }
}
