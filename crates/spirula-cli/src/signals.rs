//! Process signal handling (P1-F09).
//!
//! First SIGINT sets a cooperative flag the session loop observes (submit
//! `Stop { save: true }`, wait for terminal, exit 130). Second SIGINT
//! restores the default disposition per runtime-contracts §5.

use std::sync::atomic::{AtomicU32, Ordering};

static SIGINT_COUNT: AtomicU32 = AtomicU32::new(0);

/// Install the SIGINT handler. Safe to call once per process.
pub fn install() {
    #[cfg(unix)]
    unix::install();
    #[cfg(not(unix))]
    {
        // Non-Unix hosts: cooperative cancel remains available via other
        // controls; SIGINT disposition is OS-default.
    }
}

/// Number of SIGINT deliveries observed by the custom handler.
pub fn sigint_count() -> u32 {
    SIGINT_COUNT.load(Ordering::SeqCst)
}

/// True after the first SIGINT (cooperative stop requested).
pub fn stop_requested() -> bool {
    sigint_count() >= 1
}

#[cfg(unix)]
mod unix {
    use super::SIGINT_COUNT;
    use std::sync::atomic::Ordering;

    #[allow(unsafe_code)] // libc signal registration; second SIGINT → SIG_DFL
    extern "C" fn handle_sigint(_: libc::c_int) {
        let prev = SIGINT_COUNT.fetch_add(1, Ordering::SeqCst);
        if prev >= 1 {
            // Second (and later) SIGINT: restore default disposition so the
            // next delivery uses OS default (reference behavior).
            unsafe {
                libc::signal(libc::SIGINT, libc::SIG_DFL);
            }
        }
    }

    #[allow(unsafe_code)] // install SIGINT handler once
    pub fn install() {
        unsafe {
            libc::signal(libc::SIGINT, handle_sigint as *const () as usize);
        }
    }
}
