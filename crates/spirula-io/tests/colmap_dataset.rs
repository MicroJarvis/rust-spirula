//! P3-F03 COLMAP production reader tests (not `fixtures.rs` checker).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use spirula_core::{CancelReason, CancelToken, ErrorCode};
use spirula_io::colmap::{
    ColmapCamera, ColmapLoadConfig, DatasetFormat, DatasetProbe, SplitConfig, bake_and_fit,
    bake_and_fit_cancelable, colmap_model_num_images, colmap_to_c2w, compute_splits,
    load_colmap_dataset, load_colmap_dataset_budgeted, probe_dataset, read_cameras_binary,
    read_cameras_text, read_images_binary, read_images_text, read_points3d_binary,
    read_points3d_text, train_subset,
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
fn probe_nerfstudio_available_metashape_xml_vs_psx() {
    let h = TempHarness::new().unwrap();
    let ns = h.path().join("ns");
    fs::create_dir_all(&ns).unwrap();
    fs::write(ns.join("transforms.json"), "{}").unwrap();
    match probe_dataset(&ns, None).unwrap() {
        DatasetProbe::Available {
            format: DatasetFormat::Nerfstudio,
            ..
        } => {}
        other => panic!("expected nerfstudio available, got {other:?}"),
    }

    let ms = h.path().join("ms");
    fs::create_dir_all(&ms).unwrap();
    // bare `meta` / bare `<chunk/>` must NOT match Metashape sniff.
    fs::write(ms.join("project.psx"), "meta").unwrap();
    match probe_dataset(&ms, None).unwrap() {
        DatasetProbe::NotADataset { .. } => {}
        other => panic!("expected not-a-dataset for bad.psx=meta, got {other:?}"),
    }
    fs::remove_file(ms.join("project.psx")).unwrap();
    fs::write(ms.join("export.xml"), "<chunk/>").unwrap();
    match probe_dataset(&ms, None).unwrap() {
        DatasetProbe::NotADataset { .. } => {}
        other => panic!("expected not-a-dataset for bad.xml=<chunk/>, got {other:?}"),
    }
    fs::write(ms.join("export.xml"), "<document><chunk/></document>").unwrap();
    match probe_dataset(&ms, None).unwrap() {
        DatasetProbe::Available {
            format: DatasetFormat::Metashape,
            ..
        } => {}
        other => panic!("expected metashape available for good.xml, got {other:?}"),
    }
    fs::remove_file(ms.join("export.xml")).unwrap();
    // Pin: project form is `<document path=.../>` — no literal `psx:` required.
    // .psx-only remains typed unavailable (no zip runtime).
    fs::write(ms.join("project.psx"), "<document path=\"scene.psz\"/>").unwrap();
    match probe_dataset(&ms, None).unwrap() {
        DatasetProbe::Unavailable {
            format: DatasetFormat::Metashape,
            ..
        } => {}
        other => panic!("expected metashape unavailable for <document path=...>, got {other:?}"),
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

#[test]
fn nontrivial_filename_all_and_eval_split_indices() {
    let root = fixtures().join("datasets/colmap-goldens/nontrivial");
    let cfg_all = ColmapLoadConfig {
        eval_mode: "filename".into(),
        split: "all".into(),
        ..Default::default()
    };
    let ds = load_colmap_dataset(&root, &cfg_all, None).unwrap();
    assert_eq!(ds.frames.len(), 2);
    let names: Vec<&str> = ds.frames.iter().map(|f| f.name.as_str()).collect();
    assert!(names[0].contains("eval") || names[1].contains("eval"));
    // Emitted-domain: train must not include the eval frame.
    for &i in &ds.split.train {
        assert!(
            ds.frames[i].name.contains("train"),
            "train idx {i} is {}",
            ds.frames[i].name
        );
    }
    for &i in &ds.split.eval {
        assert!(
            ds.frames[i].name.contains("eval"),
            "eval idx {i} is {}",
            ds.frames[i].name
        );
    }
    assert_eq!(ds.split.train.len(), 1);
    assert_eq!(ds.split.eval.len(), 1);

    let cfg_eval = ColmapLoadConfig {
        eval_mode: "filename".into(),
        split: "eval".into(),
        validation_fraction: 0.25,
        ..Default::default()
    };
    let ev = load_colmap_dataset(&root, &cfg_eval, None).unwrap();
    assert_eq!(ev.frames.len(), 1);
    assert!(ev.frames[0].name.contains("eval"));
    // Eval-only: local train = all selected; validation forced 0.
    assert_eq!(ev.split.train, vec![0]);
    assert!(ev.split.eval.is_empty());
    assert!(ev.split.validation.is_empty());
}

#[test]
fn bake_and_fit_rejects_bad_focal_direct_and_load() {
    for params in [
        vec![-100.0, -100.0, 32.0, 24.0],
        vec![0.0, 0.0, 32.0, 24.0],
        vec![1e100, 1e100, 32.0, 24.0],
    ] {
        let cam = ColmapCamera {
            camera_id: 1,
            model: "PINHOLE".into(),
            width: 64,
            height: 48,
            params: params.clone(),
        };
        assert!(
            bake_and_fit(&cam).is_err(),
            "direct bake_and_fit must reject {params:?}"
        );
    }

    let h = TempHarness::new().unwrap();
    let d = h.path().join("ds");
    fs::create_dir_all(d.join("images")).unwrap();
    fs::copy(tiny().join("images/tiny.png"), d.join("images/tiny.png")).unwrap();
    fs::write(d.join("images.txt"), "1 1 0 0 0 0 0 0 1 tiny.png\n\n").unwrap();

    // Text path
    fs::write(d.join("cameras.txt"), "1 PINHOLE 64 48 -100 -100 32 24\n").unwrap();
    let err = load_colmap_dataset(&d, &ColmapLoadConfig::default(), None).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ValueOutOfRange);

    // Binary path: declared count 1, bad focals.
    let mut cam_bin = Vec::new();
    push_u64(&mut cam_bin, 1);
    push_i32(&mut cam_bin, 1);
    push_i32(&mut cam_bin, 1); // PINHOLE
    push_u64(&mut cam_bin, 64);
    push_u64(&mut cam_bin, 48);
    for v in [-100.0, -100.0, 32.0, 24.0] {
        push_f64(&mut cam_bin, v);
    }
    fs::remove_file(d.join("cameras.txt")).ok();
    fs::write(d.join("cameras.bin"), &cam_bin).unwrap();
    // images as text still
    let err = load_colmap_dataset(&d, &ColmapLoadConfig::default(), None).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ValueOutOfRange);

    // Reader alone still accepts finite f64; bake gate is the boundary.
    let cams = read_cameras_binary(&d.join("cameras.bin"), None).unwrap();
    assert!(bake_and_fit(&cams[&1]).is_err());
}

#[test]
fn points_bin_huge_count_tiny_file_rejects_before_reserve() {
    let h = TempHarness::new().unwrap();
    let p = h.path().join("points3D.bin");
    // Declared count within MAX_POINTS but file is only 8 bytes → must not
    // Vec::with_capacity(50e6)-style reserve; fail on remaining-byte check.
    let n = 1_000_000u64;
    fs::write(&p, n.to_le_bytes()).unwrap();
    let err = read_points3d_binary(&p, None).unwrap_err();
    assert!(
        matches!(
            err.code(),
            ErrorCode::MalformedRecord | ErrorCode::ValueOutOfRange
        ),
        "got {:?}",
        err.code()
    );
}

#[test]
fn observations_count_within_limit_but_truncated() {
    let h = TempHarness::new().unwrap();
    let p = h.path().join("images.bin");
    let mut buf = Vec::new();
    push_u64(&mut buf, 1);
    push_i32(&mut buf, 1);
    for v in [1.0, 0.0, 0.0, 0.0] {
        push_f64(&mut buf, v);
    }
    for v in [0.0, 0.0, 0.0] {
        push_f64(&mut buf, v);
    }
    push_i32(&mut buf, 1);
    buf.extend_from_slice(b"a.png\0");
    // Declare 10 observations but provide none.
    push_u64(&mut buf, 10);
    fs::write(&p, &buf).unwrap();
    let err = read_images_binary(&p, None).unwrap_err();
    assert_eq!(err.code(), ErrorCode::MalformedRecord);
}

#[test]
fn after_start_cancel_during_large_observation_parse() {
    let h = TempHarness::new().unwrap();
    let p = h.path().join("images.bin");
    let mut buf = Vec::new();
    push_u64(&mut buf, 1);
    push_i32(&mut buf, 1);
    for v in [1.0, 0.0, 0.0, 0.0] {
        push_f64(&mut buf, v);
    }
    for v in [0.0, 0.0, 0.0] {
        push_f64(&mut buf, v);
    }
    push_i32(&mut buf, 1);
    buf.extend_from_slice(b"big.png\0");
    let n_obs = 200_000u64;
    push_u64(&mut buf, n_obs);
    for _ in 0..n_obs {
        push_f64(&mut buf, 1.0);
        push_f64(&mut buf, 2.0);
        push_u64(&mut buf, u64::MAX);
    }
    fs::write(&p, &buf).unwrap();

    let cancel = CancelToken::new();
    let cancel2 = cancel.clone();
    let handle = std::thread::spawn(move || {
        // After-start: let parse begin, then cancel.
        std::thread::sleep(Duration::from_millis(5));
        assert!(cancel2.request(CancelReason::User));
    });
    let err = read_images_binary(&p, Some(&cancel)).unwrap_err();
    handle.join().unwrap();
    assert_eq!(err.code(), ErrorCode::Canceled);
}

#[test]
fn after_start_cancel_during_track_parse() {
    let h = TempHarness::new().unwrap();
    let p = h.path().join("points3D.bin");
    let mut buf = Vec::new();
    push_u64(&mut buf, 1);
    push_u64(&mut buf, 1);
    for v in [0.0, 0.0, 0.0] {
        push_f64(&mut buf, v);
    }
    buf.extend_from_slice(&[1u8, 2, 3]);
    push_f64(&mut buf, 0.0);
    let track_len = 200_000u64;
    push_u64(&mut buf, track_len);
    for i in 0..track_len {
        push_i32(&mut buf, 1);
        push_i32(&mut buf, i as i32);
    }
    fs::write(&p, &buf).unwrap();

    let cancel = CancelToken::new();
    let cancel2 = cancel.clone();
    let handle = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(5));
        assert!(cancel2.request(CancelReason::User));
    });
    let err = read_points3d_binary(&p, Some(&cancel)).unwrap_err();
    handle.join().unwrap();
    assert_eq!(err.code(), ErrorCode::Canceled);
}

#[test]
fn cancel_before_text_read_capped_observes_token() {
    let h = TempHarness::new().unwrap();
    let p = h.path().join("cameras.txt");
    // Multi-chunk body so a non-canceled path would stream past the first poll.
    let mut body = String::from("# cameras\n");
    for i in 0..8_000 {
        body.push_str(&format!("{i} PINHOLE 64 48 100 100 32 24\n"));
    }
    fs::write(&p, &body).unwrap();
    let cancel = CancelToken::new();
    assert!(cancel.request(CancelReason::User));
    let err = read_cameras_text(&p, Some(&cancel)).unwrap_err();
    assert_eq!(err.code(), ErrorCode::Canceled);
}

#[cfg(unix)]
#[test]
fn after_start_cancel_during_text_read_capped() {
    // FIFO so cancel is armed after poll_now / first read chunk — no wall-clock race
    // against a warm tmpfs finishing a multi-MiB regular file in <5ms.
    let h = TempHarness::new().unwrap();
    let p = h.path().join("cameras.txt");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&p)
            .status()
            .unwrap()
            .success()
    );

    let cancel = CancelToken::new();
    let cancel_w = cancel.clone();
    let p_w = p.clone();
    let writer = std::thread::spawn(move || {
        let mut f = fs::OpenOptions::new().write(true).open(&p_w).unwrap();
        // Unblock the reader's first chunk after its opening poll_now.
        f.write_all(b"# ").unwrap();
        f.write_all(&vec![b'x'; 256 * 1024]).unwrap();
        assert!(cancel_w.request(CancelReason::User));
        // Keep feeding until the reader observes cancel (ignore EPIPE).
        let pad = vec![b'x'; 256 * 1024];
        for _ in 0..16 {
            if f.write_all(&pad).is_err() {
                break;
            }
        }
    });

    let err = read_cameras_text(&p, Some(&cancel)).unwrap_err();
    let _ = writer.join();
    assert_eq!(err.code(), ErrorCode::Canceled);
}

#[test]
fn probe_header_atoll_and_bin_preference_hold() {
    let h = TempHarness::new().unwrap();
    let dir = h.path().join("m");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("images.txt"),
        "# Number of images: 10, mean observations per image: 0\n",
    )
    .unwrap();
    assert_eq!(colmap_model_num_images(&dir), 10);

    // Truncated bin wins preference and does not fall back to the headered txt.
    fs::write(dir.join("images.bin"), [1u8, 2, 3]).unwrap();
    assert_eq!(colmap_model_num_images(&dir), -1);
}

#[test]
fn compute_splits_table_smoke_exported() {
    let names = vec!["img_eval_b.png".into(), "img_train_a.png".into()];
    let plan = compute_splits(
        &names,
        &SplitConfig {
            eval_mode: "filename".into(),
            split: "all".into(),
            validation_fraction: 0.0,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(plan.train, vec![1]);
    assert_eq!(plan.eval, vec![0]);
}

#[test]
fn pin_bake_host_golden_holdout() {
    use spirula_core::{FrameCamera, Vec3};
    use spirula_io::colmap::{COLMAP_MODELS, colmap_to_c2w};
    let expected =
        fs::read_to_string(fixtures().join("datasets/colmap-goldens/bake-pin/expected.txt"))
            .expect("bake-pin expected.txt");
    let inputs: Vec<Vec<f64>> = serde_json::from_str(
        &fs::read_to_string(fixtures().join("datasets/colmap-goldens/bake-pin/inputs.json"))
            .expect("bake-pin inputs.json"),
    )
    .expect("inputs json");
    assert_eq!(inputs.len(), COLMAP_MODELS.len());

    let mut model_lines = Vec::new();
    let mut ray_lines: std::collections::BTreeMap<usize, Vec<f64>> =
        std::collections::BTreeMap::new();
    let mut pose_vals: Option<Vec<f64>> = None;
    for line in expected.lines() {
        let mut toks = line.split_whitespace();
        let Some(tag) = toks.next() else {
            continue;
        };
        if tag == "pose" {
            pose_vals = Some(toks.map(|t| t.parse().unwrap()).collect());
            continue;
        }
        if tag == "ray" {
            let id: usize = toks.next().unwrap().parse().unwrap();
            let px: Vec<f64> = toks.map(|t| t.parse().unwrap()).collect();
            assert_eq!(px.len(), 6, "ray {id} needs 3 pixel pairs");
            ray_lines.insert(id, px);
            continue;
        }
        // Model row starts with numeric id.
        let id: usize = tag.parse().unwrap();
        let rest: Vec<&str> = std::iter::once(tag).chain(toks).collect();
        model_lines.push((id, rest.join(" ")));
    }
    assert_eq!(model_lines.len(), COLMAP_MODELS.len());
    assert_eq!(ray_lines.len(), COLMAP_MODELS.len());

    // Independent unfitted rays — Rust side always uses baked FrameCamera;
    // golden pixels come from C++ camhost::ray_pixel (source when fitted).
    const RAYS: [(f64, f64, f64); 3] = [(0.0, 0.0, 1.0), (0.2, -0.1, 1.0), (-0.15, 0.25, 1.0)];
    for (id, params) in inputs.iter().enumerate() {
        let info = &COLMAP_MODELS[id];
        let cam = ColmapCamera {
            camera_id: id as i32,
            model: info.name.into(),
            width: 640,
            height: 480,
            params: params.clone(),
        };
        let bi = bake_and_fit(&cam).unwrap_or_else(|e| panic!("{}: {e}", info.name));
        let line = &model_lines.iter().find(|(i, _)| *i == id).unwrap().1;
        let mut toks = line.split_whitespace();
        let _gid = toks.next().unwrap();
        let g_model: i32 = toks.next().unwrap().parse().unwrap();
        let g_tier: i32 = toks.next().unwrap().parse().unwrap();
        let gfx: f64 = toks.next().unwrap().parse().unwrap();
        let gfy: f64 = toks.next().unwrap().parse().unwrap();
        let gcx: f64 = toks.next().unwrap().parse().unwrap();
        let gcy: f64 = toks.next().unwrap().parse().unwrap();
        let g_source: i32 = toks.next().unwrap().parse().unwrap();
        let g_fit: f64 = toks.next().unwrap().parse().unwrap();
        let mut g_dist = [0.0f64; 8];
        for d in &mut g_dist {
            *d = toks.next().unwrap().parse().unwrap();
        }
        assert_eq!(bi.model.as_i32(), g_model, "{} model", info.name);
        assert_eq!(bi.distortion.as_i32(), g_tier, "{} tier", info.name);
        assert_eq!(
            bi.source.source_model, g_source,
            "{} source {} vs {g_source}",
            info.name, bi.source.source_model
        );
        // Absolute intrinsics tolerances (no relative / 50% coordinate escape).
        assert!(
            (f64::from(bi.fx) - gfx).abs() < 2.0,
            "{} fx {} vs {gfx}",
            info.name,
            bi.fx
        );
        assert!(
            (f64::from(bi.fy) - gfy).abs() < 2.0,
            "{} fy {} vs {gfy}",
            info.name,
            bi.fy
        );
        assert!(
            (f64::from(bi.cx) - gcx).abs() < 2.0,
            "{} cx {} vs {gcx}",
            info.name,
            bi.cx
        );
        assert!(
            (f64::from(bi.cy) - gcy).abs() < 2.0,
            "{} cy {} vs {gcy}",
            info.name,
            bi.cy
        );
        assert!(
            (f64::from(bi.source.fit_max_px) - g_fit).abs() < 1.0,
            "{} fit_max_px {} vs {g_fit}",
            info.name,
            bi.source.fit_max_px
        );
        for (i, gd) in g_dist.iter().enumerate() {
            let bd = f64::from(bi.dist[i]);
            assert!(
                (bd - gd).abs() < 0.05,
                "{} dist[{i}] {bd} vs {gd}",
                info.name
            );
        }

        let g_ray = ray_lines.get(&id).expect("ray golden");
        let coeffs = bi.dist.map(f64::from);
        let frame = FrameCamera::try_new(
            bi.model,
            bi.distortion,
            640,
            480,
            f64::from(bi.fx),
            f64::from(bi.fy),
            f64::from(bi.cx),
            f64::from(bi.cy),
            coeffs,
        )
        .unwrap_or_else(|e| panic!("{} frame: {e}", info.name));
        // Absolute pixel budget: exact bake must be tight; fitted source path
        // may differ from baked FrameCamera by about fit_max_px.
        let ray_atol = g_fit.max(0.0) + 1.0;
        for (ri, &(x, y, z)) in RAYS.iter().enumerate() {
            let ray = Vec3::try_new(x, y, z).unwrap();
            let p = frame
                .ray_to_pixel(ray)
                .unwrap_or_else(|e| panic!("{} baked ray: {e}", info.name));
            let px = p.x();
            let py = p.y();
            let gpx = g_ray[ri * 2];
            let gpy = g_ray[ri * 2 + 1];
            assert!(
                (px - gpx).abs() <= ray_atol,
                "{} ray[{ri}] px {px} vs {gpx} (atol {ray_atol})",
                info.name
            );
            assert!(
                (py - gpy).abs() <= ray_atol,
                "{} ray[{ri}] py {py} vs {gpy} (atol {ray_atol})",
                info.name
            );
        }
    }

    let pose = pose_vals.expect("pose golden");
    assert_eq!(pose.len(), 12);
    let c2w = colmap_to_c2w(
        [0.9238795325112867, 0.0, 0.3826834323650898, 0.0],
        [1.0, 2.0, 3.0],
    )
    .unwrap();
    for (g, v) in pose.iter().zip(c2w.iter()) {
        assert!((g - v).abs() < 1e-9, "pose {g} vs {v}");
    }
}

#[test]
fn bake_and_fit_cancelable_observes_pre_canceled_fov() {
    let cam = ColmapCamera {
        camera_id: 1,
        model: "FOV".into(),
        width: 64,
        height: 48,
        params: vec![50.0, 50.0, 32.0, 24.0, 0.7],
    };
    let cancel = CancelToken::new();
    assert!(cancel.request(CancelReason::User));
    let err = bake_and_fit_cancelable(&cam, Some(&cancel)).unwrap_err();
    assert_eq!(err.code(), ErrorCode::Canceled);
}

#[test]
fn after_start_cancel_during_load_bake_loop_before_dense_fit() {
    // Many exact-tier cameras so cancel lands in the load bake loop *before*
    // any dense LM (no FOV here). Mid-camera poll_now observes the token.
    let h = TempHarness::new().unwrap();
    let ds = h.path().join("ds");
    fs::create_dir_all(&ds).unwrap();
    let n = 8_000usize;
    let mut cams = String::new();
    for i in 1..=n {
        cams.push_str(&format!("{i} PINHOLE 64 48 100 100 32 24\n"));
    }
    fs::write(ds.join("cameras.txt"), cams).unwrap();
    fs::write(ds.join("images.txt"), "1 1 0 0 0 0 0 0 1 frame.png\n\n").unwrap();

    let cancel = CancelToken::new();
    let cancel2 = cancel.clone();
    let handle = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(5));
        assert!(cancel2.request(CancelReason::User));
    });
    let err = load_colmap_dataset(
        &ds,
        &ColmapLoadConfig {
            require_image_files: false,
            ..Default::default()
        },
        Some(&cancel),
    )
    .unwrap_err();
    handle.join().unwrap();
    assert_eq!(err.code(), ErrorCode::Canceled);
}

#[test]
fn after_start_cancel_during_load_fit_path() {
    // Multiple FOV cameras force fit_colmap_source_cancelable on the real load
    // path. Domain-scan azimuth polls observe after-start cancel; dense
    // mid-fit_model LM remains non-preemptible (not claimed here).
    let h = TempHarness::new().unwrap();
    let ds = h.path().join("ds");
    fs::create_dir_all(&ds).unwrap();
    let n = 6usize;
    let mut cams = String::new();
    for i in 1..=n {
        cams.push_str(&format!("{i} FOV 64 48 50 50 32 24 0.7\n"));
    }
    fs::write(ds.join("cameras.txt"), cams).unwrap();
    fs::write(ds.join("images.txt"), "1 1 0 0 0 0 0 0 1 frame.png\n\n").unwrap();

    let cancel = CancelToken::new();
    let cancel2 = cancel.clone();
    let handle = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(5));
        assert!(cancel2.request(CancelReason::User));
    });
    let err = load_colmap_dataset(
        &ds,
        &ColmapLoadConfig {
            require_image_files: false,
            ..Default::default()
        },
        Some(&cancel),
    )
    .unwrap_err();
    handle.join().unwrap();
    assert_eq!(err.code(), ErrorCode::Canceled);
}

#[test]
fn load_budget_injection_preserves_normal_results() {
    let root = tiny();
    let normal = load_colmap_dataset(&root, &ColmapLoadConfig::default(), None).unwrap();
    let budgeted = load_colmap_dataset_budgeted(
        &root,
        &ColmapLoadConfig::default(),
        None,
        spirula_io::colmap::MAX_LOAD_MEMORY_BYTES,
    )
    .unwrap();
    assert_eq!(normal.frames.len(), budgeted.frames.len());
    assert_eq!(normal.split, budgeted.split);
    assert_eq!(normal.summary.num_train, budgeted.summary.num_train);
    assert_eq!(normal.summary.num_eval, budgeted.summary.num_eval);

    let err =
        load_colmap_dataset_budgeted(&root, &ColmapLoadConfig::default(), None, 64).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
}

#[allow(dead_code)]
fn _keep_path_import(_: &Path) {}
