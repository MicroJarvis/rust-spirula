//! Fixed workspace verification gate (P1-F06).
//!
//! These steps are the single local entry used by `spirula-xtask verify`.
//! They run on the host triple only; they do not pass `--target`.

/// One cargo (or git) step in the workspace gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GateStep {
    pub name: &'static str,
    pub program: &'static str,
    pub args: &'static [&'static str],
}

/// The fixed fmt / check / test / clippy gate, then whitespace diff.
///
/// Feature coverage: when no optional features are declared, `check`/`test`
/// default features and `clippy --all-features` are equivalent. When optional
/// features appear, [`crate::policy::check_workspace_policy`] requires the gate
/// to cover `--all-features` explicitly.
pub fn workspace_gate_steps() -> &'static [GateStep] {
    &[
        GateStep {
            name: "fmt",
            program: "cargo",
            args: &["fmt", "--all", "--", "--check"],
        },
        GateStep {
            name: "check",
            program: "cargo",
            args: &["check", "--workspace"],
        },
        GateStep {
            name: "test",
            program: "cargo",
            args: &["test", "--workspace"],
        },
        GateStep {
            name: "clippy",
            program: "cargo",
            args: &[
                "clippy",
                "--workspace",
                "--all-targets",
                "--all-features",
                "--",
                "-D",
                "warnings",
            ],
        },
    ]
}

/// Trailing whitespace / conflict-marker check after the cargo gate.
pub fn diff_gate_step() -> GateStep {
    GateStep {
        name: "diff",
        program: "git",
        args: &["diff", "--check"],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_gate_names_and_deny_warnings() {
        let names: Vec<_> = workspace_gate_steps()
            .iter()
            .map(|step| step.name)
            .collect();
        assert_eq!(names, ["fmt", "check", "test", "clippy"]);
        let clippy = workspace_gate_steps()
            .iter()
            .find(|step| step.name == "clippy")
            .expect("clippy");
        assert!(clippy.args.contains(&"-D"));
        assert!(clippy.args.contains(&"warnings"));
        assert!(clippy.args.contains(&"--all-features"));
        for step in workspace_gate_steps() {
            assert!(
                !step.args.contains(&"--target"),
                "{} must not pass --target",
                step.name
            );
        }
    }
}
