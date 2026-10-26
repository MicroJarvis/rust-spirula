//! Presentation helpers. No business state.

use spirula_application::{Event, EventEnvelope, RunOutcome, SessionState};
use spirula_core::DomainError;
use std::io::{self, Write};

/// Stable ready markers for subprocess tests (stdout). Synchronize on these
/// lines — never on fixed sleeps.
pub const READY_PREFIX: &str = "spirula:ready ";

pub fn help_text() -> String {
    let mut out = String::new();
    out.push_str("Spirula Studio CLI (P1-F09 minimal session entry)\n\n");
    out.push_str("Usage:\n");
    out.push_str("  spirula help\n");
    out.push_str("  spirula capabilities\n");
    out.push_str("  spirula probe-load --label <name> [--log-dir <path>]\n");
    out.push_str(
        "  spirula probe-run --steps <n> [--load-label <name>] [--log-dir <path>] [--step-delay-ms <n>] [--save|--no-save]\n",
    );
    out.push_str(
        "  spirula probe --label <name> [--steps <n>] [--log-dir <path>] [--step-delay-ms <n>]\n\n",
    );
    out.push_str("Unavailable in this build:\n");
    out.push_str("  train     (P5-F07)\n");
    out.push_str("  preview   (P4-F06)\n\n");
    out.push_str(
        "Exit codes: 0 success, 1 failure, 2 usage (before session), 130 cancel/SIGINT.\n",
    );
    out.push_str("SIGINT submits Stop{save:true}, waits for the terminal, then exits 130.\n");
    out.push_str("A second SIGINT restores SIG_DFL and re-raises so this delivery terminates.\n");
    out
}

pub fn capabilities_text() -> String {
    let mut out = String::new();
    out.push_str("capabilities:\n");
    out.push_str("  available: help, capabilities, probe-load, probe-run, probe\n");
    out.push_str("  unavailable: train, preview\n");
    out.push_str("  session: shared spirula-application (fake probes only)\n");
    out.push_str("  disk_log: optional --log-dir (application-owned rotated sink)\n");
    out.push_str("  errors: DomainError codes; SPIRULA_ERROR_CODES=1 appends (code: …)\n");
    out
}

pub fn show_error_codes() -> bool {
    std::env::var_os("SPIRULA_ERROR_CODES").is_some_and(|v| v == "1" || v == "true")
}

pub fn write_error(error: &DomainError) {
    let line = error.cli_stderr_line(show_error_codes());
    let mut err = io::stderr().lock();
    let _ = writeln!(err, "{line}");
}

pub fn write_event(envelope: &EventEnvelope) {
    let mut out = io::stdout().lock();
    match &envelope.event {
        Event::StateChanged { from, to, reason } => {
            let _ = writeln!(out, "state {} -> {} ({reason})", from.as_str(), to.as_str());
            // Ready markers for tests / operators.
            if matches!(
                to,
                SessionState::Ready | SessionState::Running | SessionState::Loading
            ) {
                let _ = writeln!(out, "{READY_PREFIX}{}", to.as_str());
            }
        }
        Event::Progress(p) => {
            let total = p
                .total_steps
                .map(|t| t.to_string())
                .unwrap_or_else(|| "?".into());
            let _ = writeln!(out, "progress {}/{total}", p.step);
        }
        Event::Log { level, message } => {
            let _ = writeln!(out, "log {level:?}: {message}");
        }
        Event::Warning { error } => {
            let _ = writeln!(out, "warning: {error}");
        }
        Event::CommandAccepted { command } => {
            let _ = writeln!(out, "accepted {command:?}");
        }
        Event::CommandRejected { command, error } => {
            let _ = writeln!(out, "rejected {command:?}: {error}");
        }
        Event::Completed { outcome } => {
            let _ = writeln!(out, "completed {}", format_outcome(outcome));
        }
        Event::ProbeLoaded { label } => {
            let _ = writeln!(out, "probe-loaded {label}");
        }
        Event::ProbeStarted { steps } => {
            let _ = writeln!(out, "probe-started steps={steps}");
        }
        Event::CheckpointSaved { step, label } => {
            let _ = writeln!(out, "checkpoint step={step} label={label}");
        }
        Event::ShutdownComplete => {
            let _ = writeln!(out, "shutdown-complete");
        }
    }
    let _ = out.flush();
}

fn format_outcome(outcome: &RunOutcome) -> String {
    match outcome {
        RunOutcome::Completed { step } => format!("ok step={step}"),
        RunOutcome::Canceled {
            step,
            reason,
            saved_checkpoint,
        } => format!("canceled step={step} reason={reason:?} saved={saved_checkpoint:?}"),
        RunOutcome::Failed { error } => format!("failed: {error}"),
    }
}
