//! CLI entry adapter for Spirula Studio (P1-F09).
//!
//! Parses argv, presents events/text, and handles process signals. All
//! business orchestration and disk-log lifecycle go through
//! `spirula-application`. Binding: `docs/phase-1/cli.md`.

mod args;
mod present;
mod run;
mod signals;

pub use args::{CliAction, ParseError, parse_args};
pub use present::{OutputSink, capabilities_text, help_text, show_error_codes};
pub use run::{CliExit, run_action};

use spirula_core::{
    DomainError, EXIT_CANCELED, EXIT_FAILURE, EXIT_SUCCESS, EXIT_USAGE, ProcessStage,
    process_exit_code,
};
use std::process::ExitCode;

/// Process entry used by the `spirula` binary.
pub fn main_from_args<I, S>(args: I) -> ExitCode
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let argv: Vec<String> = args.into_iter().map(|s| s.as_ref().to_string()).collect();
    // Skip argv[0] when present (binary name).
    let rest: Vec<&str> = if argv.is_empty() {
        Vec::new()
    } else {
        argv[1..].iter().map(String::as_str).collect()
    };
    match parse_args(&rest) {
        Ok(action) => match run_action(action) {
            CliExit::Code(code) => ExitCode::from(code),
            CliExit::Error { error, stage } => {
                present::write_error(&error);
                ExitCode::from(process_exit_code(&error, stage))
            }
        },
        Err(ParseError::Help) => {
            print!("{}", help_text());
            ExitCode::from(EXIT_SUCCESS)
        }
        Err(ParseError::Usage(error)) => {
            present::write_error(&error);
            ExitCode::from(process_exit_code(&error, ProcessStage::BeforeSession))
        }
    }
}

/// Map a completed run outcome to a process exit code.
pub fn exit_from_outcome(outcome: &spirula_application::RunOutcome) -> u8 {
    use spirula_application::RunOutcome;
    match outcome {
        RunOutcome::Completed { .. } => EXIT_SUCCESS,
        RunOutcome::Canceled { .. } => EXIT_CANCELED,
        RunOutcome::Failed { error } => process_exit_code(error, ProcessStage::InSession),
    }
}

#[allow(dead_code)]
fn _exit_constants_used() -> [u8; 4] {
    [EXIT_SUCCESS, EXIT_FAILURE, EXIT_USAGE, EXIT_CANCELED]
}

/// Build a BeforeSession usage error.
pub fn usage_error(code: spirula_core::ErrorCode, message: &str) -> DomainError {
    DomainError::try_new(code, message).expect("message")
}
