//! P3-F02 fixture holdouts for EXIF orientation and mask normalize.

use std::path::PathBuf;

use spirula_io::{
    ApplyPolicy, ExifOrientation, ExifOrientationMode, MaskNormalizeOptions, MaskSourceKind,
    apply_orientation, decode_mask_image_bytes, decode_rgb8, nearest_resize_u8, normalize_mask,
    oriented_size, probe_image_size, read_exif_orientations, read_jpeg_exif_orientation,
    read_path_exif_orientation,
};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/images")
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
    // Left column of source was red → after 90° CW should dominate top or side.
    // Spot-check: every pixel remains pure red or pure green (no bleed).
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
    // Source left half kept → after 90° CW keepers form a contiguous block.
    assert_eq!(dst.iter().filter(|&&v| v == 1).count(), 4);
    assert_eq!(dst.iter().filter(|&&v| v == 0).count(), 4);
}

#[test]
fn missing_and_illegal_metadata() {
    assert!(probe_image_size(&fixtures().join("nope.png")).is_err());
    assert!(ExifOrientation::from_u8(0).is_err());
    assert!(ExifOrientation::from_u8(9).is_err());
    assert!(ExifOrientationMode::parse("bogus").is_err());
    // APP1 claims 64 bytes of payload but file ends early.
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
