use spirula_xtask::{
    check_workspace_policy, ensure_host_only_gate, workspace_gate_steps, workspace_root,
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

    let rustc_host = rustc_host_from_process();
    assert_eq!(
        report.rustc_host, rustc_host,
        "policy must report the actual rustc host (including Windows hosts)"
    );
    assert_eq!(
        report.targets_checked,
        vec![rustc_host.clone()],
        "host-only gate checks the rustc host triple, not an invented OS filter"
    );
    // Matching override is fine; a differing override would have failed policy.
    if let Some(cargo_target) = &report.cargo_build_target {
        assert_eq!(cargo_target, &rustc_host);
    }
    ensure_host_only_gate(&report.rustc_host, report.cargo_build_target.as_deref())
        .expect("live tree must satisfy the host-only gate");
}

#[test]
fn verify_gate_stays_host_only_four_cargo_steps() {
    let steps = workspace_gate_steps();
    assert_eq!(steps.len(), 4);
    for step in steps {
        assert_eq!(step.program, "cargo");
        assert!(
            !step.args.contains(&"--target"),
            "{} must remain host-only on the CLI; Cargo build.target overrides are policed separately",
            step.name
        );
    }
}

fn rustc_host_from_process() -> String {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_owned());
    let output = std::process::Command::new(rustc)
        .args(["-vV"])
        .output()
        .expect("rustc -vV");
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .find_map(|line| line.strip_prefix("host: ").map(str::trim))
        .filter(|host| !host.is_empty())
        .expect("rustc host")
        .to_owned()
}
