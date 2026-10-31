//! Parent-owned independent preview packaging review. Expected bytes are fixed
//! closed-form/C++ lround references, never produced by the CPU/GPU renderer.
use spirula_core::{
    CameraPose, CancelToken, ErrorCode, GaussianSplat, OpenCvDistortion, PinholeCamera, Quat,
    SplatSet, Vec3,
};
use spirula_render::{
    BackgroundPolicy, ColorSpace, CompositeImage, CompositeOptions, CompositePixel,
    RenderPrimitive, RenderRequest, RenderThresholds, pack_composite_to_preview_rgb8,
    run_headless_gpu_preview,
};

fn image(width: u32, height: u32, rgb: &[[f64; 3]]) -> CompositeImage {
    CompositeImage {
        contract_version: spirula_render::COMPOSITE_CONTRACT_VERSION,
        width,
        height,
        alpha_cutoff: 0.1,
        effective_alpha_cutoff: 0.1,
        early_out_transmittance: 1e-4,
        effective_early_out: 1e-4,
        background: BackgroundPolicy::Constant([0.1, 0.2, 0.3]),
        pixels: rgb
            .iter()
            .map(|&rgb| CompositePixel {
                rgb,
                ..CompositePixel::default()
            })
            .collect(),
    }
}

#[test]
fn quantization_matches_independent_cpp_lround_and_preserves_row_channel_order() {
    let rows: Vec<_> = include_str!("data/p4-f06-review.tsv")
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            let input: [f64; 3] = std::array::from_fn(|i| fields[i + 1].parse::<f64>().unwrap());
            let output: [u8; 3] = std::array::from_fn(|i| fields[i + 4].parse::<u8>().unwrap());
            (input, output)
        })
        .collect();
    for (input, expected) in &rows {
        let frame = pack_composite_to_preview_rgb8(&image(1, 1, &[*input])).unwrap();
        assert_eq!(frame.rgb8, *expected);
        assert_eq!(frame.stride_bytes, 3);
        assert_eq!(
            frame.source_background,
            BackgroundPolicy::Constant([0.1, 0.2, 0.3])
        );
    }
    let colors: Vec<_> = rows[..6].iter().map(|row| row.0).collect();
    let expected: Vec<_> = rows[..6].iter().flat_map(|row| row.1).collect();
    let frame = pack_composite_to_preview_rgb8(&image(3, 2, &colors)).unwrap();
    assert_eq!((frame.width, frame.height, frame.stride_bytes), (3, 2, 9));
    assert_eq!(
        frame.rgb8, expected,
        "row-major RGB only, without alpha or gamma"
    );
}

#[test]
fn packing_rejects_invalid_extents_nonfinite_color_and_unadmitted_clear() {
    let valid = image(1, 1, &[[0.5; 3]]);
    for bad in [
        CompositeImage {
            width: 0,
            ..valid.clone()
        },
        CompositeImage {
            height: 0,
            ..valid.clone()
        },
        CompositeImage {
            width: 2,
            ..valid.clone()
        },
        CompositeImage {
            width: u32::MAX,
            height: u32::MAX,
            ..valid.clone()
        },
        CompositeImage {
            pixels: vec![],
            ..valid.clone()
        },
    ] {
        let result = std::panic::catch_unwind(|| pack_composite_to_preview_rgb8(&bad));
        assert!(result.is_ok(), "invalid geometry must reject, not panic");
        assert!(result.unwrap().is_err());
    }
    for channel in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(pack_composite_to_preview_rgb8(&image(1, 1, &[[channel, 0.0, 0.0]])).is_err());
    }
    let mut clear = valid.clone();
    clear.background = BackgroundPolicy::Clear;
    assert_eq!(
        pack_composite_to_preview_rgb8(&clear).unwrap_err().code(),
        ErrorCode::UnimplementedOption
    );
    let mut obsolete = valid;
    obsolete.contract_version = 0;
    assert_eq!(
        pack_composite_to_preview_rgb8(&obsolete)
            .unwrap_err()
            .code(),
        ErrorCode::UnsupportedVersion
    );
}

fn analytic_request(cx: f64) -> RenderRequest {
    let camera = PinholeCamera::try_new(
        3,
        2,
        1.0,
        1.0,
        cx,
        0.5,
        OpenCvDistortion::try_new(0.0, 0.0, 0.0, 0.0).unwrap(),
        CameraPose::identity(),
    )
    .unwrap();
    let splat = GaussianSplat::try_new(
        Vec3::try_new(0.0, 0.0, -2.0).unwrap(),
        Quat::identity(),
        Vec3::try_new(-30.0, -30.0, -30.0).unwrap(),
        0.0,
        Vec3::try_new(-2.0, 0.0, 2.0).unwrap(),
    )
    .unwrap();
    RenderRequest::try_new(
        camera,
        SplatSet::try_from_splats(vec![splat]).unwrap(),
        RenderPrimitive::Gaussian3d,
        ColorSpace::LinearRgb,
        0,
        RenderThresholds {
            alpha_cutoff: 0.1,
            early_out_transmittance: 1e-4,
        },
    )
    .unwrap()
}

#[test]
fn real_headless_facade_matches_asymmetric_full_image_golden_and_camera_changes() {
    let options = CompositeOptions {
        background: BackgroundPolicy::Constant([0.1, 0.2, 0.3]),
        ..CompositeOptions::default()
    };
    let result =
        match run_headless_gpu_preview(&analytic_request(0.5), options, &CancelToken::new()) {
            Ok(result) => result,
            Err(error) => {
                if std::env::var_os("SPIRULA_REQUIRE_RENDER_GPU").is_some() {
                    panic!("required headless facade failed: {error:?}");
                }
                assert_eq!(error.code(), ErrorCode::NoAdapter);
                eprintln!("headless facade unavailable: {error:?}");
                return;
            }
        };
    let mut expected = [26, 51, 77].repeat(6);
    expected[..3].copy_from_slice(&[13, 89, 174]);
    assert_eq!(result.frame.rgb8, expected);
    assert_eq!(
        (
            result.frame.width,
            result.frame.height,
            result.frame.stride_bytes
        ),
        (3, 2, 9)
    );
    assert_eq!(result.metrics.rgb8_bytes, 18);
    let moved =
        run_headless_gpu_preview(&analytic_request(1.5), options, &CancelToken::new()).unwrap();
    let mut shifted = [26, 51, 77].repeat(6);
    shifted[3..6].copy_from_slice(&[13, 89, 174]);
    assert_eq!(
        moved.frame.rgb8, shifted,
        "camera input must change rendered pixels"
    );
    assert_ne!(moved.frame.rgb8, result.frame.rgb8);
}
