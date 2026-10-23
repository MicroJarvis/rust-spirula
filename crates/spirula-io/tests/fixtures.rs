//! Integration tests for the Phase 0 fixture set.

use spirula_io::fixtures::{check_fixtures, default_fixture_root};
use std::fs;
use std::path::{Path, PathBuf};

fn copy_tree(source: &Path, target: &Path) {
    fs::create_dir_all(target).expect("create fixture copy");
    for entry in fs::read_dir(source).expect("read fixture directory") {
        let entry = entry.expect("fixture entry");
        let file_type = entry.file_type().expect("fixture entry type");
        let destination = target.join(entry.file_name());
        if file_type.is_dir() {
            copy_tree(&entry.path(), &destination);
        } else {
            fs::copy(entry.path(), &destination).expect("copy fixture file");
        }
    }
}

fn scratch_dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("spirula-fixtures-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    path
}

#[test]
fn fixtures_pass_all_checks() {
    let root = default_fixture_root();
    let report = check_fixtures(&root).expect("fixture check");
    assert!(
        report.checked_files >= 21,
        "expected the manifest to cover the full fixture set, got {}",
        report.checked_files
    );
    assert_eq!(
        report.checks,
        vec![
            "camera: intrinsics, distortion and projection samples",
            "dataset: COLMAP text rows, golden values and tiny PNG",
            "scene: similarity blocks, inverse, centering, georeference and samples",
            "splats: PLY headers, raw values, bounds and checkpoint copy",
            "checkpoint: ustar entries, state.json, NPY payloads and run config",
            "render: golden PPM/PNG, projected center and analytic alpha",
        ]
    );
}

#[test]
fn corrupted_fixture_is_rejected() {
    let source = default_fixture_root();
    let scratch = scratch_dir("corrupt");
    copy_tree(&source, &scratch);

    let target = scratch.join("splats/render_splat.ply");
    let mut bytes = fs::read(&target).expect("read render splat");
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    fs::write(&target, bytes).expect("corrupt render splat");

    let error = check_fixtures(&scratch).expect_err("corruption must fail");
    assert!(
        error.to_string().contains("checksum mismatch"),
        "unexpected error: {error}"
    );

    fs::remove_dir_all(&scratch).ok();
}

#[test]
fn missing_golden_file_is_rejected() {
    let source = default_fixture_root();
    let scratch = scratch_dir("missing");
    copy_tree(&source, &scratch);
    fs::remove_file(scratch.join("golden/camera.json")).expect("remove golden");

    let error = check_fixtures(&scratch).expect_err("missing golden must fail");
    assert!(
        error.to_string().contains("golden/camera.json"),
        "unexpected error: {error}"
    );

    fs::remove_dir_all(&scratch).ok();
}
