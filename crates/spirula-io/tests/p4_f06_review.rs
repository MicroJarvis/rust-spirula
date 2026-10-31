//! Independent P4-F06 IO contract review; fixed bytes and malformed records.
use serde_json::{Value, json};
use spirula_core::{CancelReason, CancelToken, ErrorCode};
use spirula_io::{
    encode_preview_png_rgb8, load_preview_request, load_preview_scene, publish_preview_png,
    temp_harness::TempHarness,
};
use std::fs;

fn descriptor() -> Value {
    json!({
        "schema": "spirula-preview-request", "version": 1,
        "splat_ply": "scene.ply",
        "camera": {
            "model": "PINHOLE", "width": 3, "height": 2,
            "fx": 1.0, "fy": 1.0, "cx": 0.5, "cy": 0.5,
            "c2w": {"rotation_row_major": [1,0,0,0,1,0,0,0,1], "translation": [0,0,0]}
        },
        "background": [0.1,0.2,0.3],
        "thresholds": {"alpha_cutoff": 0.1, "early_out_transmittance": 0.0001}
    })
}

#[test]
fn descriptor_resolves_paths_and_rejects_unsupported_or_malformed_semantics() {
    let harness = TempHarness::new().unwrap();
    let path = harness.path().join("preview.json");
    fs::write(&path, serde_json::to_vec(&descriptor()).unwrap()).unwrap();
    let parsed = load_preview_request(&path, None).unwrap();
    assert_eq!(parsed.splat_ply, harness.path().join("scene.ply"));
    assert_eq!((parsed.camera.width(), parsed.camera.height()), (3, 2));
    assert_eq!(parsed.background, [0.1, 0.2, 0.3]);
    let mut cases = Vec::new();
    let mut v = descriptor();
    v["version"] = json!(2);
    cases.push(v);
    let mut v = descriptor();
    v["camera"]["width"] = json!(0);
    cases.push(v);
    let mut v = descriptor();
    v["camera"]["fx"] = json!(-1);
    cases.push(v);
    let mut v = descriptor();
    v["camera"]["model"] = json!("FISHEYE");
    cases.push(v);
    let mut v = descriptor();
    v["camera"]["c2w"]["rotation_row_major"] = json!([2, 0, 0, 0, 1, 0, 0, 0, 1]);
    cases.push(v);
    let mut v = descriptor();
    v["camera"]["distortion"] = json!({"tier":"OpenCV","coefficients":[0.1,0,0,0]});
    cases.push(v);
    let mut v = descriptor();
    v["primitive"] = json!("mip");
    cases.push(v);
    let mut v = descriptor();
    v["sh_degree"] = json!(3);
    cases.push(v);
    let mut v = descriptor();
    v["color_space"] = json!("srgb");
    cases.push(v);
    let mut v = descriptor();
    v["camera"]["distortoin"] = json!([0.1, 0, 0, 0]);
    cases.push(v);
    let mut v = descriptor();
    v["thresholds"]["alpha_cutof"] = json!(0.9);
    cases.push(v);
    let mut v = descriptor();
    v["budgets"] = json!({"max_pixels":5});
    cases.push(v);
    let mut v = descriptor();
    v["budgets"] = json!({"max_gpu_bytes":0});
    cases.push(v);
    for invalid in cases {
        fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
        assert!(
            load_preview_request(&path, None).is_err(),
            "must reject {invalid}"
        );
    }
}

#[test]
fn png_roundtrip_keeps_exact_asymmetric_rgb_pixels_and_opaque_layout() {
    let rgb = [
        13, 89, 174, 26, 51, 77, 255, 0, 128, 0, 255, 1, 0, 0, 0, 255, 255, 255,
    ];
    let bytes = encode_preview_png_rgb8(3, 2, &rgb, 1 << 20).unwrap();
    let decoded = image::load_from_memory(&bytes).unwrap();
    assert_eq!(decoded.color(), image::ColorType::Rgb8);
    assert_eq!((decoded.width(), decoded.height()), (3, 2));
    assert_eq!(decoded.to_rgb8().as_raw(), &rgb);
    assert!(encode_preview_png_rgb8(3, 2, &rgb, 1).is_err());
    assert!(encode_preview_png_rgb8(3, 2, &rgb[..15], 1 << 20).is_err());
    assert!(encode_preview_png_rgb8(0, 2, &[], 1 << 20).is_err());
}

#[test]
fn scene_file_cap_applies_to_ply_and_cannot_be_bypassed_by_larger_host_budget() {
    let harness = TempHarness::new().unwrap();
    let source = include_bytes!("../../../fixtures/splats/render_splat.ply");
    let split = source.iter().position(|b| *b == b'\n').unwrap() + 1;
    let mut oversized = source[..split].to_vec();
    oversized.extend_from_slice(format!("comment {}\n", "x".repeat(2048)).as_bytes());
    oversized.extend_from_slice(&source[split..]);
    fs::write(harness.path().join("scene.ply"), oversized).unwrap();
    let path = harness.path().join("request.json");
    let mut request = descriptor();
    request["budgets"] = json!({"max_file_bytes":1024,"max_host_bytes":1048576});
    let bytes = serde_json::to_vec(&request).unwrap();
    assert!(bytes.len() < 1024);
    fs::write(&path, bytes).unwrap();
    let error = load_preview_scene(&path, None).unwrap_err();
    assert_eq!(error.code(), ErrorCode::ValueOutOfRange);
}

#[test]
fn scene_file_cap_is_independent_of_descriptor_hardcap() {
    let harness = TempHarness::new().unwrap();
    let source = include_bytes!("../../../fixtures/splats/render_splat.ply");
    let mut ply = b"ply\ncomment ".to_vec();
    ply.extend(std::iter::repeat_n(b'x', (1 << 20) + 16));
    ply.push(b'\n');
    ply.extend_from_slice(&source[4..]);
    fs::write(harness.path().join("scene.ply"), ply).unwrap();
    let mut request = descriptor();
    request["budgets"] = json!({"max_file_bytes":2 << 20,"max_host_bytes":8 << 20});
    let path = harness.path().join("request.json");
    fs::write(&path, serde_json::to_vec(&request).unwrap()).unwrap();
    let loaded = load_preview_scene(&path, None).unwrap();
    assert_eq!(loaded.request.budgets.max_file_bytes, 2 << 20);
    assert_eq!(loaded.splats.len(), 1);

    // A large configured scene cap must not enlarge the descriptor read cap.
    let mut descriptor_bytes = serde_json::to_vec(&request).unwrap();
    descriptor_bytes.resize((1 << 20) + 1, b' ');
    fs::write(&path, descriptor_bytes).unwrap();
    assert_eq!(
        load_preview_request(&path, None).unwrap_err().code(),
        ErrorCode::ValueOutOfRange
    );
}

#[test]
fn atomic_png_publish_preserves_existing_output_on_cancel_or_invalid_data() {
    let harness = TempHarness::new().unwrap();
    let path = harness.path().join("preview.png");
    let old = encode_preview_png_rgb8(1, 1, &[1, 2, 3], 1 << 20).unwrap();
    fs::write(&path, &old).unwrap();
    let valid = encode_preview_png_rgb8(1, 1, &[4, 5, 6], 1 << 20).unwrap();
    let cancel = CancelToken::new();
    cancel.request(CancelReason::User);
    assert_eq!(
        publish_preview_png(&path, &valid, Some(&cancel))
            .unwrap_err()
            .code(),
        ErrorCode::Canceled
    );
    assert_eq!(fs::read(&path).unwrap(), old);
    for invalid in [
        b"".as_slice(),
        b"garbage",
        b"\x89PNG\r\n\x1a\nnot PNG chunks",
    ] {
        assert!(publish_preview_png(&path, invalid, None).is_err());
        assert_eq!(fs::read(&path).unwrap(), old);
    }
    let rgb = image::RgbImage::from_raw(1, 1, vec![9, 8, 7]).unwrap();
    let mut jpeg = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(rgb)
        .write_to(&mut jpeg, image::ImageFormat::Jpeg)
        .unwrap();
    assert!(
        publish_preview_png(&path, jpeg.get_ref(), None).is_err(),
        "valid JPEG bytes must not be published under the PNG contract"
    );
    assert_eq!(fs::read(&path).unwrap(), old);
    publish_preview_png(&path, &valid, None).unwrap();
    assert_eq!(fs::read(&path).unwrap(), valid);
}

#[test]
fn atomic_writer_observes_cancel_after_temp_write_before_rename_and_preview_retry_succeeds() {
    use spirula_io::{PublishOptions, publish_writer, recover_publish_orphans};
    use std::io::Write;

    let harness = TempHarness::new().unwrap();
    let path = harness.path().join("cancel-at-commit.png");
    let old = encode_preview_png_rgb8(1, 1, &[1, 2, 3], 1024).unwrap();
    let next = encode_preview_png_rgb8(1, 1, &[4, 5, 6], 1024).unwrap();
    fs::write(&path, &old).unwrap();
    let cancel = CancelToken::new();
    let mut wrote_temp = false;
    let result = publish_writer(
        &path,
        PublishOptions::default().with_cancel(&cancel),
        |file| {
            file.write_all(&next).unwrap();
            wrote_temp = true;
            cancel.request(CancelReason::User);
            Ok(next.len() as u64)
        },
    );
    assert!(
        wrote_temp,
        "cancellation must happen after actual temporary-file write"
    );
    assert_eq!(result.unwrap_err().code(), ErrorCode::Canceled);
    assert_eq!(fs::read(&path).unwrap(), old);
    assert!(recover_publish_orphans(&path).unwrap().is_empty());
    // Preview uses the same atomic helper with the cancellation option wired.
    publish_preview_png(&path, &next, None).unwrap();
    assert_eq!(fs::read(path).unwrap(), next);
}
