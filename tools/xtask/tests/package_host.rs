//! Host evidence for P2-F07: assemble from built bins + isolated run.
//!
//! Builds package binaries in the current worktree (may be dirty), assembles
//! via [`assemble_package_from_bin_dir`], then runs the isolated gate. The
//! clean-tree / `--from-ref` path is exercised by the `package` CLI after the
//! HOLD commit.

use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use spirula_xtask::{
    AssembleMeta, assemble_package_from_bin_dir, check_package, run_package_isolated,
    workspace_root,
};

#[test]
fn host_assemble_check_and_isolated_run() {
    let root = workspace_root();
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let status = Command::new(&cargo)
        .args([
            "build",
            "--locked",
            "-p",
            "spirula-cli",
            "-p",
            "spirula-gpu",
        ])
        .current_dir(&root)
        .status()
        .expect("spawn cargo build");
    assert!(status.success(), "cargo build --locked package bins");

    let bin_dir = root.join("target/debug");
    let parent = std::env::temp_dir().join("spirula-p2-f07-host");
    std::fs::create_dir_all(&parent).unwrap();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let out = parent.join(format!("pkg with spaces {nanos}"));
    let outcome = assemble_package_from_bin_dir(
        &bin_dir,
        &out,
        &AssembleMeta {
            revision: "host-test".into(),
            version: "0.1.0".into(),
            profile: "dev".into(),
        },
    )
    .expect("assemble");
    check_package(&outcome.package_dir).expect("check");

    // Tamper then restore: prove check catches it.
    let smoke_bin = outcome.package_dir.join("bin/spirula-gpu-smoke");
    let original = std::fs::read(&smoke_bin).unwrap();
    std::fs::write(&smoke_bin, b"tampered-host").unwrap();
    assert!(
        check_package(&outcome.package_dir)
            .expect_err("tamper")
            .contains("sha256 mismatch")
    );
    std::fs::write(&smoke_bin, &original).unwrap();
    check_package(&outcome.package_dir).expect("restored");

    let isolate_parent = parent.join(format!("isolate {nanos}"));
    let report = run_package_isolated(&outcome.package_dir, &isolate_parent).expect("isolated");
    assert_eq!(report.help_exit, 0);
    assert_eq!(report.capabilities_exit, 0);
    assert_eq!(report.probe_exit, 0);
    assert_eq!(report.smoke_exit, 0);
    assert!(
        report.smoke_status == "pass" || report.smoke_status == "skip",
        "status={}",
        report.smoke_status
    );
    if report.smoke_status == "pass" {
        assert!(report.smoke_counted_as_pass);
        assert_eq!(report.smoke_match.as_deref(), Some("cpu_exact"));
        assert_eq!(report.smoke_current, Some(0));
    } else {
        assert!(!report.smoke_counted_as_pass, "skip must not count as pass");
    }

    // Destination-exists retry path.
    let err = assemble_package_from_bin_dir(
        &bin_dir,
        &out,
        &AssembleMeta {
            revision: "host-test".into(),
            version: "0.1.0".into(),
            profile: "dev".into(),
        },
    )
    .expect_err("exists");
    assert!(err.contains("already exists"), "{err}");

    let _ = std::fs::remove_dir_all(&parent);
    let _ = PathBuf::from(&out);
}
