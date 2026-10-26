//! Process signal handling (P1-F09).
//!
//! First SIGINT sets a cooperative flag the session loop observes (submit
//! `Stop { save: true }`, wait for terminal, exit 130). Second SIGINT
//! restores `SIG_DFL` and re-raises the current signal so the process
//! terminates under the default disposition (C++ `main.cpp` 63–64).

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

/// Number of first-handler SIGINT deliveries (saturates at 1 for counting
/// cooperative stops; the second delivery never returns from the handler).
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

    /// Async-signal-safe second-SIGINT path matching the C++ reference:
    /// restore default disposition, then re-raise so *this* delivery kills.
    #[allow(unsafe_code)] // libc signal / raise; must stay async-signal-safe
    extern "C" fn handle_sigint(_: libc::c_int) {
        let prev = SIGINT_COUNT.fetch_add(1, Ordering::SeqCst);
        if prev == 0 {
            // First: cooperative cancel only.
            return;
        }
        // Second: restore default and deliver the current signal again.
        unsafe {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            let _ = libc::raise(libc::SIGINT);
        }
    }

    #[allow(unsafe_code)] // install SIGINT handler once
    pub fn install() {
        unsafe {
            libc::signal(libc::SIGINT, handle_sigint as *const () as usize);
        }
    }
}
