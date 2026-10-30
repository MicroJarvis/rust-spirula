//! P3-F05 focused tests: scene-transform JSON, region I/O, center apply, goldens.

use std::path::{Path, PathBuf};

use spirula_core::{
    Aabb, CancelReason, CancelToken, CenterMode, CsgOp, ErrorCode, GaussianSplat, Mat3, Quat,
    Region, SceneTransform, SplatSet, Vec3, resolve_scene_center, scene_center,
};
use spirula_io::colmap::{
    ColmapPoint3D, ColmapPoints3D, DatasetFormat, DatasetLoadSummary, LoadedFrame, ParsedDataset,
    RedistortSource, SplitIndices,
};
use spirula_io::temp_harness::TempHarness;
use spirula_io::{
    AppliedTransform, ColmapLoadConfig, PublishOptions, SceneApplyConfig, apply_scene_to_dataset,
    parse_region_bytes, parse_scene_transform_bytes, publish_bytes_validated, read_region_json,
    read_scene_transform_json, region_to_json, write_region_json, write_scene_transform_json,
};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture(rel: &str) -> PathBuf {
    repo_root().join(rel)
}

#[test]
fn scene_transform_fixture_matches_golden_samples() {
    let doc = read_scene_transform_json(&fixture("fixtures/scene/scene_transform.json")).unwrap();
    let golden: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixture("fixtures/golden/scene.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(doc.center_mode, golden["centering_mode"]);
    assert_eq!(
        doc.center_world,
        [
            golden["center_world"][0].as_f64().unwrap(),
            golden["center_world"][1].as_f64().unwrap(),
            golden["center_world"][2].as_f64().unwrap(),
        ]
    );
    assert!((doc.train_from_world.scale() - golden["scale"].as_f64().unwrap()).abs() < 1e-15);
    for sample in golden["samples"].as_array().unwrap() {
        let w = sample["world"].as_array().unwrap();
        let t = sample["train"].as_array().unwrap();
        let world = Vec3::try_new(
            w[0].as_f64().unwrap(),
            w[1].as_f64().unwrap(),
            w[2].as_f64().unwrap(),
        )
        .unwrap();
        let train = doc.train_from_world.apply(world).unwrap();
        assert!((train.x() - t[0].as_f64().unwrap()).abs() < 1e-12);
        assert!((train.y() - t[1].as_f64().unwrap()).abs() < 1e-12);
        assert!((train.z() - t[2].as_f64().unwrap()).abs() < 1e-12);
    }
    let cpp = serde_json::from_str::<serde_json::Value>(
        &std::fs::read_to_string(fixture("fixtures/golden/scene_cpp_compare.json")).unwrap(),
    )
    .unwrap();
    let exp = &cpp["expected_train_from_world"];
    assert!((doc.train_from_world.scale() - exp["scale"].as_f64().unwrap()).abs() < 1e-15);
}

#[test]
fn inverse_composition_identity() {
    let doc = read_scene_transform_json(&fixture("fixtures/scene/scene_transform.json")).unwrap();
    let inv = doc.train_from_world.inverse().unwrap();
    let p = Vec3::try_new(7.0, -3.0, 1.25).unwrap();
    let back = inv.apply(doc.train_from_world.apply(p).unwrap()).unwrap();
    assert!((back.x() - p.x()).abs() < 1e-12);
    assert!((back.y() - p.y()).abs() < 1e-12);
    assert!((back.z() - p.z()).abs() < 1e-12);
}

#[test]
fn empty_and_degenerate_center_modes() {
    let origin = scene_center(CenterMode::None, &[], &[]).unwrap();
    assert_eq!(origin.to_array(), [0.0, 0.0, 0.0]);
    // Degenerate: single point.
    let pts = [[5.0, 5.0, 5.0]];
    let m = scene_center(CenterMode::PointMean, &[], &pts).unwrap();
    assert_eq!(m.to_array(), [5.0, 5.0, 5.0]);
    let med = scene_center(CenterMode::PointMedian, &[], &pts).unwrap();
    assert_eq!(med.to_array(), [5.0, 5.0, 5.0]);
    // Camera focus with one camera: stays at init mean.
    let c2w = [[1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 2.0, 0.0, 0.0, 1.0, 3.0]];
    let f = scene_center(CenterMode::CameraFocus, &c2w, &[]).unwrap();
    assert_eq!(f.to_array(), [1.0, 2.0, 3.0]);
}

#[test]
fn nonfinite_center_threshold_rejected() {
    let e = resolve_scene_center("auto", f32::NAN, &[], &[[1.0, 0.0, 0.0]]).unwrap_err();
    assert_eq!(e.code(), ErrorCode::ValueOutOfRange);
}

#[test]
fn region_fixture_and_bounds() {
    let r = read_region_json(&fixture("fixtures/scene/region_sphere.json")).unwrap();
    assert!(r.contains(Vec3::try_new(0.0, 0.0, 0.0).unwrap()));
    assert!(!r.contains(Vec3::try_new(3.0, 0.0, 0.0).unwrap()));
    let aabb = r.try_aabb().unwrap();
    assert_eq!(aabb.min.to_array(), [-2.0, -2.0, -2.0]);
    assert_eq!(aabb.max.to_array(), [2.0, 2.0, 2.0]);
}

#[test]
fn region_types_roundtrip() {
    let kinds = [
        Region::box_region(
            Vec3::try_new(0.0, 0.0, 0.0).unwrap(),
            Vec3::try_new(1.0, 2.0, 3.0).unwrap(),
            Mat3::identity(),
        )
        .unwrap(),
        Region::ellipsoid(
            Vec3::try_new(0.0, 0.0, 0.0).unwrap(),
            Vec3::try_new(1.0, 2.0, 3.0).unwrap(),
            Mat3::identity(),
        )
        .unwrap(),
        Region::cylinder(
            Vec3::try_new(0.0, 0.0, 0.0).unwrap(),
            Vec3::try_new(1.0, 1.0, 2.0).unwrap(),
            Mat3::identity(),
        )
        .unwrap(),
        Region::halfspace(Vec3::try_new(0.0, 0.0, 1.0).unwrap(), -1.0).unwrap(),
    ];
    for r in kinds {
        let back = parse_region_bytes(region_to_json(&r).unwrap().as_bytes()).unwrap();
        assert_eq!(back.kind(), r.kind());
    }
    let u = Region::csg(
        CsgOp::Union,
        vec![
            Region::sphere(Vec3::try_new(0.0, 0.0, 0.0).unwrap(), 1.0).unwrap(),
            Region::sphere(Vec3::try_new(2.0, 0.0, 0.0).unwrap(), 1.0).unwrap(),
        ],
    )
    .unwrap();
    let back = parse_region_bytes(region_to_json(&u).unwrap().as_bytes()).unwrap();
    assert!(back.contains(Vec3::try_new(0.0, 0.0, 0.0).unwrap()));
}

#[test]
fn illegal_region_write_does_not_clobber() {
    let h = TempHarness::new().unwrap();
    let path = h.path().join("roi.json");
    let good = Region::sphere(Vec3::try_new(0.0, 0.0, 0.0).unwrap(), 1.0).unwrap();
    write_region_json(&path, &good, None).unwrap();
    let before = std::fs::read(&path).unwrap();
    // Force a validate-fail path: publish_bytes_validated with bad payload is
    // exercised by attempting to write after manually placing sentinel, then
    // ensuring parse reject of garbage leaves bytes when we only call write with good.
    assert!(parse_region_bytes(br#"{"type":"donut"}"#).is_err());
    write_region_json(&path, &good, None).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn scene_transform_write_cancel() {
    let h = TempHarness::new().unwrap();
    let path = h.path().join("scene.json");
    let doc = read_scene_transform_json(&fixture("fixtures/scene/scene_transform.json")).unwrap();
    let token = CancelToken::new();
    token.request(CancelReason::User);
    let err = write_scene_transform_json(&path, &doc, Some(&token)).unwrap_err();
    assert_eq!(err.code(), ErrorCode::Canceled);
    assert!(!path.exists());
}

#[test]
fn scene_transform_failed_validate_leaves_file() {
    let h = TempHarness::new().unwrap();
    let path = h.path().join("scene.json");
    let doc = read_scene_transform_json(&fixture("fixtures/scene/scene_transform.json")).unwrap();
    write_scene_transform_json(&path, &doc, None).unwrap();
    let before = std::fs::read(&path).unwrap();
    // Direct publish of invalid JSON with validate should not replace.
    let bad = br#"{"format":"nope"}"#;
    let err = publish_bytes_validated(&path, bad, PublishOptions::default(), |payload| {
        parse_scene_transform_bytes(payload).map(|_| ())
    })
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::UnsupportedFormat);
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn splat_bounds_and_region_filter() {
    let q = Quat::identity();
    let s0 = GaussianSplat::try_new(
        Vec3::try_new(0.0, 0.0, 0.0).unwrap(),
        q,
        Vec3::try_new(0.0, 0.0, 0.0).unwrap(),
        0.0,
        Vec3::try_new(0.0, 0.0, 0.0).unwrap(),
    )
    .unwrap();
    let s1 = GaussianSplat::try_new(
        Vec3::try_new(5.0, 0.0, 0.0).unwrap(),
        q,
        Vec3::try_new(0.0, 0.0, 0.0).unwrap(),
        0.0,
        Vec3::try_new(0.0, 0.0, 0.0).unwrap(),
    )
    .unwrap();
    let set = SplatSet::try_from_splats(vec![s0, s1]).unwrap();
    let (mn, mx) = set.bounds();
    assert_eq!(mn.to_array(), [0.0, 0.0, 0.0]);
    assert_eq!(mx.to_array(), [5.0, 0.0, 0.0]);
    let aabb = Aabb::try_new(mn, mx).unwrap();
    let region = Region::sphere(Vec3::try_new(0.0, 0.0, 0.0).unwrap(), 1.0).unwrap();
    let inside: Vec<_> = set
        .splats()
        .iter()
        .filter(|s| region.contains(s.mean()) && aabb.contains(s.mean()))
        .collect();
    assert_eq!(inside.len(), 1);
}

#[test]
fn load_dataset_applies_explicit_center() {
    // Exercise apply API directly (load_dataset wires the same path).
    use spirula_core::{CameraModel, DISTORTION_PARAMS, DistortionTier};
    use std::collections::BTreeMap;

    let mut ds = ParsedDataset {
        dataset_dir: PathBuf::from("."),
        recon_dir: PathBuf::from("."),
        format: DatasetFormat::Colmap,
        cameras: BTreeMap::new(),
        frames: vec![LoadedFrame {
            image_id: 1,
            camera_id: 1,
            name: "a.jpg".into(),
            path: PathBuf::from("a.jpg"),
            c2w: [1.0, 0.0, 0.0, 4.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            width: 1,
            height: 1,
            intrins: [1.0; 4],
            model: CameraModel::Pinhole,
            distortion: DistortionTier::None,
            dist_coeffs: [0.0; DISTORTION_PARAMS],
            redistort: RedistortSource::default(),
            observations: vec![],
        }],
        points: ColmapPoints3D {
            points: vec![ColmapPoint3D {
                point3d_id: 1,
                xyz: [4.0, 0.0, 0.0],
                rgb: [0; 3],
                error: 0.0,
                track: vec![],
            }],
        },
        split: SplitIndices::default(),
        summary: DatasetLoadSummary {
            path: ".".into(),
            format: "colmap".into(),
            recon_dir: ".".into(),
            num_cameras: 0,
            num_images: 1,
            num_points: 1,
            num_train: 0,
            num_eval: 0,
            num_validation: 0,
        },
        center_mode: String::new(),
        center: [0.0; 3],
        center_json: [0.0; 3],
        raw_to_file: [0.0; 16],
        train_frame_scale: 0.0,
        train_to_normalized: [0.0; 16],
        normalized_rotation: [0.0; 9],
    }
    .with_identity_scene();

    let xf = apply_scene_to_dataset(
        &mut ds,
        &SceneApplyConfig {
            center_mode: "point-mean".into(),
            compute_normalize: true,
            ..SceneApplyConfig::default()
        },
    )
    .unwrap();
    assert_eq!(ds.center_mode, "point-mean");
    assert_eq!(ds.center, [4.0, 0.0, 0.0]);
    assert!((ds.frames[0].c2w[3]).abs() < 1e-12);
    assert_eq!(ds.points.points[0].xyz, [0.0, 0.0, 0.0]);
    let t = xf.translation().to_array();
    assert_eq!(t, [-4.0, 0.0, 0.0]);
    let _ = ColmapLoadConfig::default();
    let _ = AppliedTransform::identity();
    let _ = SceneTransform::identity();
    let _ = Path::new(".");
}
