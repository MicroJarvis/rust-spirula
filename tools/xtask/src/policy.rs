//! Workspace policy checks for P1-F06: lint inheritance, target-architecture
//! dependency edges, declared feature combinations, and CLI dependency shape.
//!
//! These checks inspect real Cargo metadata and manifests. They do not invent
//! target triples or claim Windows/cross builds ran. They do not AST-scan CLI
//! sources for “business logic”; CLI limits are dependency boundaries plus a
//! documented skeleton placeholder.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
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
    /// `rustc -vV` host triple (may be a Windows host).
    pub rustc_host: String,
    /// Explicit Cargo build target from `CARGO_BUILD_TARGET` or config, if any.
    pub cargo_build_target: Option<String>,
    /// Targets the host-only gate actually exercises (equals [`Self::rustc_host`]
    /// when the host gate is satisfied).
    pub targets_checked: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DepKind {
    Normal,
    Dev,
    Build,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DepRef {
    /// Cargo package name (identity), not the import rename.
    name: String,
    rename: Option<String>,
    kind: DepKind,
    is_path: bool,
}

/// Target-architecture allowed **workspace** path edges.
///
/// Unused edges are fine (skeleton crates). Planned crates such as
/// `spirula-application` are listed so they validate when added; they are not
/// banned until Phase 6.
fn allowed_workspace_deps(crate_name: &str) -> Option<&'static [&'static str]> {
    Some(match crate_name {
        "spirula-core" => &[],
        "spirula-io" => &["spirula-core"],
        "spirula-gpu" => &["spirula-core"],
        "spirula-render" => &["spirula-core", "spirula-gpu"],
        // Architecture text: train calls render and GPU through explicit
        // interfaces; diagram is train → render → gpu.
        "spirula-train" => &[
            "spirula-core",
            "spirula-io",
            "spirula-render",
            "spirula-gpu",
        ],
        "spirula-application" => &["spirula-core", "spirula-io", "spirula-train"],
        "spirula-ui" => &["spirula-core", "spirula-application"],
        "spirula-cli" => &["spirula-core", "spirula-application"],
        // Tooling only; not a runtime layer.
        "spirula-xtask" => &["spirula-io", "spirula-core"],
        _ => return None,
    })
}

/// External crates that `spirula-core` must never take (normal/dev/build).
fn core_forbidden_external(name: &str) -> bool {
    matches!(
        name,
        "wgpu"
            | "slint"
            | "ash"
            | "metal"
            | "glow"
            | "glutin"
            | "winit"
            | "tokio"
            | "async-std"
            | "nix"
            | "windows"
            | "winapi"
            | "objc"
            | "objc2"
            | "cocoa"
            | "gtk"
            | "tao"
    ) || name.starts_with("wgpu-")
        || name.starts_with("slint-")
}

/// Workspace crates UI must not depend on directly (use application instead).
fn ui_forbidden_workspace(name: &str) -> bool {
    matches!(
        name,
        "spirula-gpu" | "spirula-io" | "spirula-train" | "spirula-render"
    )
}

/// Fixed host gate steps plus policy conclusions about feature coverage.
pub fn check_workspace_policy(root: &Path) -> Result<PolicyReport, String> {
    let metadata = cargo_metadata(root)?;
    let packages = workspace_packages(&metadata)?;
    let package_names: BTreeSet<String> = packages.iter().map(|(name, _)| name.clone()).collect();

    let mut path_edges = 0;
    let mut optional_features = BTreeSet::new();

    for (name, pkg) in &packages {
        let manifest_path = pkg
            .get("manifest_path")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{name}: missing manifest_path in cargo metadata"))?;
        ensure_workspace_lints(Path::new(manifest_path), name)?;

        let deps = collect_deps(pkg);
        path_edges += deps.iter().filter(|dep| dep.is_path).count();
        validate_member_dependencies(name, &deps, &package_names)?;

        for feature in optional_feature_names(pkg) {
            optional_features.insert(format!("{name}/{feature}"));
        }
    }

    let combinations = feature_combinations(optional_features.len());
    check_gate_covers_features(&combinations)?;

    let rustc_host = rustc_host_triple()?;
    let cargo_build_target = discover_cargo_build_target(root, &TargetLookup::from_process())?;
    ensure_host_only_gate(&rustc_host, cargo_build_target.as_deref())?;

    Ok(PolicyReport {
        members: packages.len(),
        path_edges,
        optional_features: optional_features.len(),
        feature_combinations: combinations,
        targets_checked: vec![rustc_host.clone()],
        rustc_host,
        cargo_build_target,
    })
}

/// Process environment inputs for Cargo target resolution (injectable in tests).
#[derive(Clone, Debug, Default)]
pub struct TargetLookup {
    pub cargo_build_target: Option<String>,
    pub cargo_home: Option<std::path::PathBuf>,
}

impl TargetLookup {
    fn from_process() -> Self {
        Self {
            cargo_build_target: std::env::var("CARGO_BUILD_TARGET").ok(),
            cargo_home: std::env::var_os("CARGO_HOME").map(std::path::PathBuf::from),
        }
    }
}

/// Resolve Cargo's effective default build target without `--target` on the CLI.
///
/// Precedence matches Cargo when the gate omits `--target`:
/// `CARGO_BUILD_TARGET`, then the nearest project `.cargo/config(.toml)`, then
/// `$CARGO_HOME/config(.toml)`.
pub fn discover_cargo_build_target(
    workspace_root: &Path,
    lookup: &TargetLookup,
) -> Result<Option<String>, String> {
    if let Some(target) = lookup
        .cargo_build_target
        .as_deref()
        .map(str::trim)
        .filter(|target| !target.is_empty())
    {
        return Ok(Some(target.to_owned()));
    }

    for dir in workspace_root.ancestors() {
        if let Some(target) = read_build_target_from_cargo_dir(&dir.join(".cargo"))? {
            return Ok(Some(target));
        }
        if dir.parent().is_none() {
            break;
        }
    }

    if let Some(home) = &lookup.cargo_home
        && let Some(target) = read_build_target_from_cargo_dir(home)?
    {
        return Ok(Some(target));
    }
    Ok(None)
}

fn read_build_target_from_cargo_dir(cargo_dir: &Path) -> Result<Option<String>, String> {
    for name in ["config.toml", "config"] {
        let path = cargo_dir.join(name);
        if !path.is_file() {
            continue;
        }
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("reading {}: {error}", path.display()))?;
        if let Some(target) = parse_build_target_from_config(&text) {
            return Ok(Some(target));
        }
    }
    Ok(None)
}

/// Parse `[build] target = "..."` from a Cargo config file body.
pub fn parse_build_target_from_config(text: &str) -> Option<String> {
    let mut in_build = false;
    for line in text.lines() {
        let trimmed = line.split('#').next().unwrap_or("").trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('[') {
            in_build = trimmed == "[build]";
            continue;
        }
        if !in_build {
            continue;
        }
        let Some(rest) = trimmed.strip_prefix("target") else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        return parse_toml_string_value(rest.trim());
    }
    None
}

fn parse_toml_string_value(value: &str) -> Option<String> {
    let value = value.trim();
    if let Some(inner) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
        return Some(inner.to_owned());
    }
    if let Some(inner) = value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
        return Some(inner.to_owned());
    }
    // Bare identifiers are unusual for triples; accept non-empty tokens.
    if !value.is_empty() && !value.starts_with('{') && !value.starts_with('[') {
        return Some(value.to_owned());
    }
    None
}

/// The fixed gate omits `--target`, so Cargo must not be steered to another
/// triple via env/config. Matching the rustc host (including Windows hosts) is
/// allowed; a different override fails the policy.
pub fn ensure_host_only_gate(
    rustc_host: &str,
    cargo_build_target: Option<&str>,
) -> Result<(), String> {
    match cargo_build_target {
        None => Ok(()),
        Some(target) if target == rustc_host => Ok(()),
        Some(target) => Err(format!(
            "workspace verify gate is host-only (steps omit --target), but Cargo would build for `{target}` while rustc host is `{rustc_host}` \
             (CARGO_BUILD_TARGET or .cargo/config build.target). Unset the override for verify, or add an explicit cross-target gate — do not claim host checks while a cross target is configured"
        )),
    }
}

fn validate_member_dependencies(
    crate_name: &str,
    deps: &[DepRef],
    members: &BTreeSet<String>,
) -> Result<(), String> {
    let allowed = allowed_workspace_deps(crate_name).ok_or_else(|| {
        format!("workspace member `{crate_name}` has no architecture allowlist entry")
    })?;
    let allowed_set: BTreeSet<&str> = allowed.iter().copied().collect();

    for dep in deps {
        let label = dep_label(dep);

        if dep.is_path {
            // Path deps are local packages: must be known workspace members and
            // on the allowlist. Aliases (`rename`) do not change package identity.
            if !dep.name.starts_with("spirula-") {
                return Err(format!(
                    "{crate_name}: path dependency {label} is not a spirula-* workspace crate"
                ));
            }
            if !members.contains(&dep.name) {
                return Err(format!(
                    "{crate_name}: path dependency {label} is not a workspace member"
                ));
            }
            if crate_name == "spirula-ui" && ui_forbidden_workspace(&dep.name) {
                return Err(format!(
                    "spirula-ui must not depend on `{}` directly ({}); route through spirula-application",
                    dep.name,
                    kind_name(dep.kind)
                ));
            }
            if !allowed_set.contains(dep.name.as_str()) {
                return Err(format!(
                    "architecture boundary violation: `{crate_name}` → `{}` ({}) is not an allowed edge",
                    dep.name,
                    kind_name(dep.kind)
                ));
            }
        } else {
            // External / registry / git deps.
            if crate_name == "spirula-core" && core_forbidden_external(&dep.name) {
                return Err(format!(
                    "spirula-core must not depend on {label}; domain layer stays free of GPU/UI/OS runtimes"
                ));
            }
            // Non-path spirula-* should not appear as registry deps in this repo.
            if dep.name.starts_with("spirula-") {
                return Err(format!(
                    "{crate_name}: spirula-* dependency {label} must be a path workspace member"
                ));
            }
        }
    }
    Ok(())
}

fn dep_label(dep: &DepRef) -> String {
    match &dep.rename {
        Some(rename) => format!(
            "`{}` (package `{}`, {})",
            rename,
            dep.name,
            kind_name(dep.kind)
        ),
        None => format!("`{}` ({})", dep.name, kind_name(dep.kind)),
    }
}

fn kind_name(kind: DepKind) -> &'static str {
    match kind {
        DepKind::Normal => "normal",
        DepKind::Dev => "dev",
        DepKind::Build => "build",
    }
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

fn collect_deps(pkg: &Value) -> Vec<DepRef> {
    let mut deps = Vec::new();
    let Some(list) = pkg.get("dependencies").and_then(Value::as_array) else {
        return deps;
    };
    for dep in list {
        let Some(name) = dep.get("name").and_then(Value::as_str) else {
            continue;
        };
        let kind = match dep.get("kind").and_then(Value::as_str) {
            Some("dev") => DepKind::Dev,
            Some("build") => DepKind::Build,
            _ => DepKind::Normal,
        };
        let rename = dep.get("rename").and_then(Value::as_str).map(str::to_owned);
        // Path members have `source: null`. Registry/git deps have a source URL.
        // Prefer an explicit `path` field when cargo metadata provides one.
        let is_path = dep.get("path").and_then(Value::as_str).is_some()
            || dep.get("source").is_some_and(Value::is_null);
        deps.push(DepRef {
            name: name.to_owned(),
            rename,
            kind,
            is_path,
        });
    }
    deps
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

/// Declared meaningful combinations only — never invent `--no-default-features`
/// or placeholder feature names.
fn feature_combinations(optional_count: usize) -> Vec<String> {
    let mut combos = vec!["default".to_owned()];
    if optional_count > 0 {
        combos.push("all-features".to_owned());
    }
    combos
}

fn cargo_gate_covers(steps: &[crate::gate::GateStep], name_prefix: &str, flag: &str) -> bool {
    steps.iter().any(|step| {
        step.program == "cargo" && step.name.starts_with(name_prefix) && step.args.contains(&flag)
    })
}

fn check_gate_covers_features(combinations: &[String]) -> Result<(), String> {
    let steps = workspace_gate_steps();
    for step in steps {
        if step.args.contains(&"--target") {
            return Err(format!(
                "workspace gate step `{}` passes --target; host-only verify must not pretend cross targets ran",
                step.name
            ));
        }
    }

    if !combinations.iter().any(|name| name == "all-features") {
        return Ok(());
    }

    if !cargo_gate_covers(steps, "clippy", "--all-features") {
        return Err(
            "optional crate features are declared, but no clippy gate covers --all-features"
                .to_owned(),
        );
    }
    if !cargo_gate_covers(steps, "check", "--all-features") {
        return Err(
            "optional crate features are declared, but no check gate covers --all-features"
                .to_owned(),
        );
    }
    if !cargo_gate_covers(steps, "test", "--all-features") {
        return Err(
            "optional crate features are declared, but no test gate covers --all-features"
                .to_owned(),
        );
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

fn rustc_host_triple() -> Result<String, String> {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_owned());
    let output = Command::new(rustc)
        .args(["-vV"])
        .output()
        .map_err(|error| format!("could not run rustc -vV: {error}"))?;
    if !output.status.success() {
        return Err("rustc -vV failed".to_owned());
    }
    parse_rustc_host(&String::from_utf8_lossy(&output.stdout))
        .ok_or_else(|| "rustc -vV did not report host".to_owned())
}

fn parse_rustc_host(verbose: &str) -> Option<String> {
    for line in verbose.lines() {
        if let Some(triple) = line.strip_prefix("host: ") {
            let triple = triple.trim();
            if !triple.is_empty() {
                return Some(triple.to_owned());
            }
        }
    }
    None
}

/// Target allowlist table for docs/tests.
pub fn allowed_edge_table() -> BTreeMap<&'static str, &'static [&'static str]> {
    [
        "spirula-core",
        "spirula-io",
        "spirula-gpu",
        "spirula-render",
        "spirula-train",
        "spirula-application",
        "spirula-ui",
        "spirula-cli",
        "spirula-xtask",
    ]
    .into_iter()
    .map(|name| (name, allowed_workspace_deps(name).expect("allowlisted")))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn members(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    fn path_dep(name: &str, kind: DepKind) -> DepRef {
        DepRef {
            name: name.to_owned(),
            rename: None,
            kind,
            is_path: true,
        }
    }

    fn path_dep_renamed(package: &str, rename: &str, kind: DepKind) -> DepRef {
        DepRef {
            name: package.to_owned(),
            rename: Some(rename.to_owned()),
            kind,
            is_path: true,
        }
    }

    fn external_dep(name: &str, kind: DepKind) -> DepRef {
        DepRef {
            name: name.to_owned(),
            rename: None,
            kind,
            is_path: false,
        }
    }

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
    fn target_allowlist_matches_architecture() {
        let table = allowed_edge_table();
        assert!(table["spirula-core"].is_empty());
        assert_eq!(table["spirula-io"], &["spirula-core"]);
        assert_eq!(table["spirula-gpu"], &["spirula-core"]);
        assert_eq!(table["spirula-render"], &["spirula-core", "spirula-gpu"]);
        assert!(table["spirula-train"].contains(&"spirula-io"));
        assert!(table["spirula-train"].contains(&"spirula-gpu"));
        assert_eq!(
            table["spirula-ui"],
            &["spirula-core", "spirula-application"]
        );
        assert_eq!(
            table["spirula-cli"],
            &["spirula-core", "spirula-application"]
        );
        assert!(table.contains_key("spirula-application"));
    }

    #[test]
    fn mutated_metadata_rejects_forbidden_edges() {
        let workspace = members(&[
            "spirula-core",
            "spirula-io",
            "spirula-gpu",
            "spirula-render",
            "spirula-train",
            "spirula-ui",
            "spirula-cli",
            "spirula-application",
        ]);

        assert!(
            validate_member_dependencies(
                "spirula-gpu",
                &[path_dep("spirula-core", DepKind::Normal)],
                &workspace
            )
            .is_ok()
        );
        assert!(
            validate_member_dependencies(
                "spirula-render",
                &[
                    path_dep("spirula-core", DepKind::Normal),
                    path_dep("spirula-gpu", DepKind::Normal),
                ],
                &workspace
            )
            .is_ok()
        );
        assert!(
            validate_member_dependencies(
                "spirula-train",
                &[
                    path_dep("spirula-core", DepKind::Normal),
                    path_dep("spirula-io", DepKind::Normal),
                    path_dep("spirula-render", DepKind::Normal),
                    path_dep("spirula-gpu", DepKind::Normal),
                ],
                &workspace
            )
            .is_ok()
        );
        assert!(
            validate_member_dependencies(
                "spirula-cli",
                &[
                    path_dep("spirula-core", DepKind::Normal),
                    path_dep("spirula-application", DepKind::Normal),
                ],
                &workspace
            )
            .is_ok()
        );

        let core_wgpu = validate_member_dependencies(
            "spirula-core",
            &[external_dep("wgpu", DepKind::Normal)],
            &workspace,
        );
        assert!(core_wgpu.is_err(), "{core_wgpu:?}");

        let core_wgpu_dev = validate_member_dependencies(
            "spirula-core",
            &[external_dep("wgpu", DepKind::Dev)],
            &workspace,
        );
        assert!(core_wgpu_dev.is_err(), "{core_wgpu_dev:?}");

        let ui_io = validate_member_dependencies(
            "spirula-ui",
            &[path_dep("spirula-io", DepKind::Normal)],
            &workspace,
        );
        assert!(ui_io.is_err(), "{ui_io:?}");

        let ui_train_dev = validate_member_dependencies(
            "spirula-ui",
            &[path_dep("spirula-train", DepKind::Dev)],
            &workspace,
        );
        assert!(ui_train_dev.is_err(), "{ui_train_dev:?}");

        let arbitrary_path = validate_member_dependencies(
            "spirula-io",
            &[path_dep("local-helper", DepKind::Normal)],
            &workspace,
        );
        assert!(arbitrary_path.is_err(), "{arbitrary_path:?}");

        let aliased_gpu = validate_member_dependencies(
            "spirula-cli",
            &[path_dep_renamed(
                "spirula-gpu",
                "harmless_alias",
                DepKind::Normal,
            )],
            &workspace,
        );
        assert!(aliased_gpu.is_err(), "{aliased_gpu:?}");

        let missing_member = validate_member_dependencies(
            "spirula-ui",
            &[path_dep("spirula-application", DepKind::Normal)],
            &members(&["spirula-ui", "spirula-core"]),
        );
        assert!(missing_member.is_err(), "{missing_member:?}");
    }

    #[test]
    fn collect_deps_keeps_non_spirula_path_entries() {
        let pkg = serde_json::json!({
            "dependencies": [
                {
                    "name": "local-helper",
                    "source": null,
                    "kind": null,
                    "rename": null
                },
                {
                    "name": "spirula-gpu",
                    "source": null,
                    "kind": "dev",
                    "rename": "gfx"
                },
                {
                    "name": "serde",
                    "source": "registry+https://github.com/rust-lang/crates.io-index",
                    "kind": null,
                    "rename": null
                }
            ]
        });
        let deps = collect_deps(&pkg);
        assert_eq!(deps.len(), 3);
        assert!(
            deps.iter()
                .any(|dep| dep.name == "local-helper" && dep.is_path)
        );
        assert!(deps.iter().any(|dep| {
            dep.name == "spirula-gpu"
                && dep.rename.as_deref() == Some("gfx")
                && dep.kind == DepKind::Dev
                && dep.is_path
        }));
        assert!(deps.iter().any(|dep| dep.name == "serde" && !dep.is_path));
    }

    #[test]
    fn feature_combinations_stay_declared_only() {
        assert_eq!(feature_combinations(0), vec!["default".to_owned()]);
        assert_eq!(
            feature_combinations(2),
            vec!["default".to_owned(), "all-features".to_owned()]
        );
    }

    #[test]
    fn gate_requires_check_and_test_for_all_features() {
        assert!(check_gate_covers_features(&["default".to_owned()]).is_ok());
        let err = check_gate_covers_features(&["default".to_owned(), "all-features".to_owned()]);
        assert!(err.is_err(), "{err:?}");
        let message = err.unwrap_err();
        assert!(
            message.contains("check") || message.contains("test"),
            "{message}"
        );
    }

    #[test]
    fn parse_rustc_host_accepts_windows_and_unix_hosts() {
        assert_eq!(
            parse_rustc_host("host: x86_64-pc-windows-msvc\n").as_deref(),
            Some("x86_64-pc-windows-msvc")
        );
        assert_eq!(
            parse_rustc_host("host: aarch64-apple-darwin\n").as_deref(),
            Some("aarch64-apple-darwin")
        );
    }

    #[test]
    fn parse_build_target_from_config_reads_build_table() {
        assert_eq!(
            parse_build_target_from_config(
                "[build]\n# comment\ntarget = \"wasm32-unknown-unknown\"\njobs = 4\n"
            )
            .as_deref(),
            Some("wasm32-unknown-unknown")
        );
        assert_eq!(
            parse_build_target_from_config("[alias]\nfoo = \"bar\"\n"),
            None
        );
    }

    #[test]
    fn host_only_gate_allows_matching_windows_host_override() {
        assert!(ensure_host_only_gate("x86_64-pc-windows-msvc", None).is_ok());
        assert!(
            ensure_host_only_gate("x86_64-pc-windows-msvc", Some("x86_64-pc-windows-msvc")).is_ok()
        );
        let err = ensure_host_only_gate("x86_64-pc-windows-msvc", Some("x86_64-unknown-linux-gnu"));
        assert!(err.is_err(), "{err:?}");
        assert!(err.unwrap_err().contains("host-only"));
    }

    #[test]
    fn discover_prefers_env_over_project_config_without_set_var() {
        let root = spirula_io::TempHarness::new().expect("harness");
        let cargo_dir = root.path().join(".cargo");
        fs::create_dir(&cargo_dir).unwrap();
        fs::write(
            cargo_dir.join("config.toml"),
            "[build]\ntarget = \"wasm32-unknown-unknown\"\n",
        )
        .unwrap();

        let from_config = discover_cargo_build_target(
            root.path(),
            &TargetLookup {
                cargo_build_target: None,
                cargo_home: None,
            },
        )
        .unwrap();
        assert_eq!(from_config.as_deref(), Some("wasm32-unknown-unknown"));

        let from_env = discover_cargo_build_target(
            root.path(),
            &TargetLookup {
                cargo_build_target: Some("x86_64-unknown-linux-gnu".to_owned()),
                cargo_home: None,
            },
        )
        .unwrap();
        assert_eq!(from_env.as_deref(), Some("x86_64-unknown-linux-gnu"));
    }

    #[test]
    fn discover_reads_cargo_home_config_when_project_has_none() {
        let project = spirula_io::TempHarness::new().expect("project");
        let home = spirula_io::TempHarness::new().expect("cargo home");
        fs::write(
            home.path().join("config.toml"),
            "[build]\ntarget = \"x86_64-pc-windows-gnu\"\n",
        )
        .unwrap();

        let found = discover_cargo_build_target(
            project.path(),
            &TargetLookup {
                cargo_build_target: None,
                cargo_home: Some(home.path().to_path_buf()),
            },
        )
        .unwrap();
        assert_eq!(found.as_deref(), Some("x86_64-pc-windows-gnu"));
    }
}
