use spirula_xtask::{check_workspace_policy, workspace_gate_steps, workspace_root};

#[test]
fn workspace_policy_matches_live_manifests() {
    let report = check_workspace_policy(&workspace_root()).expect("policy check");
    assert_eq!(report.members, 8, "skeleton workspace member count");
    assert_eq!(
        report.path_edges, 2,
        "expected spirula-io→core and spirula-xtask→io only"
    );
    assert_eq!(
        report.optional_features, 0,
        "no optional features are declared yet"
    );
    assert_eq!(report.feature_combinations, vec!["default".to_owned()]);
    assert_eq!(report.host_targets_checked.len(), 1);
    assert!(
        !report.host_targets_checked[0].is_empty(),
        "host triple must come from rustc"
    );
    assert!(
        !report
            .host_targets_checked
            .iter()
            .any(|t| t.contains("windows")),
        "policy must not invent a Windows target that was not run"
    );
}

#[test]
fn verify_gate_stays_host_only_four_cargo_steps() {
    let steps = workspace_gate_steps();
    assert_eq!(steps.len(), 4);
    for step in steps {
        assert_eq!(step.program, "cargo");
        assert!(
            !step.args.contains(&"--target"),
            "{} must remain host-only",
            step.name
        );
    }
}
