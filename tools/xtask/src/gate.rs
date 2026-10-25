//! Fixed workspace verification gate (P1-F06).
//!
//! Compile steps pin the rustc host with `--target <host>` so Cargo config /
//! `CARGO_BUILD_TARGET` cannot silently cross-compile. `fmt` stays target-free.

/// One cargo (or git) step in the workspace gate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateStep {
    pub name: &'static str,
    pub program: &'static str,
    pub args: Vec<String>,
}

impl GateStep {
    pub fn arg_refs(&self) -> Vec<&str> {
        self.args.iter().map(String::as_str).collect()
    }

    /// Value following `--target`, if present.
    pub fn target_flag_value(&self) -> Option<&str> {
        let mut args = self.args.iter().map(String::as_str);
        while let Some(arg) = args.next() {
            if arg == "--target" {
                return args.next();
            }
            if let Some(value) = arg.strip_prefix("--target=") {
                return Some(value);
            }
        }
        None
    }
}

/// Build the fixed fmt / check / test / clippy gate for `rustc_host`.
///
/// Feature coverage: when no optional features are declared, `check`/`test`
/// default features and `clippy --all-features` are equivalent. When optional
/// features appear, [`crate::policy::check_workspace_policy`] requires the gate
/// to cover `--all-features` on check, test, and clippy (extend this table;
/// do not invent `--no-default-features`).
pub fn workspace_gate_steps(rustc_host: &str) -> Vec<GateStep> {
    vec![
        GateStep {
            name: "fmt",
            program: "cargo",
            args: vec![
                "fmt".to_owned(),
                "--all".to_owned(),
                "--".to_owned(),
                "--check".to_owned(),
            ],
        },
        GateStep {
            name: "check",
            program: "cargo",
            args: vec![
                "check".to_owned(),
                "--workspace".to_owned(),
                "--target".to_owned(),
                rustc_host.to_owned(),
            ],
        },
        GateStep {
            name: "test",
            program: "cargo",
            args: vec![
                "test".to_owned(),
                "--workspace".to_owned(),
                "--target".to_owned(),
                rustc_host.to_owned(),
            ],
        },
        GateStep {
            name: "clippy",
            program: "cargo",
            args: vec![
                "clippy".to_owned(),
                "--workspace".to_owned(),
                "--all-targets".to_owned(),
                "--all-features".to_owned(),
                "--target".to_owned(),
                rustc_host.to_owned(),
                "--".to_owned(),
                "-D".to_owned(),
                "warnings".to_owned(),
            ],
        },
    ]
}

/// Trailing whitespace / conflict-marker check after the cargo gate.
pub fn diff_gate_step() -> GateStep {
    GateStep {
        name: "diff",
        program: "git",
        args: vec!["diff".to_owned(), "--check".to_owned()],
    }
}

/// Compile steps must pin `rustc_host`; fmt must not take `--target`.
pub fn ensure_compile_steps_pin_host(steps: &[GateStep], rustc_host: &str) -> Result<(), String> {
    let fmt = steps
        .iter()
        .find(|step| step.name == "fmt")
        .ok_or_else(|| "workspace gate is missing the fmt step".to_owned())?;
    if fmt.target_flag_value().is_some() {
        return Err("fmt must not pass --target".to_owned());
    }

    for name in ["check", "test", "clippy"] {
        let step = steps
            .iter()
            .find(|step| step.name == name)
            .ok_or_else(|| format!("workspace gate is missing the {name} step"))?;
        match step.target_flag_value() {
            Some(target) if target == rustc_host => {}
            Some(target) => {
                return Err(format!(
                    "{name} pins --target `{target}` but rustc host is `{rustc_host}`"
                ));
            }
            None => {
                return Err(format!(
                    "{name} must pass `--target {rustc_host}` so Cargo config/env cannot silently cross-compile"
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_gate_pins_host_on_compile_steps_only() {
        let host = "x86_64-pc-windows-msvc";
        let steps = workspace_gate_steps(host);
        let names: Vec<_> = steps.iter().map(|step| step.name).collect();
        assert_eq!(names, ["fmt", "check", "test", "clippy"]);
        ensure_compile_steps_pin_host(&steps, host).unwrap();

        let clippy = steps.iter().find(|step| step.name == "clippy").unwrap();
        assert!(clippy.args.iter().any(|arg| arg == "-D"));
        assert!(clippy.args.iter().any(|arg| arg == "warnings"));
        assert!(clippy.args.iter().any(|arg| arg == "--all-features"));
        assert_eq!(clippy.target_flag_value(), Some(host));

        let fmt = steps.iter().find(|step| step.name == "fmt").unwrap();
        assert!(fmt.target_flag_value().is_none());
    }

    #[test]
    fn mismatched_or_missing_host_pin_fails() {
        let mut steps = workspace_gate_steps("aarch64-apple-darwin");
        assert!(ensure_compile_steps_pin_host(&steps, "x86_64-pc-windows-msvc").is_err());

        // Drop --target from check.
        if let Some(check) = steps.iter_mut().find(|step| step.name == "check") {
            check
                .args
                .retain(|arg| arg != "--target" && arg != "aarch64-apple-darwin");
        }
        assert!(ensure_compile_steps_pin_host(&steps, "aarch64-apple-darwin").is_err());
    }
}
