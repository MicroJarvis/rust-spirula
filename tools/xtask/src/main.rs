//! `spirula-xtask`: development verification entry point.
//!
//! Commands:
//!   fixtures  verify the compatibility fixture set
//!   docs      verify project-plan status, links, artifacts and error codes
//!   policy    verify lint inheritance, architecture edges, features, CLI shape
//!   baseline  verify the pinned C++ reference checkout (skipped when absent)
//!   verify    run fixtures + docs + policy + baseline note + the workspace gate

use std::env;
use std::path::Path;
use std::process::{Command, ExitCode};
use std::time::Instant;

use spirula_xtask::{
    check_docs, check_workspace_policy, diff_gate_step, workspace_gate_steps, workspace_root,
};

fn main() -> ExitCode {
    let command = env::args().nth(1).unwrap_or_else(|| "verify".to_owned());
    match command.as_str() {
        "verify" => run_verify(),
        "fixtures" => run_fixtures(),
        "docs" => run_docs(),
        "policy" => run_policy(),
        "baseline" => run_baseline(),
        "-h" | "--help" | "help" => {
            print_usage();
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("error: unknown command `{other}`");
            print_usage();
            ExitCode::from(2)
        }
    }
}

fn print_usage() {
    println!(
        "spirula-xtask <command>

  verify    run fixture, documentation, policy and workspace checks (default)
  fixtures  verify fixtures/manifest.json and every fixture format
  docs      verify project-plan status, links, artifacts and error codes
  policy    verify lint inheritance, architecture edges, features and CLI shape
  baseline  verify the pinned C++ reference checkout, if present

Environment:
  SPIRULA_REFERENCE_REPO  C++ reference checkout (default: ../spirula-studio)
  SPIRULA_FIXTURES        fixture root (default: <workspace>/fixtures)"
    );
}

fn run_fixtures() -> ExitCode {
    match spirula_xtask::fixture_check() {
        Ok(summary) => {
            println!("fixtures: ok ({summary})");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("fixtures: failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_docs() -> ExitCode {
    let root = workspace_root();
    match check_docs(&root) {
        Ok(report) => {
            println!(
                "docs: ok ({} features, {} files, {} links, {} error codes)",
                report.features, report.markdown_files, report.links, report.error_codes
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("docs: failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_policy() -> ExitCode {
    let root = workspace_root();
    match check_workspace_policy(&root) {
        Ok(report) => {
            let cargo_target = report
                .cargo_build_target
                .as_deref()
                .unwrap_or("(unset, rustc host)");
            println!(
                "policy: ok ({} members, {} path edges, {} optional features, combinations [{}], rustc host [{}], cargo build.target [{}], checked [{}])",
                report.members,
                report.path_edges,
                report.optional_features,
                report.feature_combinations.join(", "),
                report.rustc_host,
                cargo_target,
                report.targets_checked.join(", ")
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("policy: failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_baseline() -> ExitCode {
    let root = workspace_root();
    let repo = reference_repo(&root);
    if !repo.join(".git").exists() {
        println!(
            "baseline: skipped (no reference checkout at {})",
            repo.display()
        );
        return ExitCode::SUCCESS;
    }
    let script = root.join("tools/reference-baseline.sh");
    match Command::new("bash")
        .arg(&script)
        .arg("check")
        .env("SPIRULA_REFERENCE_REPO", &repo)
        .status()
    {
        Ok(status) if status.success() => {
            println!("baseline: ok");
            ExitCode::SUCCESS
        }
        Ok(status) => {
            eprintln!("baseline: failed with {status}; see docs/phase-0/reference-baseline.md");
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("baseline: could not run bash: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_verify() -> ExitCode {
    let root = workspace_root();
    let started = Instant::now();
    let mut failures = 0;

    if run_fixtures() != ExitCode::SUCCESS {
        failures += 1;
    }
    if run_docs() != ExitCode::SUCCESS {
        failures += 1;
    }
    if run_policy() != ExitCode::SUCCESS {
        failures += 1;
    }
    baseline_note(&root);

    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let gate = workspace_gate_steps();
    for step in gate {
        let program = if step.program == "cargo" {
            cargo.as_str()
        } else {
            step.program
        };
        if !run_tool(step.name, program, step.args, &root) {
            failures += 1;
        }
    }
    if root.join(".git").exists() {
        let diff = diff_gate_step();
        if !run_tool(diff.name, diff.program, diff.args, &root) {
            failures += 1;
        }
    }

    // fixtures + docs + policy + gate steps + diff
    let reported = gate.len() + 4;
    if failures == 0 {
        println!(
            "verify: ok ({reported} checks passed in {:.1}s)",
            started.elapsed().as_secs_f64()
        );
        ExitCode::SUCCESS
    } else {
        eprintln!("verify: {failures} check(s) failed");
        ExitCode::FAILURE
    }
}

fn baseline_note(root: &Path) {
    let repo = reference_repo(root);
    if !repo.join(".git").exists() {
        println!(
            "reference baseline: skipped (no checkout at {})",
            repo.display()
        );
        return;
    }
    let script = root.join("tools/reference-baseline.sh");
    match Command::new("bash")
        .arg(&script)
        .arg("check")
        .env("SPIRULA_REFERENCE_REPO", &repo)
        .output()
    {
        Ok(output) if output.status.success() => println!("reference baseline: ok"),
        Ok(output) => println!(
            "reference baseline: warning (not a verify failure)\n{}",
            String::from_utf8_lossy(&output.stderr).trim_end()
        ),
        Err(error) => println!("reference baseline: skipped ({error})"),
    }
}

fn reference_repo(root: &Path) -> std::path::PathBuf {
    if let Some(path) = env::var_os("SPIRULA_REFERENCE_REPO") {
        return path.into();
    }
    // A git worktree lives below the repository root, so a single `..` is not
    // enough; look for a sibling `spirula-studio` checkout up the tree.
    for base in root.ancestors().take(5) {
        let candidate = base.join("spirula-studio");
        if candidate.join(".git").exists() {
            return candidate;
        }
    }
    root.join("../spirula-studio")
}

fn run_tool(name: &str, program: &str, args: &[&str], cwd: &Path) -> bool {
    println!("== {name} ==");
    let started = Instant::now();
    match Command::new(program).args(args).current_dir(cwd).status() {
        Ok(status) if status.success() => {
            println!("{name}: ok ({:.1}s)", started.elapsed().as_secs_f64());
            true
        }
        Ok(status) => {
            eprintln!(
                "{name}: failed (exit {})",
                status
                    .code()
                    .map_or_else(|| "signal".to_owned(), |c| c.to_string())
            );
            false
        }
        Err(error) => {
            eprintln!("{name}: could not run {program}: {error}");
            false
        }
    }
}
