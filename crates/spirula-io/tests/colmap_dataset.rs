//! P3-F03 COLMAP production reader tests (not `fixtures.rs` checker).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use spirula_core::{CancelReason, CancelToken, ErrorCode};
use spirula_io::colmap::{
    ColmapLoadConfig, DatasetFormat, DatasetProbe, colmap_to_c2w, load_colmap_dataset,
    probe_dataset, read_cameras_binary, read_cameras_text, read_images_binary, read_images_text,
    read_points3d_binary, read_points3d_text, train_subset,
};
use spirula_io::temp_harness::TempHarness;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn tiny() -> PathBuf {
    fixtures().join("datasets/tiny-colmap")
}

#[test]
fn tiny_text_matches_golden_and_tracks() {
    let root = tiny();
    let cams = read_cameras_text(&root.join("cameras.txt"), None).unwrap();
    let imgs = read_images_text(&root.join("images.txt"), None).unwrap();
    let pts = read_points3d_text(&root.join("points3D.txt"), None).unwrap();
    assert_eq!(cams.len(), 1);
    assert_eq!(cams[&1].model, "PINHOLE");
    assert_eq!(cams[&1].params, vec![100.0, 100.0, 32.0, 24.0]);
    assert_eq!(imgs.len(), 1);
    assert_eq!(imgs[&1].name, "tiny.png");
    assert_eq!(pts.num(), 4);
    assert_eq!(
        pts.points[0].track,
        [spirula_io::colmap::ColmapTrackElem {
            image_id: 1,
            point2d_idx: 0,
        }]
    );
    // Empty POINTS2D line → empty observations (fixture has blank second line).
    assert!(imgs[&1].observations.is_empty());
}

#[test]
fn tiny_bin_text_equivalence() {
    let root = tiny();
    let ct = read_cameras_text(&root.join("cameras.txt"), None).unwrap();
    let cb = read_cameras_binary(&root.join("cameras.bin"), None).unwrap();
    assert_eq!(ct, cb);
    let it = read_images_text(&root.join("images.txt"), None).unwrap();
    let ib = read_images_binary(&root.join("images.bin"), None).unwrap();
    assert_eq!(it[&1].image_id, ib[&1].image_id);
    assert_eq!(it[&1].qvec, ib[&1].qvec);
    assert_eq!(it[&1].tvec, ib[&1].tvec);
    assert_eq!(it[&1].camera_id, ib[&1].camera_id);
    assert_eq!(it[&1].name, ib[&1].name);
    let pt = read_points3d_text(&root.join("points3D.txt"), None).unwrap();
    let pb = read_points3d_binary(&root.join("points3D.bin"), None).unwrap();
    assert_eq!(pt.num(), pb.num());
    for (a, b) in pt.points.iter().zip(pb.points.iter()) {
        assert_eq!(a.point3d_id, b.point3d_id);
        assert_eq!(a.xyz, b.xyz);
        assert_eq!(a.rgb, b.rgb);
        assert!((a.error - b.error).abs() < 1e-12);
        assert_eq!(a.track, b.track);
    }
}

#[test]
fn load_tiny_prefers_bin_and_summary() {
    let ds = load_colmap_dataset(&tiny(), &ColmapLoadConfig::default(), None).unwrap();
    assert_eq!(ds.format, DatasetFormat::Colmap);
    assert_eq!(ds.frames.len(), 1);
    assert_eq!(ds.summary.num_points, 4);
    assert_eq!(ds.summary.num_train, 1);
    let c2w = colmap_to_c2w([1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 0.0]).unwrap();
    assert_eq!(ds.frames[0].c2w, c2w);
    assert!((ds.frames[0].intrins[0] - 100.0).abs() < 1e-5);
}

#[test]
fn nontrivial_pose_lens_no_points_and_filename_split() {
    let root = fixtures().join("datasets/colmap-goldens/nontrivial");
    let cfg = ColmapLoadConfig {
        eval_mode: "filename".into(),
        split: "train".into(),
        ..Default::default()
    };
    let ds = load_colmap_dataset(&root, &cfg, None).unwrap();
    assert_eq!(ds.frames.len(), 1);
    assert!(ds.frames[0].name.contains("train"));
    assert_eq!(ds.points.num(), 0);
    // Non-identity pose: translation column nonzero.
    assert!(ds.frames[0].c2w[3].abs() > 1e-9 || ds.frames[0].c2w[11].abs() > 1e-9);
    // OPENCV nontrivial distortion kept (or demoted OpenCv).
    assert!(ds.frames[0].dist_coeffs.iter().any(|v| v.abs() > 0.0));

    let cfg = ColmapLoadConfig {
        eval_mode: "filename".into(),
        split: "eval".into(),
        ..Default::default()
    };
    let ev = load_colmap_dataset(&root, &cfg, None).unwrap();
    assert_eq!(ev.frames.len(), 1);
    assert!(ev.frames[0].name.contains("eval"));
}

#[test]
fn probe_nerfstudio_and_metashape_unavailable() {
    let h = TempHarness::new().unwrap();
    let ns = h.path().join("ns");
    fs::create_dir_all(&ns).unwrap();
    fs::write(ns.join("transforms.json"), "{}").unwrap();
    match probe_dataset(&ns, None).unwrap() {
        DatasetProbe::Unavailable {
            format: DatasetFormat::Nerfstudio,
            ..
        } => {}
        other => panic!("expected nerfstudio unavailable, got {other:?}"),
    }

    let ms = h.path().join("ms");
    fs::create_dir_all(&ms).unwrap();
    fs::write(ms.join("project.psx"), "meta").unwrap();
    match probe_dataset(&ms, None).unwrap() {
        DatasetProbe::Unavailable {
            format: DatasetFormat::Metashape,
            ..
        } => {}
        other => panic!("expected metashape unavailable, got {other:?}"),
    }
}

#[test]
fn missing_camera_duplicate_and_truncated_errors() {
    let h = TempHarness::new().unwrap();
    let d = h.path().join("ds");
    fs::create_dir_all(d.join("images")).unwrap();
    fs::write(d.join("cameras.txt"), "#\n1 PINHOLE 64 48 100 100 32 24\n").unwrap();
    fs::write(
        d.join("images.txt"),
        "#\n1 1 0 0 0 0 0 0 99 missing.png\n\n",
    )
    .unwrap();
    // no image file + require → all missing
    let err = load_colmap_dataset(&d, &ColmapLoadConfig::default(), None).unwrap_err();
    assert_eq!(err.code(), ErrorCode::NotFound);

    // Provide image but wrong camera id
    fs::copy(tiny().join("images/tiny.png"), d.join("images/missing.png")).unwrap();
    // rename image name
    fs::write(
        d.join("images.txt"),
        "#\n1 1 0 0 0 0 0 0 99 missing.png\n\n",
    )
    .unwrap();
    let err = load_colmap_dataset(&d, &ColmapLoadConfig::default(), None).unwrap_err();
    assert_eq!(err.code(), ErrorCode::MalformedRecord);

    // Truncated binary
    let mut f = fs::File::create(d.join("cameras.bin")).unwrap();
    f.write_all(&1u64.to_le_bytes()).unwrap();
    // incomplete record
    let err = read_cameras_binary(&d.join("cameras.bin"), None).unwrap_err();
    assert_eq!(err.code(), ErrorCode::MalformedRecord);
}

#[test]
fn empty_train_filename_errors_empty_eval_ok() {
    let names = vec![
        "a_train.png".into(),
        "b_train.png".into(),
        "c_train.png".into(),
    ];
    let cfg = spirula_io::colmap::SplitConfig {
        eval_mode: "filename".into(),
        split: "eval".into(),
        ..Default::default()
    };
    let eval = train_subset(3, &names, &cfg).unwrap();
    assert!(eval.is_empty());

    let cfg2 = spirula_io::colmap::SplitConfig {
        eval_mode: "filename".into(),
        split: "train".into(),
        ..Default::default()
    };
    let names2 = vec!["only_eval.png".into()];
    let err = train_subset(1, &names2, &cfg2).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidArgument);
}

#[test]
fn cancel_during_parse_observes_token() {
    let root = tiny();
    let cancel = CancelToken::new();
    assert!(cancel.request(CancelReason::User));
    // Already canceled → first poll_now fails.
    let err = load_colmap_dataset(&root, &ColmapLoadConfig::default(), Some(&cancel)).unwrap_err();
    assert_eq!(err.code(), ErrorCode::Canceled);
    let _ = Duration::from_millis(1);
}

#[test]
fn huge_count_budget_rejects() {
    let h = TempHarness::new().unwrap();
    let p = h.path().join("cameras.bin");
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(spirula_io::colmap::MAX_CAMERAS + 1).to_le_bytes());
    fs::write(&p, bytes).unwrap();
    let err = read_cameras_binary(&p, None).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
}

#[test]
fn explicit_recon_dir_and_sparse_probe() {
    let h = TempHarness::new().unwrap();
    let ds = h.path().join("ds");
    let recon = ds.join("sparse/0");
    fs::create_dir_all(recon.join("..")).unwrap();
    fs::create_dir_all(&recon).unwrap();
    fs::create_dir_all(ds.join("images")).unwrap();
    fs::copy(tiny().join("cameras.txt"), recon.join("cameras.txt")).unwrap();
    fs::copy(tiny().join("images.txt"), recon.join("images.txt")).unwrap();
    fs::copy(tiny().join("images/tiny.png"), ds.join("images/tiny.png")).unwrap();
    let loaded = load_colmap_dataset(&ds, &ColmapLoadConfig::default(), None).unwrap();
    assert_eq!(loaded.frames.len(), 1);

    let cfg = ColmapLoadConfig {
        recon_dir: Some("sparse/0".into()),
        ..Default::default()
    };
    let loaded2 = load_colmap_dataset(&ds, &cfg, None).unwrap();
    assert_eq!(loaded2.frames.len(), 1);
}

#[test]
fn illegal_parent_path_rejected() {
    let h = TempHarness::new().unwrap();
    let d = h.path().join("ds");
    fs::create_dir_all(d.join("images")).unwrap();
    fs::write(d.join("cameras.txt"), "1 PINHOLE 64 48 100 100 32 24\n").unwrap();
    fs::write(d.join("images.txt"), "1 1 0 0 0 0 0 0 1 ../secret.png\n\n").unwrap();
    let err = load_colmap_dataset(&d, &ColmapLoadConfig::default(), None).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidArgument);
}

#[test]
fn illegal_absolute_image_path_rejected() {
    let h = TempHarness::new().unwrap();
    let d = h.path().join("ds");
    fs::create_dir_all(d.join("images")).unwrap();
    fs::write(d.join("cameras.txt"), "1 PINHOLE 64 48 100 100 32 24\n").unwrap();
    fs::write(
        d.join("images.txt"),
        "1 1 0 0 0 0 0 0 1 /tmp/escape.png\n\n",
    )
    .unwrap();
    let err = load_colmap_dataset(&d, &ColmapLoadConfig::default(), None).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidArgument);
}

fn push_f64(buf: &mut Vec<u8>, v: f64) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn push_i32(buf: &mut Vec<u8>, v: i32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn push_u64(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn write_pinhole_camera_bin(buf: &mut Vec<u8>, camera_id: i32) {
    push_i32(buf, camera_id);
    push_i32(buf, 1); // PINHOLE
    push_u64(buf, 64);
    push_u64(buf, 48);
    for v in [100.0, 100.0, 32.0, 24.0] {
        push_f64(buf, v);
    }
}

fn write_image_bin(buf: &mut Vec<u8>, image_id: i32, camera_id: i32, name: &str) {
    push_i32(buf, image_id);
    for v in [1.0, 0.0, 0.0, 0.0] {
        push_f64(buf, v);
    }
    for v in [0.0, 0.0, 0.0] {
        push_f64(buf, v);
    }
    push_i32(buf, camera_id);
    buf.extend_from_slice(name.as_bytes());
    buf.push(0);
    push_u64(buf, 0); // no POINTS2D
}

fn write_point3d_bin(buf: &mut Vec<u8>, point3d_id: u64) {
    push_u64(buf, point3d_id);
    for v in [0.0, 0.0, 0.0] {
        push_f64(buf, v);
    }
    buf.extend_from_slice(&[1u8, 2, 3]);
    push_f64(buf, 0.0);
    push_u64(buf, 0); // empty track
}

#[test]
fn duplicate_ids_text_and_binary_error() {
    let h = TempHarness::new().unwrap();
    let d = h.path();

    // cameras text
    fs::write(
        d.join("dup_cam.txt"),
        "1 PINHOLE 64 48 100 100 32 24\n1 PINHOLE 64 48 100 100 32 24\n",
    )
    .unwrap();
    let err = read_cameras_text(&d.join("dup_cam.txt"), None).unwrap_err();
    assert_eq!(err.code(), ErrorCode::MalformedRecord);

    // cameras binary
    let mut cam_bin = Vec::new();
    push_u64(&mut cam_bin, 2);
    write_pinhole_camera_bin(&mut cam_bin, 7);
    write_pinhole_camera_bin(&mut cam_bin, 7);
    fs::write(d.join("dup_cam.bin"), &cam_bin).unwrap();
    let err = read_cameras_binary(&d.join("dup_cam.bin"), None).unwrap_err();
    assert_eq!(err.code(), ErrorCode::MalformedRecord);

    // images text
    fs::write(
        d.join("dup_img.txt"),
        "1 1 0 0 0 0 0 0 1 a.png\n\n1 1 0 0 0 0 0 0 1 b.png\n\n",
    )
    .unwrap();
    let err = read_images_text(&d.join("dup_img.txt"), None).unwrap_err();
    assert_eq!(err.code(), ErrorCode::MalformedRecord);

    // images binary
    let mut img_bin = Vec::new();
    push_u64(&mut img_bin, 2);
    write_image_bin(&mut img_bin, 3, 1, "a.png");
    write_image_bin(&mut img_bin, 3, 1, "b.png");
    fs::write(d.join("dup_img.bin"), &img_bin).unwrap();
    let err = read_images_binary(&d.join("dup_img.bin"), None).unwrap_err();
    assert_eq!(err.code(), ErrorCode::MalformedRecord);

    // points3D text
    fs::write(
        d.join("dup_pts.txt"),
        "9 0 0 0 1 2 3 0.0\n9 1 1 1 4 5 6 0.1\n",
    )
    .unwrap();
    let err = read_points3d_text(&d.join("dup_pts.txt"), None).unwrap_err();
    assert_eq!(err.code(), ErrorCode::MalformedRecord);

    // points3D binary
    let mut pts_bin = Vec::new();
    push_u64(&mut pts_bin, 2);
    write_point3d_bin(&mut pts_bin, 42);
    write_point3d_bin(&mut pts_bin, 42);
    fs::write(d.join("dup_pts.bin"), &pts_bin).unwrap();
    let err = read_points3d_binary(&d.join("dup_pts.bin"), None).unwrap_err();
    assert_eq!(err.code(), ErrorCode::MalformedRecord);
}

#[test]
fn text_camera_model_name_and_exact_arity() {
    let h = TempHarness::new().unwrap();
    let d = h.path();

    fs::write(d.join("bad_model.txt"), "1 NOT_A_MODEL 64 48 1 2 3\n").unwrap();
    let err = read_cameras_text(&d.join("bad_model.txt"), None).unwrap_err();
    assert_eq!(err.code(), ErrorCode::UnsupportedFormat);

    // PINHOLE needs exactly 4 params — surplus rejected.
    fs::write(d.join("surplus.txt"), "1 PINHOLE 64 48 100 100 32 24 999\n").unwrap();
    let err = read_cameras_text(&d.join("surplus.txt"), None).unwrap_err();
    assert_eq!(err.code(), ErrorCode::MalformedRecord);

    // missing param
    fs::write(d.join("missing.txt"), "1 PINHOLE 64 48 100 100 32\n").unwrap();
    let err = read_cameras_text(&d.join("missing.txt"), None).unwrap_err();
    assert_eq!(err.code(), ErrorCode::MalformedRecord);

    // exact arity still ok
    fs::write(d.join("ok.txt"), "1 PINHOLE 64 48 100 100 32 24\n").unwrap();
    let cams = read_cameras_text(&d.join("ok.txt"), None).unwrap();
    assert_eq!(cams[&1].params.len(), 4);
}

#[test]
fn points_optional() {
    let h = TempHarness::new().unwrap();
    let d = h.path().join("ds");
    fs::create_dir_all(d.join("images")).unwrap();
    fs::copy(tiny().join("cameras.txt"), d.join("cameras.txt")).unwrap();
    fs::copy(tiny().join("images.txt"), d.join("images.txt")).unwrap();
    fs::copy(tiny().join("images/tiny.png"), d.join("images/tiny.png")).unwrap();
    let ds = load_colmap_dataset(&d, &ColmapLoadConfig::default(), None).unwrap();
    assert_eq!(ds.points.num(), 0);
    assert_eq!(ds.frames.len(), 1);
}

#[allow(dead_code)]
fn _keep_path_import(_: &Path) {}
