//! P3-F06 Gaussian splat PLY compatibility and atomic-write tests.

use std::path::PathBuf;

use spirula_core::{CancelReason, CancelToken, ErrorCode, GaussianSplat, Quat, SplatSet, Vec3};
use spirula_io::{
    TempHarness, read_ply_splats, read_ply_splats_budgeted, write_ply_splats,
    write_ply_splats_budgeted,
};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture(name: &str) -> PathBuf {
    root().join("fixtures/splats").join(name)
}

#[test]
fn tiny_fixture_decodes_raw_values_and_bounds() {
    let set = read_ply_splats(&fixture("tiny_splat.ply"), None).unwrap();
    assert_eq!(set.len(), 3);
    let (min, max) = set.bounds();
    assert_eq!(min.to_array(), [-0.5, -0.5, 2.0]);
    assert_eq!(max.to_array(), [0.5, 0.5, 4.0]);
    let first = set.splats()[0];
    assert_eq!(first.mean().to_array(), [0.0, 0.0, 2.0]);
    assert!((first.opacity() - 0.8).abs() < 1e-6);
    let color = first.base_color().to_array();
    assert!((color[0] - 1.0).abs() < 1e-6);
    assert!(color[1].abs() < 1e-6);
    assert!(color[2].abs() < 1e-6);
    assert_eq!(first.rotation().wxyz(), [1.0, 0.0, 0.0, 0.0]);
}

#[test]
fn render_fixture_decodes_and_round_trips_as_binary_le() {
    let set = read_ply_splats(&fixture("render_splat.ply"), None).unwrap();
    assert_eq!(set.len(), 1);
    let h = TempHarness::new().unwrap();
    let out = h.path().join("roundtrip.ply");
    write_ply_splats(&out, &set, None).unwrap();
    let bytes = std::fs::read(&out).unwrap();
    assert!(bytes.starts_with(b"ply\nformat binary_little_endian 1.0\n"));
    let back = read_ply_splats(&out, None).unwrap();
    assert_eq!(back, set);
}

#[test]
fn ascii_splat_read_is_compatible_with_binary_reader() {
    let ascii = br#"ply
format ascii 1.0
element vertex 1
property float x
property float y
property float z
property float nx
property float ny
property float nz
property float f_dc_0
property float f_dc_1
property float f_dc_2
property float opacity
property float scale_0
property float scale_1
property float scale_2
property float rot_0
property float rot_1
property float rot_2
property float rot_3
end_header
0 0 -2 0 0 0 1.77245385 -1.77245385 -1.77245385 4.59511985 -1.6094379 -1.6094379 -1.6094379 1 0 0 0
"#;
    let h = TempHarness::new().unwrap();
    let path = h.path().join("ascii.ply");
    std::fs::write(&path, ascii).unwrap();
    let set = read_ply_splats(&path, None).unwrap();
    assert_eq!(set.len(), 1);
    assert!((set.splats()[0].opacity() - 0.99).abs() < 1e-5);
}

#[test]
fn malformed_property_order_and_empty_input_are_typed_errors() {
    let h = TempHarness::new().unwrap();
    let empty = h.path().join("empty.ply");
    std::fs::write(
        &empty,
        b"ply\nformat ascii 1.0\nelement vertex 0\nend_header\n",
    )
    .unwrap();
    assert_eq!(
        read_ply_splats(&empty, None).unwrap_err().code(),
        ErrorCode::MalformedRecord
    );

    let bad = h.path().join("bad.ply");
    let mut text = String::from("ply\nformat ascii 1.0\nelement vertex 1\n");
    text.push_str("property float y\nproperty float x\n");
    for name in [
        "z", "nx", "ny", "nz", "f_dc_0", "f_dc_1", "f_dc_2", "opacity", "scale_0", "scale_1",
        "scale_2", "rot_0", "rot_1", "rot_2", "rot_3",
    ] {
        text.push_str(&format!("property float {name}\n"));
    }
    text.push_str("end_header\n0 0 0 0 0 0 0 0 0 0 0 0 0 1 0 0 0\n");
    std::fs::write(&bad, text).unwrap();
    assert_eq!(
        read_ply_splats(&bad, None).unwrap_err().code(),
        ErrorCode::MalformedRecord
    );

    let mut truncated = std::fs::read(fixture("render_splat.ply")).unwrap();
    truncated.pop();
    let trunc_path = h.path().join("truncated.ply");
    std::fs::write(&trunc_path, truncated).unwrap();
    assert_eq!(
        read_ply_splats(&trunc_path, None).unwrap_err().code(),
        ErrorCode::MalformedRecord
    );

    let extra = h.path().join("extra.ply");
    let mut ascii_extra = String::from(
        "ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\nproperty float y\nproperty float z\nproperty float nx\nproperty float ny\nproperty float nz\nproperty float f_dc_0\nproperty float f_dc_1\nproperty float f_dc_2\nproperty float opacity\nproperty float scale_0\nproperty float scale_1\nproperty float scale_2\nproperty float rot_0\nproperty float rot_1\nproperty float rot_2\nproperty float rot_3\nend_header\n",
    );
    ascii_extra.push_str("0 0 0 0 0 0 0 0 0 0 0 0 0 1 0 0 0 99\n");
    std::fs::write(&extra, ascii_extra).unwrap();
    assert_eq!(
        read_ply_splats(&extra, None).unwrap_err().code(),
        ErrorCode::MalformedRecord
    );
}

#[test]
fn budget_and_cancel_are_observed_before_publish() {
    let err = read_ply_splats_budgeted(&fixture("tiny_splat.ply"), None, 8).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ValueOutOfRange);

    let set = read_ply_splats(&fixture("tiny_splat.ply"), None).unwrap();
    let h = TempHarness::new().unwrap();
    let path = h.path().join("cancel.ply");
    write_ply_splats(&path, &set, None).unwrap();
    let before = std::fs::read(&path).unwrap();
    let cancel = CancelToken::new();
    cancel.request(CancelReason::User);
    assert_eq!(
        write_ply_splats(&path, &set, Some(&cancel))
            .unwrap_err()
            .code(),
        ErrorCode::Canceled
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn writer_rejects_nonrepresentable_f32_and_budget_before_touching_target() {
    let splat = GaussianSplat::try_new(
        Vec3::try_new(1.0e300, 0.0, 0.0).unwrap(),
        Quat::identity(),
        Vec3::try_new(0.0, 0.0, 0.0).unwrap(),
        0.0,
        Vec3::try_new(0.0, 0.0, 0.0).unwrap(),
    )
    .unwrap();
    let set = SplatSet::try_from_splats(vec![splat]).unwrap();
    let h = TempHarness::new().unwrap();
    let path = h.path().join("extreme.ply");
    let err = write_ply_splats(&path, &set, None).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
    assert!(!path.exists());

    let normal = read_ply_splats(&fixture("render_splat.ply"), None).unwrap();
    let budget_path = h.path().join("budget.ply");
    let err = write_ply_splats_budgeted(&budget_path, &normal, None, 64).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
    assert!(!budget_path.exists());
}
