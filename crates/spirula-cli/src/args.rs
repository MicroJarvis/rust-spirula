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
    /// Real COLMAP dataset load through the application owner (P3-F03).
    LoadDataset {
        path: PathBuf,
        recon_dir: Option<String>,
        eval_mode: String,
        split: String,
        log_dir: Option<PathBuf>,
    },
    /// Real headless GPU preview (P4-F06).
    Preview {
        request: PathBuf,
        output: PathBuf,
        log_dir: Option<PathBuf>,
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
        "help" | "--help" | "-h" => {
            reject_trailing_flags(&mut iter, "help")?;
            Ok(CliAction::Help)
        }
        "capabilities" | "--capabilities" => {
            reject_trailing_flags(&mut iter, "capabilities")?;
            Ok(CliAction::Capabilities)
        }
        "probe-load" => parse_probe_load(&mut iter),
        "probe-run" => parse_probe_run(&mut iter),
        "probe" => parse_probe(&mut iter),
        "load-dataset" => parse_load_dataset(&mut iter),
        "preview" => parse_preview(&mut iter),
        "train" => Err(ParseError::Usage(
            usage_error(
                ErrorCode::UnimplementedOption,
                "train is not available in this build (wired later)",
            )
            .with_context(spirula_core::ContextKey::Command, "train")
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

/// `help` / `capabilities` take no options; unknown trailing flags are usage.
fn reject_trailing_flags<'a, I>(
    iter: &mut std::iter::Peekable<I>,
    command: &str,
) -> Result<(), ParseError>
where
    I: Iterator<Item = &'a str>,
{
    if let Some(extra) = iter.next() {
        return Err(ParseError::Usage(usage_error(
            ErrorCode::UnknownCommand,
            &format!("{command} does not accept '{extra}'"),
        )));
    }
    Ok(())
}

fn require_nonempty_label(label: &str, flag: &str) -> Result<(), ParseError> {
    if label.trim().is_empty() {
        return Err(ParseError::Usage(usage_error(
            ErrorCode::InvalidArgument,
            &format!("{flag} must be non-empty"),
        )));
    }
    Ok(())
}

fn require_positive_steps(steps: u64, command: &str) -> Result<(), ParseError> {
    if steps == 0 {
        return Err(ParseError::Usage(usage_error(
            ErrorCode::ValueOutOfRange,
            &format!("{command} --steps must be > 0"),
        )));
    }
    Ok(())
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
    require_nonempty_label(&label, "probe-load --label")?;
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
                let v = require_value(iter, "--load-label")?.to_string();
                require_nonempty_label(&v, "probe-run --load-label")?;
                load_label = Some(v);
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
    require_positive_steps(steps, "probe-run")?;
    Ok(CliAction::ProbeRun {
        steps,
        load_label,
        log_dir,
        step_delay_ms,
        save_on_stop,
    })
}

fn parse_load_dataset<'a, I>(iter: &mut std::iter::Peekable<I>) -> Result<CliAction, ParseError>
where
    I: Iterator<Item = &'a str>,
{
    let mut path: Option<PathBuf> = None;
    let mut recon_dir: Option<String> = None;
    let mut eval_mode = "all".to_string();
    let mut split = "train".to_string();
    let mut log_dir: Option<PathBuf> = None;
    while let Some(arg) = iter.next() {
        match arg {
            "--path" => {
                path = Some(PathBuf::from(require_value(iter, "--path")?));
            }
            "--recon-dir" => {
                recon_dir = Some(require_value(iter, "--recon-dir")?.to_string());
            }
            "--eval-mode" => {
                eval_mode = require_value(iter, "--eval-mode")?.to_string();
            }
            "--split" => {
                split = require_value(iter, "--split")?.to_string();
            }
            "--log-dir" => {
                log_dir = Some(PathBuf::from(require_value(iter, "--log-dir")?));
            }
            "--help" | "-h" => return Err(ParseError::Help),
            other => {
                return Err(ParseError::Usage(usage_error(
                    ErrorCode::UnknownCommand,
                    &format!("unknown load-dataset flag '{other}'"),
                )));
            }
        }
    }
    let path = path.ok_or_else(|| {
        ParseError::Usage(usage_error(
            ErrorCode::InvalidArgument,
            "load-dataset requires --path",
        ))
    })?;
    match eval_mode.as_str() {
        "all" | "fraction" | "interval" | "filename" => {}
        other => {
            return Err(ParseError::Usage(usage_error(
                ErrorCode::InvalidArgument,
                &format!("unknown eval_mode '{other}'"),
            )));
        }
    }
    match split.as_str() {
        "train" | "eval" | "all" => {}
        other => {
            return Err(ParseError::Usage(usage_error(
                ErrorCode::InvalidArgument,
                &format!("unknown split '{other}'"),
            )));
        }
    }
    Ok(CliAction::LoadDataset {
        path,
        recon_dir,
        eval_mode,
        split,
        log_dir,
    })
}

fn parse_preview<'a, I>(iter: &mut std::iter::Peekable<I>) -> Result<CliAction, ParseError>
where
    I: Iterator<Item = &'a str>,
{
    let mut request: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut log_dir: Option<PathBuf> = None;
    while let Some(arg) = iter.next() {
        match arg {
            "--request" => {
                request = Some(PathBuf::from(require_value(iter, "--request")?));
            }
            "--out" => {
                output = Some(PathBuf::from(require_value(iter, "--out")?));
            }
            "--log-dir" => {
                log_dir = Some(PathBuf::from(require_value(iter, "--log-dir")?));
            }
            "--help" | "-h" => return Err(ParseError::Help),
            other => {
                return Err(ParseError::Usage(usage_error(
                    ErrorCode::UnknownCommand,
                    &format!("unknown preview flag '{other}'"),
                )));
            }
        }
    }
    let request = request.ok_or_else(|| {
        ParseError::Usage(usage_error(
            ErrorCode::InvalidArgument,
            "preview requires --request",
        ))
    })?;
    let output = output.ok_or_else(|| {
        ParseError::Usage(usage_error(
            ErrorCode::InvalidArgument,
            "preview requires --out",
        ))
    })?;
    Ok(CliAction::Preview {
        request,
        output,
        log_dir,
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
    require_nonempty_label(&label, "probe --label")?;
    if let Some(s) = steps {
        require_positive_steps(s, "probe")?;
    }
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

    #[test]
    fn help_and_capabilities_reject_unknown_flags() {
        match parse_args(&["help", "--nonsense"]).unwrap_err() {
            ParseError::Usage(e) => assert_eq!(e.code(), ErrorCode::UnknownCommand),
            ParseError::Help => panic!("expected usage"),
        }
        match parse_args(&["capabilities", "--nonsense"]).unwrap_err() {
            ParseError::Usage(e) => assert_eq!(e.code(), ErrorCode::UnknownCommand),
            ParseError::Help => panic!("expected usage"),
        }
    }

    #[test]
    fn probe_aliases_reject_empty_label_and_zero_steps_before_session() {
        match parse_args(&["probe", "--label", "", "--steps", "0"]).unwrap_err() {
            ParseError::Usage(e) => assert_eq!(e.code(), ErrorCode::InvalidArgument),
            ParseError::Help => panic!("expected usage"),
        }
        match parse_args(&["probe", "--label", "ds", "--steps", "0"]).unwrap_err() {
            ParseError::Usage(e) => assert_eq!(e.code(), ErrorCode::ValueOutOfRange),
            ParseError::Help => panic!("expected usage"),
        }
        match parse_args(&["probe-load", "--label", ""]).unwrap_err() {
            ParseError::Usage(e) => assert_eq!(e.code(), ErrorCode::InvalidArgument),
            ParseError::Help => panic!("expected usage"),
        }
        match parse_args(&["probe-run", "--steps", "1", "--load-label", ""]).unwrap_err() {
            ParseError::Usage(e) => assert_eq!(e.code(), ErrorCode::InvalidArgument),
            ParseError::Help => panic!("expected usage"),
        }
    }
}
