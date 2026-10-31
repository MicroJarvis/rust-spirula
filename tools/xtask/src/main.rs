//! `spirula-xtask`: development verification entry point.
//!
//! Commands:
//!   fixtures  verify the compatibility fixture set
//!   docs      verify project-plan status, links, artifacts and error codes
//!   policy    verify lint inheritance, architecture edges, features, CLI shape
//!   baseline  verify the pinned C++ reference checkout (skipped when absent)
//!   package   build the P2-F07 minimal CLI package (clean HEAD or --from-ref)
//!   package-check  verify package hashes / layout
//!   package-run    isolated help/capabilities/probe + GPU smoke
//!   verify    run fixtures + docs + policy + baseline note + the workspace gate

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Instant;

use spirula_xtask::{
    PackageOptions, build_package, check_docs, check_package, check_workspace_policy,
    diff_gate_step, run_package_isolated, rustc_host_triple, workspace_gate_steps, workspace_root,
};

fn main() -> ExitCode {
    let command = env::args().nth(1).unwrap_or_else(|| "verify".to_owned());
    match command.as_str() {
        "verify" => run_verify(),
        "fixtures" => run_fixtures(),
        "docs" => run_docs(),
        "policy" => run_policy(),
        "baseline" => run_baseline(),
        "package" => run_package(),
        "package-check" => run_package_check(),
        "package-run" => run_package_run(),
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

  verify         run fixture, documentation, policy and workspace checks (default)
  fixtures       verify fixtures/manifest.json and every fixture format
  docs           verify project-plan status, links, artifacts and error codes
  policy         verify lint inheritance, architecture edges, features and CLI shape
  baseline       verify the pinned C++ reference checkout, if present
  package        build minimal CLI package (--out DIR [--from-ref SHA] [--profile release|dev])
  package-check  verify package MANIFEST hashes and forbidden entries (--package DIR)
  package-run    isolated help/capabilities/probe + compute smoke (--package DIR)

Environment:
  SPIRULA_REFERENCE_REPO  C++ reference checkout (default: ../spirula-studio)
  SPIRULA_FIXTURES        fixture root (default: <workspace>/fixtures)"
    );
}

fn run_package() -> ExitCode {
    let args: Vec<String> = env::args().skip(2).collect();
    let mut out = None;
    let mut from_ref = None;
    let mut profile = "release".to_owned();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--out" => {
                let v = args.get(i + 1).cloned();
                if v.is_none() {
                    eprintln!("error: --out requires a path");
                    return ExitCode::from(2);
                }
                out = v;
                i += 2;
            }
            "--from-ref" => {
                let v = args.get(i + 1).cloned();
                if v.is_none() {
                    eprintln!("error: --from-ref requires a revision");
                    return ExitCode::from(2);
                }
                from_ref = v;
                i += 2;
            }
            "--profile" => {
                let v = args.get(i + 1).cloned();
                if v.is_none() {
                    eprintln!("error: --profile requires release|dev");
                    return ExitCode::from(2);
                }
                profile = v.unwrap();
                i += 2;
            }
            "-h" | "--help" => {
                print_usage();
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("error: unknown package flag `{other}`");
                return ExitCode::from(2);
            }
        }
    }
    let Some(out) = out else {
        eprintln!("error: package requires --out DIR");
        return ExitCode::from(2);
    };
    let opts = PackageOptions {
        out: PathBuf::from(out),
        workspace: workspace_root(),
        from_ref,
        profile,
    };
    match build_package(&opts) {
        Ok(outcome) => {
            println!(
                "package: ok revision={} out={}",
                outcome.revision,
                outcome.package_dir.display()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("package: failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_package_check() -> ExitCode {
    let args: Vec<String> = env::args().skip(2).collect();
    let package = match package_dir_flag(&args) {
        Ok(p) => p,
        Err(code) => return code,
    };
    match check_package(&package) {
        Ok(manifest) => {
            let rev = manifest
                .get("revision")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            println!(
                "package-check: ok revision={rev} path={}",
                package.display()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("package-check: failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_package_run() -> ExitCode {
    let args: Vec<String> = env::args().skip(2).collect();
    let package = match package_dir_flag(&args) {
        Ok(p) => p,
        Err(code) => return code,
    };
    let isolate_parent = std::env::temp_dir().join("spirula-pkg-isolate");
    match run_package_isolated(&package, &isolate_parent) {
        Ok(report) => {
            let preview = match &report.preview {
                spirula_xtask::IsolatedPreviewOutcome::Unavailable { reason } => {
                    format!("unavailable({reason})")
                }
                spirula_xtask::IsolatedPreviewOutcome::Ran {
                    exit,
                    width,
                    height,
                    rgb8_len,
                } => format!("ran exit={exit} {width}x{height} rgb8={rgb8_len}"),
            };
            println!(
                "package-run: ok preview={preview} smoke_status={} counted_as_pass={} match={:?} current={:?} isolate={}",
                report.smoke_status,
                report.smoke_counted_as_pass,
                report.smoke_match,
                report.smoke_current,
                report.isolate.display()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("package-run: failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn package_dir_flag(args: &[String]) -> Result<PathBuf, ExitCode> {
    let mut package = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--package" => {
                let v = args.get(i + 1).cloned();
                if v.is_none() {
                    eprintln!("error: --package requires a path");
                    return Err(ExitCode::from(2));
                }
                package = v;
                i += 2;
            }
            "-h" | "--help" => {
                print_usage();
                return Err(ExitCode::SUCCESS);
            }
            other => {
                eprintln!("error: unknown flag `{other}`");
                return Err(ExitCode::from(2));
            }
        }
    }
    package.map(PathBuf::from).ok_or_else(|| {
        eprintln!("error: requires --package DIR");
        ExitCode::from(2)
    })
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
            println!(
                "policy: ok ({} members, {} path edges, {} optional features, combinations [{}], rustc host [{}], gate --target [{}])",
                report.members,
                report.path_edges,
                report.optional_features,
                report.feature_combinations.join(", "),
                report.rustc_host,
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
    let rustc_host = match rustc_host_triple() {
        Ok(host) => host,
        Err(error) => {
            eprintln!("verify: could not resolve rustc host: {error}");
            return ExitCode::FAILURE;
        }
    };
    let gate = workspace_gate_steps(&rustc_host);
    for step in &gate {
        let program = if step.program == "cargo" {
            cargo.as_str()
        } else {
            step.program
        };
        let args = step.arg_refs();
        if !run_tool(step.name, program, &args, &root) {
            failures += 1;
        }
    }
    if root.join(".git").exists() {
        let diff = diff_gate_step();
        let args = diff.arg_refs();
        if !run_tool(diff.name, diff.program, &args, &root) {
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
