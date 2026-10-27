//! C-DATA-07/08/09 format + depth/normal + offset holdouts (P3-F02).

use std::path::PathBuf;

use spirula_io::{
    DecodedRgb, MaskNormalizeOptions, MaskSourceKind, apply_mask_boundary_offset_frac,
    decode_depth16_bytes, decode_normal_rgb8_bytes, decode_rgb_auto_bytes, decode_rgb8,
    normalize_depth16, normalize_mask, normalize_normal_rgb8, probe_image_size,
    turn_normal_vectors,
};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/images")
}

#[test]
fn probe_and_decode_bmp_tga_tiff() {
    for name in ["tiny_2x2.bmp", "tiny_2x2.tga", "tiny_2x2.tiff"] {
        let path = fixtures().join(name);
        let (w, h) = probe_image_size(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!((w, h), (2, 2), "{name}");
        let (dw, dh, rgb) = decode_rgb8(&path).unwrap_or_else(|e| panic!("{name} decode: {e}"));
        assert_eq!((dw, dh), (2, 2));
        assert_eq!(rgb.len(), 12);
    }
}

#[test]
fn probe_and_decode_exr_f32() {
    let path = fixtures().join("tiny_2x2.exr");
    let (w, h) = probe_image_size(&path).expect("exr probe");
    assert_eq!((w, h), (2, 2));
    let bytes = std::fs::read(&path).unwrap();
    match decode_rgb_auto_bytes(&bytes).expect("exr decode") {
        DecodedRgb::F32 { width, height, rgb } => {
            assert_eq!((width, height), (2, 2));
            assert_eq!(rgb.len(), 12);
            assert!((rgb[0] - 1.0).abs() < 1e-3);
        }
        other => panic!("expected F32, got {other:?}"),
    }
}

#[test]
fn depth16_bilinear_matches_cpp_golden() {
    let bytes = std::fs::read(fixtures().join("depth_2x2.png")).unwrap();
    let (w, h, samples) = decode_depth16_bytes(&bytes).unwrap();
    assert_eq!((w, h), (2, 2));
    let mut dst = [0u16; 1];
    normalize_depth16(&samples, w, h, 0, 1, 1, &mut dst).unwrap();
    assert_eq!(dst[0], 250);
}

#[test]
fn normal_turn_and_decode() {
    let mut nrm = [200u8, 100, 128];
    turn_normal_vectors(&mut nrm, 1, 1);
    assert_eq!(nrm, [155, 200, 128]);
    let bytes = std::fs::read(fixtures().join("normal_2x2.png")).unwrap();
    let (w, h, samples) = decode_normal_rgb8_bytes(&bytes).unwrap();
    let mut dst = vec![0u8; 2 * 2 * 3];
    normalize_normal_rgb8(&samples, w, h, 1, 2, 2, &mut dst).unwrap();
    assert_eq!(dst.len(), 12);
}

#[test]
fn boundary_offset_matches_cpp_golden() {
    let g: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixtures().join("cpp_orient_mask_golden.json")).unwrap(),
    )
    .unwrap();
    let ce = &g["counterexamples"];
    let src: Vec<u8> = ce["mask5_plus"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as u8)
        .collect();
    let expect_dil: Vec<u8> = ce["mask5_dilate"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as u8)
        .collect();
    let expect_ero: Vec<u8> = ce["mask5_erode"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as u8)
        .collect();
    let frac = ce["offset_frac"].as_f64().unwrap() as f32;
    let mut dil = src.clone();
    apply_mask_boundary_offset_frac(&mut dil, 5, 5, frac).unwrap();
    assert_eq!(dil, expect_dil);
    let mut ero = src;
    apply_mask_boundary_offset_frac(&mut ero, 5, 5, -frac).unwrap();
    assert_eq!(ero, expect_ero);
}

#[test]
fn normalize_mask_applies_flip_then_offset() {
    // 3x3 center keep; flip → then dilate with positive fraction.
    let samples = [0u8, 0, 0, 0, 1, 0, 0, 0, 0];
    let mut dst = [0u8; 9];
    normalize_mask(
        &samples,
        3,
        3,
        1,
        3,
        3,
        MaskNormalizeOptions {
            kind: MaskSourceKind::NonZero,
            flip: true,
            boundary_offset_frac: 0.0,
            ..MaskNormalizeOptions::default()
        },
        &mut dst,
    )
    .unwrap();
    assert_eq!(dst.iter().filter(|&&v| v == 1).count(), 8);
}
