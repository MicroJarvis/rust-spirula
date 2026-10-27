//! P3-F02 fixture holdouts for EXIF orientation and mask normalize.
//! C++ pixel goldens: `fixtures/images/cpp_orient_mask_golden.json`.

use std::path::PathBuf;

use spirula_io::{
    ApplyPolicy, ExifOrientation, ExifOrientationMode, MaskNormalizeOptions, MaskSourceKind,
    apply_orientation, cpu_resize_u8, decode_mask_image_bytes, decode_rgb8, nearest_resize_u8,
    normalize_mask, orient_pixels, oriented_size, probe_image_size, read_exif_orientations,
    read_jpeg_exif_orientation, read_path_exif_orientation,
};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/images")
}

fn load_cpp_golden() -> serde_json::Value {
    let path = fixtures().join("cpp_orient_mask_golden.json");
    let raw = std::fs::read_to_string(&path).expect("cpp golden");
    serde_json::from_str(&raw).expect("cpp golden json")
}

#[test]
fn probe_nonsquare_png_size() {
    let (w, h) = probe_image_size(&fixtures().join("nonsquare_4x2.png")).unwrap();
    assert_eq!((w, h), (4, 2));
}

#[test]
fn jpeg_exif_orientation_6() {
    let path = fixtures().join("orient6_1x1.jpg");
    let o = read_path_exif_orientation(&path).unwrap();
    assert_eq!(o.get(), 6);
    let xf = o.transform();
    assert_eq!(xf.turns_cw, 1);
    assert!(!xf.mirror);
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(read_jpeg_exif_orientation(&bytes).unwrap().get(), 6);
}

#[test]
fn png_path_skips_exif_open() {
    let o = read_path_exif_orientation(&fixtures().join("nonsquare_4x2.png")).unwrap();
    assert_eq!(o, ExifOrientation::IDENTITY);
}

#[test]
fn apply_orient6_swaps_nonsquare_rgb() {
    let (w, h, rgb) = decode_rgb8(&fixtures().join("nonsquare_4x2.png")).unwrap();
    assert_eq!((w, h), (4, 2));
    let o = ExifOrientation::from_u8(6).unwrap();
    let (ow, oh) = oriented_size(w, h, o.transform().turns_cw);
    assert_eq!((ow, oh), (2, 4));
    let mut dst = vec![0u8; (ow as usize) * (oh as usize) * 3];
    let (dw, dh) = apply_orientation(&rgb, w, h, 3, o, ApplyPolicy::TurnsOnly, &mut dst).unwrap();
    assert_eq!((dw, dh), (2, 4));
    for px in dst.as_chunks::<3>().0 {
        assert!(
            (*px == [200, 0, 0] || *px == [0, 200, 0]),
            "unexpected rgb {px:?}"
        );
    }
}

#[test]
fn mask_aligns_after_orientation_and_resize() {
    let bytes = std::fs::read(fixtures().join("mask_4x2.png")).unwrap();
    let (w, h, ch, samples) = decode_mask_image_bytes(&bytes).unwrap();
    assert_eq!((w, h), (4, 2));
    let mut dst = [0u8; 8]; // 2x4 after orient6
    let opts = MaskNormalizeOptions {
        kind: MaskSourceKind::NonZero,
        flip: false,
        orientation: ExifOrientation::from_u8(6).unwrap(),
        apply_policy: ApplyPolicy::TurnsOnly,
    };
    normalize_mask(&samples, w, h, ch, 2, 4, opts, &mut dst).unwrap();
    assert_eq!(dst.iter().filter(|&&v| v == 1).count(), 4);
    assert_eq!(dst.iter().filter(|&&v| v == 0).count(), 4);
}

#[test]
fn missing_and_illegal_metadata() {
    assert!(probe_image_size(&fixtures().join("nope.png")).is_err());
    assert!(ExifOrientation::from_u8(0).is_err());
    assert!(ExifOrientation::from_u8(9).is_err());
    assert!(ExifOrientationMode::parse("bogus").is_err());
    let truncated = [0xFFu8, 0xD8, 0xFF, 0xE1, 0x00, 0x40, 0x45, 0x78];
    assert!(read_jpeg_exif_orientation(&truncated).is_err());
}

#[test]
fn read_exif_orientations_mode_none_empty() {
    let paths = [fixtures().join("orient6_1x1.jpg")];
    let v = read_exif_orientations(ExifOrientationMode::None, &paths).unwrap();
    assert!(v.is_empty());
    let v = read_exif_orientations(ExifOrientationMode::Apply, &paths).unwrap();
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].get(), 6);
}

#[test]
fn nearest_resize_mask_to_image_size() {
    let src = [1u8, 0, 1, 0];
    let mut dst = [0u8; 16];
    nearest_resize_u8(&src, 2, 2, 4, 4, &mut dst).unwrap();
    assert_eq!(dst[0], 1);
    assert_eq!(dst[15], 0);
}

#[test]
fn cpp_golden_eight_orientations_full_and_turns() {
    let g = load_cpp_golden();
    assert_eq!(g["schema"], "spirula-image-orient-mask-cpp-golden");
    assert_eq!(
        g["reference_commit"],
        "9a24f09927944a795e7ba1bd8243a625bf419479"
    );
    let src_w = g["src_w"].as_u64().unwrap() as u32;
    let src_h = g["src_h"].as_u64().unwrap() as u32;
    let gray: Vec<u8> = g["gray"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as u8)
        .collect();
    let rgb: Vec<u8> = g["rgb"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as u8)
        .collect();
    assert_eq!(gray.len(), 6);
    assert_eq!(rgb.len(), 18);

    for entry in g["orientations"].as_array().unwrap() {
        let o = entry["orientation"].as_u64().unwrap() as u8;
        let xf = ExifOrientation::from_u8(o).unwrap().transform();
        assert_eq!(xf.turns_cw, entry["turns_cw"].as_u64().unwrap() as u8);
        assert_eq!(xf.mirror, entry["mirror"].as_bool().unwrap());
        let out_w = entry["out_w"].as_u64().unwrap() as u32;
        let out_h = entry["out_h"].as_u64().unwrap() as u32;
        assert_eq!(oriented_size(src_w, src_h, xf.turns_cw), (out_w, out_h));

        let mut full_g = vec![0u8; (out_w * out_h) as usize];
        let mut turns_g = vec![0u8; (out_w * out_h) as usize];
        let mut full_rgb = vec![0u8; (out_w * out_h * 3) as usize];
        let mut turns_rgb = vec![0u8; (out_w * out_h * 3) as usize];
        apply_orientation(
            &gray,
            src_w,
            src_h,
            1,
            ExifOrientation::from_u8(o).unwrap(),
            ApplyPolicy::FullTransform,
            &mut full_g,
        )
        .unwrap();
        apply_orientation(
            &gray,
            src_w,
            src_h,
            1,
            ExifOrientation::from_u8(o).unwrap(),
            ApplyPolicy::TurnsOnly,
            &mut turns_g,
        )
        .unwrap();
        apply_orientation(
            &rgb,
            src_w,
            src_h,
            3,
            ExifOrientation::from_u8(o).unwrap(),
            ApplyPolicy::FullTransform,
            &mut full_rgb,
        )
        .unwrap();
        apply_orientation(
            &rgb,
            src_w,
            src_h,
            3,
            ExifOrientation::from_u8(o).unwrap(),
            ApplyPolicy::TurnsOnly,
            &mut turns_rgb,
        )
        .unwrap();

        let expect = |key: &str| -> Vec<u8> {
            entry[key]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u8)
                .collect()
        };
        assert_eq!(full_g, expect("full_gray"), "o={o} full_gray");
        assert_eq!(turns_g, expect("turns_gray"), "o={o} turns_gray");
        assert_eq!(full_rgb, expect("full_rgb"), "o={o} full_rgb");
        assert_eq!(turns_rgb, expect("turns_rgb"), "o={o} turns_rgb");

        // Mask turns + nearest 2x3 position check
        let mut bin = vec![0u8; 6];
        for i in 0..6 {
            bin[i] = u8::from(gray[i] != 0);
        }
        let mut turns_only = xf;
        turns_only.mirror = false;
        let (mw, mh) = oriented_size(src_w, src_h, turns_only.turns_cw);
        let mut mob = vec![0u8; (mw * mh) as usize];
        if turns_only.identity() {
            mob.copy_from_slice(&bin);
        } else {
            orient_pixels(&bin, src_w, src_h, 1, turns_only, &mut mob).unwrap();
        }
        let mut mdst = [0u8; 6];
        nearest_resize_u8(&mob, mw, mh, 2, 3, &mut mdst).unwrap();
        assert_eq!(
            mdst.as_slice(),
            expect("mask_turns_nearest_2x3").as_slice(),
            "o={o} mask nearest"
        );
    }
}

#[test]
fn cpp_golden_alpha_cpu_resize_threshold() {
    let g = load_cpp_golden();
    let alpha: Vec<u8> = g["alpha"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as u8)
        .collect();
    let src_w = g["src_w"].as_u64().unwrap() as u32;
    let src_h = g["src_h"].as_u64().unwrap() as u32;
    for entry in g["alpha_resize"].as_array().unwrap() {
        let dw = entry["dst_w"].as_u64().unwrap() as u32;
        let dh = entry["dst_h"].as_u64().unwrap() as u32;
        let expect_raw: Vec<u8> = entry["raw"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u8)
            .collect();
        let expect_bin: Vec<u8> = entry["bin"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u8)
            .collect();
        let mut raw = vec![0u8; (dw * dh) as usize];
        cpu_resize_u8(&alpha, src_w, src_h, dw, dh, &mut raw).unwrap();
        assert_eq!(raw, expect_raw, "alpha raw {dw}x{dh}");
        let mut bin = raw.clone();
        for v in &mut bin {
            *v = u8::from(*v >= 128);
        }
        assert_eq!(bin, expect_bin, "alpha bin {dw}x{dh}");

        // Through normalize_mask Alpha path (RGBA carrying alpha).
        let mut rgba = vec![0u8; 6 * 4];
        for i in 0..6 {
            rgba[i * 4 + 3] = alpha[i];
        }
        let mut dst = vec![0u8; (dw * dh) as usize];
        let opts = MaskNormalizeOptions {
            kind: MaskSourceKind::AlphaThreshold128,
            ..MaskNormalizeOptions::default()
        };
        normalize_mask(&rgba, src_w, src_h, 4, dw, dh, opts, &mut dst).unwrap();
        assert_eq!(dst, expect_bin, "normalize alpha {dw}x{dh}");
    }
}
