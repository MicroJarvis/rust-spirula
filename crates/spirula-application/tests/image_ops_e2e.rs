//! End-to-end: application/session → spirula-io image orient/mask (P3-F02).

use std::path::PathBuf;

use spirula_application::{ApplicationService, PrepareMaskArgs, SessionLimits};
use spirula_io::{ApplyPolicy, MaskSourceKind};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/images")
}

#[test]
fn session_probe_and_orient_nonsquare_png() {
    let app = ApplicationService::start(SessionLimits::for_test());
    let path = fixtures().join("nonsquare_4x2.png");
    let (w, h) = app.probe_image_file_size(&path).expect("probe");
    assert_eq!((w, h), (4, 2));

    let oriented = app
        .prepare_oriented_rgb_file(&path, Some(6), ApplyPolicy::TurnsOnly)
        .expect("orient");
    assert_eq!((oriented.width, oriented.height), (2, 4));
    assert_eq!(oriented.orientation, 6);
    assert_eq!(oriented.rgb.len(), 2 * 4 * 3);
    for px in oriented.rgb.as_chunks::<3>().0 {
        assert!(*px == [200, 0, 0] || *px == [0, 200, 0]);
    }
}

#[test]
fn session_normalize_mask_file() {
    let app = ApplicationService::start(SessionLimits::for_test());
    let path = fixtures().join("mask_4x2.png");
    let mask = app
        .prepare_normalized_mask_file(
            &path,
            PrepareMaskArgs {
                dst_w: 2,
                dst_h: 4,
                kind: MaskSourceKind::NonZero,
                orientation: 6,
                policy: ApplyPolicy::TurnsOnly,
                flip: false,
            },
        )
        .expect("mask");
    assert_eq!((mask.width, mask.height), (2, 4));
    assert_eq!(mask.samples.iter().filter(|&&v| v == 1).count(), 4);
    assert_eq!(mask.samples.iter().filter(|&&v| v == 0).count(), 4);
}

#[test]
fn session_jpeg_exif_via_override_none() {
    let app = ApplicationService::start(SessionLimits::for_test());
    let path = fixtures().join("orient6_1x1.jpg");
    let oriented = app
        .prepare_oriented_rgb_file(&path, None, ApplyPolicy::TurnsOnly)
        .expect("orient jpeg");
    assert_eq!(oriented.orientation, 6);
    // 1x1 → after 90° CW still 1x1
    assert_eq!((oriented.width, oriented.height), (1, 1));
}
