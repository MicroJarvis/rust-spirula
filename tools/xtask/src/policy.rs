//! Workspace policy checks for P1-F06: lint inheritance, architecture
//! dependency edges, declared feature combinations, and CLI entry-only shape.
//!
//! These checks inspect real Cargo metadata and manifests. They do not invent
//! target triples or claim Windows/cross builds ran.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::gate::workspace_gate_steps;

/// Summary of the workspace policy check.
#[derive(Debug, Clone)]
pub struct PolicyReport {
    pub members: usize,
    pub path_edges: usize,
    pub optional_features: usize,
    pub feature_combinations: Vec<String>,
    pub host_targets_checked: Vec<String>,
}

/// Allowed path dependency edges among workspace crates.
///
/// `spirula-application` is intentionally absent until Phase 6; no member may
/// depend on it, and it must not appear as a workspace member yet.
fn allowed_path_deps(crate_name: &str) -> Option<&'static [&'static str]> {
    Some(match crate_name {
        "spirula-core" => &[],
        "spirula-io" => &["spirula-core"],
        "spirula-gpu" => &[],
        "spirula-render" => &[],
        "spirula-train" => &[],
        "spirula-ui" => &[],
        // Entry adapter only until P6 introduces spirula-application.
        "spirula-cli" => &[],
        // Tooling may read io fixtures; it is not a runtime layer.
        "spirula-xtask" => &["spirula-io"],
        _ => return None,
    })
}

/// Fixed host gate steps plus policy conclusions about feature coverage.
pub fn check_workspace_policy(root: &Path) -> Result<PolicyReport, String> {
    let metadata = cargo_metadata(root)?;
    let packages = workspace_packages(&metadata)?;
    let package_names: BTreeSet<String> = packages.iter().map(|(name, _)| name.clone()).collect();

    if package_names.contains("spirula-application") {
        return Err(
            "spirula-application is present as a workspace member; Phase 6 owns that crate — remove it until P6-F01"
                .to_owned(),
        );
    }

    let mut path_edges = 0;
    let mut optional_features = BTreeSet::new();

    for (name, pkg) in &packages {
        let allowed = allowed_path_deps(name).ok_or_else(|| {
            format!("workspace member `{name}` has no architecture allowlist entry")
        })?;
        let allowed_set: BTreeSet<&str> = allowed.iter().copied().collect();

        let manifest_path = pkg
            .get("manifest_path")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{name}: missing manifest_path in cargo metadata"))?;
        ensure_workspace_lints(Path::new(manifest_path), name)?;

        let deps = path_crate_deps(pkg);
        for dep in &deps {
            path_edges += 1;
            if dep == "spirula-application" {
                return Err(format!(
                    "{name} depends on spirula-application; that crate is absent until Phase 6"
                ));
            }
            if !dep.starts_with("spirula-") && dep != "spirula-xtask" {
                // Non-workspace path deps are unexpected in this repo.
                return Err(format!(
                    "{name} has unexpected path dependency `{dep}` (not a spirula-* crate)"
                ));
            }
            if !allowed_set.contains(dep.as_str()) {
                return Err(format!(
                    "architecture boundary violation: `{name}` → `{dep}` is not an allowed edge"
                ));
            }
            if !package_names.contains(dep) {
                return Err(format!(
                    "{name} depends on `{dep}`, but `{dep}` is not a workspace member"
                ));
            }
        }
        for extra in allowed_set {
            // Allowed but unused edges are fine (skeleton crates).
            let _ = extra;
        }

        for feature in optional_feature_names(pkg) {
            optional_features.insert(format!("{name}/{feature}"));
        }
    }

    check_cli_entry_only(root, &packages)?;
    let combinations = feature_combinations(optional_features.len());
    check_gate_covers_features(&combinations)?;

    // Honest host coverage: only the current host triple is exercised by verify.
    let host = host_target_triple()?;
    let host_targets_checked = vec![host];

    Ok(PolicyReport {
        members: packages.len(),
        path_edges,
        optional_features: optional_features.len(),
        feature_combinations: combinations,
        host_targets_checked,
    })
}

fn cargo_metadata(root: &Path) -> Result<Value, String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(root)
        .output()
        .map_err(|error| format!("could not run cargo metadata: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(|error| format!("cargo metadata JSON: {error}"))
}

fn workspace_packages(metadata: &Value) -> Result<Vec<(String, Value)>, String> {
    let members = metadata
        .get("workspace_members")
        .and_then(Value::as_array)
        .ok_or_else(|| "cargo metadata missing workspace_members".to_owned())?;
    let packages = metadata
        .get("packages")
        .and_then(Value::as_array)
        .ok_or_else(|| "cargo metadata missing packages".to_owned())?;

    let member_ids: BTreeSet<&str> = members.iter().filter_map(Value::as_str).collect();
    let mut out = Vec::new();
    for pkg in packages {
        let id = pkg
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| "package missing id".to_owned())?;
        if !member_ids.contains(id) {
            continue;
        }
        let name = pkg
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("package {id} missing name"))?
            .to_owned();
        out.push((name, pkg.clone()));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    if out.is_empty() {
        return Err("cargo metadata returned no workspace packages".to_owned());
    }
    Ok(out)
}

fn path_crate_deps(pkg: &Value) -> Vec<String> {
    let mut deps = BTreeSet::new();
    let Some(list) = pkg.get("dependencies").and_then(Value::as_array) else {
        return Vec::new();
    };
    for dep in list {
        let Some(name) = dep.get("name").and_then(Value::as_str) else {
            continue;
        };
        // Path deps in metadata have a non-null `path` (or source absent for path).
        let is_path = dep.get("path").and_then(Value::as_str).is_some()
            || dep
                .get("source")
                .map(|source| source.is_null())
                .unwrap_or(false);
        if is_path && name.starts_with("spirula-") {
            deps.insert(name.to_owned());
        }
    }
    deps.into_iter().collect()
}

fn optional_feature_names(pkg: &Value) -> Vec<String> {
    let Some(features) = pkg.get("features").and_then(Value::as_object) else {
        return Vec::new();
    };
    features
        .keys()
        .filter(|name| name.as_str() != "default")
        .cloned()
        .collect()
}

fn feature_combinations(optional_count: usize) -> Vec<String> {
    let mut combos = vec!["default".to_owned()];
    if optional_count > 0 {
        combos.push("all-features".to_owned());
    }
    combos
}

fn check_gate_covers_features(combinations: &[String]) -> Result<(), String> {
    let steps = workspace_gate_steps();
    let clippy = steps
        .iter()
        .find(|step| step.name == "clippy")
        .ok_or_else(|| "workspace gate is missing the clippy step".to_owned())?;
    if combinations.iter().any(|name| name == "all-features")
        && !clippy.args.contains(&"--all-features")
    {
        return Err(
            "optional crate features are declared, but the clippy gate does not pass --all-features"
                .to_owned(),
        );
    }
    // When optional features exist, check/test must also cover --all-features.
    if combinations.iter().any(|name| name == "all-features") {
        let check = steps
            .iter()
            .find(|step| step.name == "check")
            .ok_or_else(|| "workspace gate is missing the check step".to_owned())?;
        if !check.args.contains(&"--all-features")
            && !steps.iter().any(|step| {
                step.name == "check-all-features" && step.args.contains(&"--all-features")
            })
        {
            return Err(
                "optional crate features are declared, but no check gate covers --all-features"
                    .to_owned(),
            );
        }
    }
    // Never claim a foreign target was part of the fixed gate.
    for step in steps {
        if step.args.contains(&"--target") {
            return Err(format!(
                "workspace gate step `{}` passes --target; host-only verify must not pretend cross targets ran",
                step.name
            ));
        }
    }
    Ok(())
}

fn ensure_workspace_lints(manifest_path: &Path, package: &str) -> Result<(), String> {
    let text = fs::read_to_string(manifest_path)
        .map_err(|error| format!("reading {}: {error}", manifest_path.display()))?;
    if !manifest_inherits_workspace_lints(&text) {
        return Err(format!(
            "{package}: Cargo.toml must inherit workspace lints with `[lints]` / `workspace = true`"
        ));
    }
    Ok(())
}

fn manifest_inherits_workspace_lints(text: &str) -> bool {
    let mut in_lints = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_lints = trimmed == "[lints]";
            continue;
        }
        if in_lints && trimmed == "workspace = true" {
            return true;
        }
    }
    false
}

fn check_cli_entry_only(root: &Path, packages: &[(String, Value)]) -> Result<(), String> {
    let cli = packages
        .iter()
        .find(|(name, _)| name == "spirula-cli")
        .ok_or_else(|| "spirula-cli is missing from the workspace".to_owned())?;
    let deps = path_crate_deps(&cli.1);
    if !deps.is_empty() {
        return Err(format!(
            "spirula-cli must stay an entry adapter until spirula-application exists; found path deps {deps:?}"
        ));
    }

    let src = root.join("crates/spirula-cli/src");
    let mut files = list_rust_files(&src)?;
    files.sort();
    if files != [src.join("main.rs")] {
        return Err(format!(
            "spirula-cli must contain only src/main.rs until application lands; found {files:?}"
        ));
    }

    let main = fs::read_to_string(src.join("main.rs"))
        .map_err(|error| format!("reading spirula-cli main: {error}"))?;
    // Structural bans: no module tree and no direct pulls of domain crates.
    if main.contains("mod ") {
        return Err("spirula-cli main.rs must not declare modules yet".to_owned());
    }
    for banned in [
        "spirula_io",
        "spirula_train",
        "spirula_render",
        "spirula_gpu",
        "spirula_ui",
        "spirula_application",
    ] {
        if main.contains(banned) {
            return Err(format!(
                "spirula-cli main.rs references `{banned}`; keep business logic out of the entry adapter"
            ));
        }
    }
    Ok(())
}

fn list_rust_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).map_err(|error| format!("reading {}: {error}", dir.display()))? {
        let entry = entry.map_err(|error| format!("reading {}: {error}", dir.display()))?;
        let path = entry.path();
        if path.is_dir() {
            return Err(format!(
                "spirula-cli must not nest source directories yet ({})",
                path.display()
            ));
        }
        if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
    Ok(files)
}

fn host_target_triple() -> Result<String, String> {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_owned());
    let output = Command::new(rustc)
        .args(["-vV"])
        .output()
        .map_err(|error| format!("could not run rustc -vV: {error}"))?;
    if !output.status.success() {
        return Err("rustc -vV failed".to_owned());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        if let Some(triple) = line.strip_prefix("host: ") {
            return Ok(triple.trim().to_owned());
        }
    }
    Err("rustc -vV did not report host".to_owned())
}

/// Parse path edges from a metadata package map for unit tests.
pub fn allowed_edge_table() -> BTreeMap<&'static str, &'static [&'static str]> {
    [
        "spirula-core",
        "spirula-io",
        "spirula-gpu",
        "spirula-render",
        "spirula-train",
        "spirula-ui",
        "spirula-cli",
        "spirula-xtask",
    ]
    .into_iter()
    .map(|name| (name, allowed_path_deps(name).expect("allowlisted")))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lint_inheritance_parser_accepts_workspace_table() {
        assert!(manifest_inherits_workspace_lints(
            "[package]\nname = \"x\"\n\n[lints]\nworkspace = true\n"
        ));
        assert!(!manifest_inherits_workspace_lints(
            "[package]\nname = \"x\"\n\n[lints.rust]\nunsafe_code = \"deny\"\n"
        ));
    }

    #[test]
    fn architecture_allowlist_keeps_cli_and_core_isolated() {
        let table = allowed_edge_table();
        assert!(table["spirula-core"].is_empty());
        assert!(table["spirula-cli"].is_empty());
        assert_eq!(table["spirula-io"], &["spirula-core"]);
        assert!(!table.contains_key("spirula-application"));
    }

    #[test]
    fn feature_combinations_stay_default_without_optional_features() {
        assert_eq!(feature_combinations(0), vec!["default".to_owned()]);
        assert_eq!(
            feature_combinations(2),
            vec!["default".to_owned(), "all-features".to_owned()]
        );
    }

    #[test]
    fn gate_rejects_target_flags_and_requires_all_features_when_needed() {
        assert!(check_gate_covers_features(&["default".to_owned()]).is_ok());
        let err = check_gate_covers_features(&["default".to_owned(), "all-features".to_owned()]);
        // Current gate clippy has --all-features but check does not; with
        // optional features that must fail until the gate grows.
        assert!(err.is_err(), "{err:?}");
    }
}
