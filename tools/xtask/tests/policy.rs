use spirula_xtask::{
    check_workspace_policy, ensure_compile_steps_pin_host, rustc_host_triple, workspace_gate_steps,
    workspace_root,
};

#[test]
fn workspace_policy_matches_live_manifests() {
    let report = check_workspace_policy(&workspace_root()).expect("policy check");
    assert_eq!(report.members, 8, "skeleton workspace member count");
    assert_eq!(
        report.path_edges, 3,
        "expected io→core (normal+dev) and xtask→io in the live tree"
    );
    assert_eq!(
        report.optional_features, 0,
        "no optional features are declared yet"
    );
    assert_eq!(report.feature_combinations, vec!["default".to_owned()]);

    let rustc_host = rustc_host_triple().expect("rustc host");
    assert_eq!(
        report.rustc_host, rustc_host,
        "policy must report the actual rustc host (including Windows hosts)"
    );
    assert_eq!(report.targets_checked, vec![rustc_host.clone()]);

    let steps = workspace_gate_steps(&rustc_host);
    ensure_compile_steps_pin_host(&steps, &rustc_host).expect("gate must pin rustc host");
}

#[test]
fn cargo_compile_steps_pin_rustc_host_explicitly() {
    let host = rustc_host_triple().expect("rustc host");
    let steps = workspace_gate_steps(&host);
    assert_eq!(steps.len(), 4);
    ensure_compile_steps_pin_host(&steps, &host).unwrap();

    let fmt = steps.iter().find(|step| step.name == "fmt").unwrap();
    assert!(
        fmt.target_flag_value().is_none(),
        "fmt does not need --target"
    );
    for name in ["check", "test", "clippy"] {
        let step = steps.iter().find(|step| step.name == name).unwrap();
        assert_eq!(
            step.target_flag_value(),
            Some(host.as_str()),
            "{name} must pin --target <rustc host> so Cargo config/env cannot cross-compile silently"
        );
    }
}
