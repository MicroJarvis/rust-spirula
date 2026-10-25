//! Development verification for the Spirula Studio Rust workspace.
//!
//! This crate is tooling, not a runtime library: `cargo run -p spirula-xtask --
//! verify` checks the compatibility fixtures, the document/status consistency,
//! the workspace policy (lints, architecture edges, features, CLI shape), the
//! optional C++ reference baseline, and the fixed workspace gate.

pub mod gate;
pub mod policy;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

pub use gate::{GateStep, diff_gate_step, workspace_gate_steps};
pub use policy::{
    PolicyReport, TargetLookup, check_workspace_policy, discover_cargo_build_target,
    ensure_host_only_gate, parse_build_target_from_config,
};

/// Summary of the document/status check.
#[derive(Debug, Clone)]
pub struct DocsReport {
    /// Number of feature rows parsed from the project plan.
    pub features: usize,
    /// Number of relative Markdown links that resolved.
    pub links: usize,
    /// Number of Markdown files scanned.
    pub markdown_files: usize,
    /// Number of stable error codes in the runtime contract catalog.
    pub error_codes: usize,
}

/// Workspace root, resolved from this crate's manifest.
pub fn workspace_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .join("../..")
        .canonicalize()
        .unwrap_or_else(|_| manifest_dir.join("../.."))
}

/// Run the fixture checker and return a one-line summary.
pub fn fixture_check() -> Result<String, String> {
    let root = spirula_io::fixtures::default_fixture_root();
    let report = spirula_io::fixtures::check_fixtures(&root).map_err(|e| e.to_string())?;
    Ok(format!(
        "{} files verified; {} format checks",
        report.checked_files,
        report.checks.len()
    ))
}

/// Check project-plan statuses, artifacts, feature references, links, and the
/// error-code catalog.
pub fn check_docs(root: &Path) -> Result<DocsReport, String> {
    let files = markdown_files(root)?;
    let mut links = 0;
    let mut all_text = String::new();
    for file in &files {
        let text =
            fs::read_to_string(file).map_err(|e| format!("reading {}: {e}", file.display()))?;
        links += check_links(file, &text)?;
        all_text.push_str(&text);
        all_text.push('\n');
    }

    let plan_path = root.join("docs/project-plan.md");
    let plan = fs::read_to_string(&plan_path)
        .map_err(|e| format!("reading {}: {e}", plan_path.display()))?;
    let features = parse_features(&plan)?;

    // Every feature token referenced anywhere must exist in the plan.
    let ids: BTreeSet<&str> = features.iter().map(|(id, _)| id.as_str()).collect();
    for token in feature_tokens(&all_text) {
        if !ids.contains(token.as_str()) {
            return Err(format!(
                "document references unknown feature id `{token}` (not in docs/project-plan.md)"
            ));
        }
    }

    // Completed features with mapped records must keep those artifacts.
    let mut completed = 0;
    for (id, status) in &features {
        if status == "已完成" {
            completed += 1;
            for required in completed_feature_artifacts(id) {
                let path = root.join(required);
                if !path.is_file() {
                    return Err(format!(
                        "{id} is marked 已完成 but its record `{required}` is missing"
                    ));
                }
            }
        }
    }
    if completed == 0 {
        return Err("no feature is marked 已完成".to_owned());
    }

    let error_codes = check_error_catalog(root)?;

    Ok(DocsReport {
        features: features.len(),
        links,
        markdown_files: files.len(),
        error_codes,
    })
}

fn parse_features(plan: &str) -> Result<Vec<(String, String)>, String> {
    const STATUSES: [&str; 5] = ["当前", "未开始", "进行中", "已完成", "阻塞"];
    let mut features: BTreeMap<String, String> = BTreeMap::new();
    let mut current = Vec::new();
    for (line_no, line) in plan.lines().enumerate() {
        if !line.starts_with("| P") {
            continue;
        }
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        if cells.len() < 4 {
            continue;
        }
        let id = cells[1];
        if !looks_like_feature_id(id) {
            continue;
        }
        let status = cells[cells.len() - 2];
        if !STATUSES.contains(&status) {
            return Err(format!(
                "docs/project-plan.md:{}: feature {id} has invalid status `{status}`",
                line_no + 1
            ));
        }
        if features.insert(id.to_owned(), status.to_owned()).is_some() {
            return Err(format!(
                "docs/project-plan.md:{}: duplicate feature id {id}",
                line_no + 1
            ));
        }
        if status == "当前" {
            current.push(id.to_owned());
        }
    }
    if features.is_empty() {
        return Err("docs/project-plan.md has no feature rows".to_owned());
    }
    if current.len() != 1 {
        return Err(format!(
            "docs/project-plan.md must have exactly one `当前` feature, found {}: {current:?}",
            current.len()
        ));
    }
    Ok(features.into_iter().collect())
}

fn completed_feature_artifacts(id: &str) -> &'static [&'static str] {
    match id {
        "P0-F01" => &["docs/phase-0/reference-baseline.md"],
        "P0-F02" => &["docs/phase-0/subsystem-inventory.md"],
        "P0-F03" => &["docs/phase-0/capability-matrix.md"],
        "P0-F04" => &["docs/phase-0/fixtures.md"],
        "P0-F05" => &["docs/phase-0/runtime-contracts.md"],
        "P0-F06" => &["docs/phase-0/exit-report.md"],
        "P1-F02" => &["docs/phase-1/config.md"],
        "P1-F03" => &["docs/phase-1/cancellation.md"],
        "P1-F04" => &["docs/phase-1/geometry.md"],
        "P1-F05" => &["docs/phase-1/atomic.md"],
        "P1-F06" => &[
            "docs/phase-1/workspace-checks.md",
            "docs/phase-1/exit-report.md",
        ],
        _ => &[],
    }
}

fn looks_like_feature_id(text: &str) -> bool {
    let Some(rest) = text.strip_prefix('P') else {
        return false;
    };
    let Some((phase, rest)) = rest.split_once("-F") else {
        return false;
    };
    !phase.is_empty()
        && phase.bytes().all(|b| b.is_ascii_digit())
        && !rest.is_empty()
        && rest.bytes().all(|b| b.is_ascii_digit())
}

fn feature_tokens(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'P' {
            index += 1;
            continue;
        }
        let start = index;
        index += 1;
        let phase_start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        if index == phase_start || bytes.get(index..index + 2) != Some(b"-F") {
            continue;
        }
        index += 2;
        let number_start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        if index > number_start {
            tokens.push(text[start..index].to_owned());
        }
    }
    tokens
}

fn markdown_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    for name in ["AGENTS.md", "README.md"] {
        let path = root.join(name);
        if path.is_file() {
            files.push(path);
        }
    }
    collect_markdown(&root.join("docs"), &mut files)?;
    files.sort();
    files.dedup();
    Ok(files)
}

fn collect_markdown(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir).map_err(|e| format!("reading {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("reading {}: {e}", dir.display()))?;
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            if name == "target" || name == ".worktrees" || name == ".git" {
                continue;
            }
            collect_markdown(&path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "md") {
            out.push(path);
        }
    }
    Ok(())
}

fn check_links(file: &Path, text: &str) -> Result<usize, String> {
    let mut count = 0;
    let mut in_fence = false;
    for (line_no, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let mut rest = line;
        while let Some(open) = rest.find("](") {
            let after = &rest[open + 2..];
            let Some(close) = after.find(')') else {
                return Err(format!(
                    "{}:{}: unterminated Markdown link",
                    file.display(),
                    line_no + 1
                ));
            };
            let target = after[..close].trim();
            rest = &after[close + 1..];
            if target.is_empty()
                || target.starts_with('#')
                || target.starts_with("http://")
                || target.starts_with("https://")
                || target.starts_with("mailto:")
            {
                continue;
            }
            let target = target.split('#').next().unwrap_or(target);
            let target = target.trim_matches(|c| c == '<' || c == '>');
            if target.is_empty() {
                continue;
            }
            let resolved = file.parent().unwrap_or(file).join(target);
            if !resolved.exists() {
                return Err(format!(
                    "{}:{}: broken link `{target}`",
                    file.display(),
                    line_no + 1
                ));
            }
            count += 1;
        }
    }
    Ok(count)
}

fn check_error_catalog(root: &Path) -> Result<usize, String> {
    let path = root.join("docs/phase-0/runtime-contracts.md");
    let text = fs::read_to_string(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let mut defined = BTreeSet::new();
    for line in text.lines() {
        let line = line.trim_start();
        let Some(rest) = line.strip_prefix("| `") else {
            continue;
        };
        let Some(end) = rest.find('`') else {
            continue;
        };
        let code = &rest[..end];
        if code.starts_with("SP-") {
            defined.insert(code.to_owned());
        }
    }
    if defined.is_empty() {
        return Err("runtime-contracts.md has no error-code catalog".to_owned());
    }

    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let Some(found) = text[index..].find("SP-") else {
            break;
        };
        let start = index + found;
        let mut end = start;
        while end < bytes.len()
            && (bytes[end].is_ascii_uppercase()
                || bytes[end].is_ascii_digit()
                || bytes[end] == b'-')
        {
            end += 1;
        }
        let token = &text[start..end];
        if token != "SP-" && !defined.contains(token) {
            return Err(format!(
                "runtime-contracts.md references undefined error code `{token}`"
            ));
        }
        index = end.max(start + 1);
    }
    Ok(defined.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_ids_are_recognized() {
        assert!(looks_like_feature_id("P0-F01"));
        assert!(looks_like_feature_id("P12-F003"));
        assert!(!looks_like_feature_id("P0F01"));
        assert!(!looks_like_feature_id("PX-F01"));
        assert!(!looks_like_feature_id("P0-F"));
    }

    #[test]
    fn feature_tokens_do_not_eat_following_text() {
        assert_eq!(
            feature_tokens("see P3-F03 and P4-F06."),
            vec!["P3-F03".to_owned(), "P4-F06".to_owned()]
        );
    }
}
