//! Host evidence for P2-F07: assemble from built bins + isolated run +
//! deterministic no-clobber publish schedules.
//!
//! Builds package binaries in the current worktree (may be dirty), assembles
//! via [`assemble_package_from_bin_dir`], then runs the isolated gate. The
//! clean-tree / `--from-ref` path is exercised by the `package` CLI after a
//! clean candidate commit.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use spirula_xtask::{
    AssembleMeta, PACKAGE_BINARIES, assemble_package_from_bin_dir,
    assemble_package_from_bin_dir_with_before_publish, check_package, run_package_isolated,
    workspace_root,
};

fn ensure_package_bins() -> PathBuf {
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
    for name in PACKAGE_BINARIES {
        assert!(
            bin_dir.join(name).is_file(),
            "missing built binary {}",
            bin_dir.join(name).display()
        );
    }
    bin_dir
}

fn unique_parent(label: &str) -> PathBuf {
    let parent = std::env::temp_dir().join("spirula-p2-f07-host");
    std::fs::create_dir_all(&parent).unwrap();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = parent.join(format!("{label}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn no_staging_residue(parent: &Path) {
    for entry in std::fs::read_dir(parent).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        assert!(
            !name.starts_with(".spirula-pkg-tmp."),
            "staging residue left: {name}"
        );
    }
}

#[test]
fn host_assemble_check_and_isolated_run() {
    let bin_dir = ensure_package_bins();
    let parent = unique_parent("pkg");
    let out = parent.join("pkg with spaces");
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

    let isolate_parent = parent.join("isolate with spaces");
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
}

#[test]
fn publish_preserves_destination_created_during_assemble() {
    // Deterministic schedule of REVIEW atomic-race: after staging is ready,
    // create a destination with a sentinel; publish must refuse and preserve it.
    let bin_dir = ensure_package_bins();
    let parent = unique_parent("race-det");
    let out = parent.join("out");
    let err = assemble_package_from_bin_dir_with_before_publish(
        &bin_dir,
        &out,
        &AssembleMeta {
            revision: "race-probe".into(),
            version: "0.1.0".into(),
            profile: "dev".into(),
        },
        |dest| {
            std::fs::create_dir(dest).unwrap();
            std::fs::write(dest.join("sentinel"), b"keep").unwrap();
            Ok(())
        },
    )
    .expect_err("race");
    assert!(err.contains("already exists"), "{err}");
    assert_eq!(
        std::fs::read(out.join("sentinel")).unwrap(),
        b"keep",
        "existing destination must be preserved"
    );
    assert!(
        !out.join("MANIFEST.json").exists(),
        "must not publish over the concurrent destination"
    );
    no_staging_residue(&parent);
    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn concurrent_publish_single_winner_deterministic() {
    let bin_dir = ensure_package_bins();
    let parent = unique_parent("race-conc");
    let out = parent.join("out");
    let barrier = Arc::new(Barrier::new(2));
    let meta = AssembleMeta {
        revision: "conc".into(),
        version: "0.1.0".into(),
        profile: "dev".into(),
    };

    let spawn = |barrier: Arc<Barrier>, bins: PathBuf, out: PathBuf, meta: AssembleMeta| {
        thread::spawn(move || {
            assemble_package_from_bin_dir_with_before_publish(&bins, &out, &meta, |_| {
                barrier.wait();
                Ok(())
            })
        })
    };

    let t1 = spawn(barrier.clone(), bin_dir.clone(), out.clone(), meta.clone());
    let t2 = spawn(barrier, bin_dir, out.clone(), meta);
    let r1 = t1.join().unwrap();
    let r2 = t2.join().unwrap();
    let wins = [&r1, &r2].iter().filter(|r| r.is_ok()).count();
    let losses = [&r1, &r2].iter().filter(|r| r.is_err()).count();
    assert_eq!(wins, 1, "r1={r1:?} r2={r2:?}");
    assert_eq!(losses, 1, "r1={r1:?} r2={r2:?}");
    assert!(out.join("MANIFEST.json").is_file());
    check_package(&out).expect("winner package checks");
    no_staging_residue(&parent);
    let loser = match (&r1, &r2) {
        (Err(err), Ok(_)) | (Ok(_), Err(err)) => err.clone(),
        _ => panic!("expected exactly one failure, r1={r1:?} r2={r2:?}"),
    };
    assert!(loser.contains("already exists"), "{loser}");
    let _ = std::fs::remove_dir_all(&parent);
}
