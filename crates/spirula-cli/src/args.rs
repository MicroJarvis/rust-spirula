//! Argv parsing only. No session orchestration.

use crate::usage_error;
use spirula_core::{DomainError, ErrorCode};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliAction {
    Help,
    Capabilities,
    /// Fake load then optional run in one process session.
    Probe {
        label: String,
        steps: Option<u64>,
        log_dir: Option<PathBuf>,
        step_delay_ms: Option<u64>,
        save_on_stop: bool,
    },
    /// Explicit control: load only.
    ProbeLoad {
        label: String,
        log_dir: Option<PathBuf>,
    },
    /// Explicit control: run (requires prior load in the same invocation via
    /// `--load-label`, or assumes Ready is unreachable without load).
    ProbeRun {
        steps: u64,
        load_label: Option<String>,
        log_dir: Option<PathBuf>,
        step_delay_ms: Option<u64>,
        save_on_stop: bool,
    },
}

#[derive(Debug)]
pub enum ParseError {
    Help,
    Usage(DomainError),
}

pub fn parse_args(args: &[&str]) -> Result<CliAction, ParseError> {
    if args.is_empty() {
        return Err(ParseError::Help);
    }
    let mut iter = args.iter().copied().peekable();
    let cmd = iter.next().unwrap_or("");
    match cmd {
        "help" | "--help" | "-h" => Ok(CliAction::Help),
        "capabilities" | "--capabilities" => Ok(CliAction::Capabilities),
        "probe-load" => parse_probe_load(&mut iter),
        "probe-run" => parse_probe_run(&mut iter),
        "probe" => parse_probe(&mut iter),
        "train" | "preview" => Err(ParseError::Usage(
            usage_error(
                ErrorCode::UnimplementedOption,
                &format!("{cmd} is not available in this build (wired later)"),
            )
            .with_context(spirula_core::ContextKey::Command, cmd)
            .expect("command"),
        )),
        other if other.starts_with('-') => Err(ParseError::Usage(usage_error(
            ErrorCode::UnknownCommand,
            &format!("unknown flag '{other}'"),
        ))),
        other => Err(ParseError::Usage(usage_error(
            ErrorCode::UnknownCommand,
            &format!("unknown command '{other}'"),
        ))),
    }
}

fn parse_probe_load<'a, I>(iter: &mut std::iter::Peekable<I>) -> Result<CliAction, ParseError>
where
    I: Iterator<Item = &'a str>,
{
    let mut label: Option<String> = None;
    let mut log_dir: Option<PathBuf> = None;
    while let Some(arg) = iter.next() {
        match arg {
            "--label" => {
                label = Some(require_value(iter, "--label")?.to_string());
            }
            "--log-dir" => {
                log_dir = Some(PathBuf::from(require_value(iter, "--log-dir")?));
            }
            "--help" | "-h" => return Err(ParseError::Help),
            other => {
                return Err(ParseError::Usage(usage_error(
                    ErrorCode::UnknownCommand,
                    &format!("unknown probe-load flag '{other}'"),
                )));
            }
        }
    }
    let label = label.ok_or_else(|| {
        ParseError::Usage(usage_error(
            ErrorCode::InvalidArgument,
            "probe-load requires --label",
        ))
    })?;
    if label.trim().is_empty() {
        return Err(ParseError::Usage(usage_error(
            ErrorCode::InvalidArgument,
            "probe-load --label must be non-empty",
        )));
    }
    Ok(CliAction::ProbeLoad { label, log_dir })
}

fn parse_probe_run<'a, I>(iter: &mut std::iter::Peekable<I>) -> Result<CliAction, ParseError>
where
    I: Iterator<Item = &'a str>,
{
    let mut steps: Option<u64> = None;
    let mut load_label: Option<String> = None;
    let mut log_dir: Option<PathBuf> = None;
    let mut step_delay_ms: Option<u64> = None;
    let mut save_on_stop = true;
    while let Some(arg) = iter.next() {
        match arg {
            "--steps" => {
                steps = Some(parse_u64(require_value(iter, "--steps")?, "steps")?);
            }
            "--load-label" => {
                load_label = Some(require_value(iter, "--load-label")?.to_string());
            }
            "--log-dir" => {
                log_dir = Some(PathBuf::from(require_value(iter, "--log-dir")?));
            }
            "--step-delay-ms" => {
                step_delay_ms = Some(parse_u64(
                    require_value(iter, "--step-delay-ms")?,
                    "step-delay-ms",
                )?);
            }
            "--save" => save_on_stop = true,
            "--no-save" => save_on_stop = false,
            "--help" | "-h" => return Err(ParseError::Help),
            other => {
                return Err(ParseError::Usage(usage_error(
                    ErrorCode::UnknownCommand,
                    &format!("unknown probe-run flag '{other}'"),
                )));
            }
        }
    }
    let steps = steps.ok_or_else(|| {
        ParseError::Usage(usage_error(
            ErrorCode::InvalidArgument,
            "probe-run requires --steps",
        ))
    })?;
    if steps == 0 {
        return Err(ParseError::Usage(usage_error(
            ErrorCode::ValueOutOfRange,
            "probe-run --steps must be > 0",
        )));
    }
    Ok(CliAction::ProbeRun {
        steps,
        load_label,
        log_dir,
        step_delay_ms,
        save_on_stop,
    })
}

fn parse_probe<'a, I>(iter: &mut std::iter::Peekable<I>) -> Result<CliAction, ParseError>
where
    I: Iterator<Item = &'a str>,
{
    let mut label: Option<String> = None;
    let mut steps: Option<u64> = None;
    let mut log_dir: Option<PathBuf> = None;
    let mut step_delay_ms: Option<u64> = None;
    let mut save_on_stop = true;
    while let Some(arg) = iter.next() {
        match arg {
            "--label" => label = Some(require_value(iter, "--label")?.to_string()),
            "--steps" => steps = Some(parse_u64(require_value(iter, "--steps")?, "steps")?),
            "--log-dir" => log_dir = Some(PathBuf::from(require_value(iter, "--log-dir")?)),
            "--step-delay-ms" => {
                step_delay_ms = Some(parse_u64(
                    require_value(iter, "--step-delay-ms")?,
                    "step-delay-ms",
                )?);
            }
            "--save" => save_on_stop = true,
            "--no-save" => save_on_stop = false,
            "--help" | "-h" => return Err(ParseError::Help),
            other => {
                return Err(ParseError::Usage(usage_error(
                    ErrorCode::UnknownCommand,
                    &format!("unknown probe flag '{other}'"),
                )));
            }
        }
    }
    let label = label.ok_or_else(|| {
        ParseError::Usage(usage_error(
            ErrorCode::InvalidArgument,
            "probe requires --label",
        ))
    })?;
    Ok(CliAction::Probe {
        label,
        steps,
        log_dir,
        step_delay_ms,
        save_on_stop,
    })
}

fn require_value<'a, I>(iter: &mut I, flag: &str) -> Result<&'a str, ParseError>
where
    I: Iterator<Item = &'a str>,
{
    iter.next().ok_or_else(|| {
        ParseError::Usage(usage_error(
            ErrorCode::InvalidArgument,
            &format!("{flag} requires a value"),
        ))
    })
}

fn parse_u64(raw: &str, field: &str) -> Result<u64, ParseError> {
    raw.parse::<u64>().map_err(|_| {
        ParseError::Usage(
            usage_error(ErrorCode::ValueOutOfRange, &format!("invalid {field}"))
                .with_context(spirula_core::ContextKey::Field, field)
                .expect("field")
                .with_context(spirula_core::ContextKey::Actual, raw)
                .expect("actual"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_and_capabilities() {
        assert!(matches!(parse_args(&["help"]), Ok(CliAction::Help)));
        assert!(matches!(
            parse_args(&["capabilities"]),
            Ok(CliAction::Capabilities)
        ));
    }

    #[test]
    fn train_unavailable_before_session() {
        let err = parse_args(&["train"]).unwrap_err();
        match err {
            ParseError::Usage(e) => {
                assert_eq!(e.code(), ErrorCode::UnimplementedOption);
            }
            ParseError::Help => panic!("expected usage"),
        }
    }

    #[test]
    fn probe_run_requires_steps() {
        assert!(matches!(
            parse_args(&["probe-run"]),
            Err(ParseError::Usage(_))
        ));
        let a = parse_args(&["probe-run", "--steps", "3", "--load-label", "ds"]).unwrap();
        assert!(matches!(a, CliAction::ProbeRun { steps: 3, .. }));
    }
}
