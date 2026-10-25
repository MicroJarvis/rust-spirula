use std::process::Command;

use spirula_xtask::{
    check_workspace_policy, ensure_compile_steps_pin_host, rustc_host_triple, workspace_gate_steps,
    workspace_root,
};

#[test]
fn workspace_policy_matches_live_manifests() {
    let root = workspace_root();
    let report = check_workspace_policy(&root).expect("policy check");
    let expected = independent_metadata_counts(&root);

    assert_eq!(report.members, expected.members);
    assert_eq!(report.path_edges, expected.path_edges);
    assert_eq!(report.optional_features, expected.optional_features);
    assert!(report.members > 0, "workspace must list members");
    if expected.optional_features == 0 {
        assert_eq!(report.feature_combinations, vec!["default".to_owned()]);
    } else {
        assert!(
            report
                .feature_combinations
                .iter()
                .any(|name| name == "all-features")
        );
    }

    let rustc_host = rustc_host_triple().expect("rustc host");
    assert_eq!(report.rustc_host, rustc_host);
    assert_eq!(report.targets_checked, vec![rustc_host.clone()]);
    ensure_compile_steps_pin_host(&workspace_gate_steps(&rustc_host), &rustc_host)
        .expect("gate must pin rustc host");
}

#[test]
fn cargo_compile_steps_pin_rustc_host_explicitly() {
    let host = rustc_host_triple().expect("rustc host");
    let steps = workspace_gate_steps(&host);
    assert_eq!(steps.len(), 4);
    ensure_compile_steps_pin_host(&steps, &host).unwrap();

    let fmt = steps.iter().find(|step| step.name == "fmt").unwrap();
    assert!(fmt.target_flag_value().is_none());
    for name in ["check", "test", "clippy"] {
        let step = steps.iter().find(|step| step.name == name).unwrap();
        assert_eq!(step.target_flag_value(), Some(host.as_str()));
    }
}

struct MetadataCounts {
    members: usize,
    path_edges: usize,
    optional_features: usize,
}

fn independent_metadata_counts(root: &std::path::Path) -> MetadataCounts {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(root)
        .output()
        .expect("cargo metadata");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("metadata json");
    let member_ids: std::collections::BTreeSet<&str> = value["workspace_members"]
        .as_array()
        .expect("members")
        .iter()
        .filter_map(|id| id.as_str())
        .collect();
    let mut members = 0;
    let mut path_edges = 0;
    let mut optional_features = 0;
    for package in value["packages"].as_array().expect("packages") {
        let id = package["id"].as_str().expect("id");
        if !member_ids.contains(id) {
            continue;
        }
        members += 1;
        if let Some(deps) = package["dependencies"].as_array() {
            for dep in deps {
                let is_path = dep.get("path").and_then(|path| path.as_str()).is_some()
                    || dep.get("source").is_some_and(serde_json::Value::is_null);
                if is_path {
                    path_edges += 1;
                }
            }
        }
        if let Some(features) = package["features"].as_object() {
            optional_features += features
                .keys()
                .filter(|name| name.as_str() != "default")
                .count();
        }
    }
    MetadataCounts {
        members,
        path_edges,
        optional_features,
    }
}
