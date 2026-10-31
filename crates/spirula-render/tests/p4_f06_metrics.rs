//! P4-F06 evidence: 800×600 F05 pressure workload stage/copy labels.
//! Retained cleanup on injected wait timeout lives in `preview` unit tests
//! (render-private on_context helper). Parent-owned `p4_f06_review.rs` is
//! not edited here.
use std::time::Instant;

use spirula_core::{
    CameraPose, CancelToken, GaussianSplat, OpenCvDistortion, PinholeCamera, Quat, SplatSet, Vec3,
};
use spirula_gpu::CapabilityRequest;
use spirula_render::{
    BackgroundPolicy, ColorSpace, CompositeOptions, PREVIEW_FLOAT_BYTES_PER_PIXEL,
    PREVIEW_RGB8_BYTES_PER_PIXEL, RenderPrimitive, RenderRequest, RenderThresholds,
    run_headless_gpu_preview,
};

fn require_host() -> bool {
    match spirula_gpu::GpuContext::open(&CapabilityRequest::diagnose_default()) {
        Ok(_ctx) => true,
        Err(err) => {
            assert!(
                std::env::var_os("SPIRULA_REQUIRE_RENDER_GPU").is_none(),
                "required GPU unavailable: {err:?}"
            );
            eprintln!("p4_f06_metrics GPU test skipped: {err:?}");
            false
        }
    }
}

/// Same 65-source / 800×600 pressure scene as `p4_f05_compositing.rs`
/// (`pair_count == 487_500`).
fn pressure_request() -> RenderRequest {
    let camera = PinholeCamera::try_new(
        800,
        600,
        600.0,
        610.0,
        400.0,
        300.0,
        OpenCvDistortion::try_new(0.0, 0.0, 0.0, 0.0).unwrap(),
        CameraPose::identity(),
    )
    .unwrap();
    let splats = (0..65)
        .map(|i| {
            GaussianSplat::try_new(
                Vec3::try_new((i % 3) as f64 * 0.001, 0.0, -2.0).unwrap(),
                Quat::try_from_wxyz(0.9238795325, 0.0, 0.0, 0.3826834324).unwrap(),
                Vec3::try_new(100.0_f64.ln(), 100.0_f64.ln(), 100.0_f64.ln()).unwrap(),
                0.0,
                Vec3::try_new(0.0, 0.0, 0.0).unwrap(),
            )
            .unwrap()
        })
        .collect();
    RenderRequest::try_new(
        camera,
        SplatSet::try_from_splats(splats).unwrap(),
        RenderPrimitive::Gaussian3d,
        ColorSpace::LinearRgb,
        0,
        RenderThresholds::default(),
    )
    .unwrap()
}

#[test]
fn headless_preview_800x600_pressure_reports_stage_walls_and_honest_copy_labels() {
    if !require_host() {
        return;
    }
    let token = CancelToken::new();
    let wall = Instant::now();
    let result = run_headless_gpu_preview(
        &pressure_request(),
        CompositeOptions {
            background: BackgroundPolicy::Constant([0.1, 0.2, 0.3]),
            max_pixels: 800 * 600,
            max_host_bytes: 512 << 20,
            max_gpu_bytes: 512 << 20,
            ..CompositeOptions::default()
        },
        &token,
    )
    .expect("800x600 pressure preview");
    let total_ns = wall.elapsed().as_nanos();
    let m = &result.metrics;
    assert_eq!((m.width, m.height), (800, 600));
    assert_eq!(m.splat_count, 65);
    assert_eq!(m.composite.pair_count, 487_500);
    assert_eq!(m.composite.pixel_count, 480_000);
    assert_eq!(m.rgb8_bytes, 800 * 600 * 3);
    assert_eq!(m.host_rgb8_payload_bytes, m.rgb8_bytes);
    let expected_reserve =
        480_000u64 * (PREVIEW_FLOAT_BYTES_PER_PIXEL + PREVIEW_RGB8_BYTES_PER_PIXEL);
    assert_eq!(m.host_float_plus_rgb8_reserve_bytes, expected_reserve);
    assert!(m.wall_render_ns > 0);
    assert!(m.retained_wait_ok, "retained submissions must settle");
    // CompositeMetrics: wall/GPU stage ns + accounted peaks — not D2H bus counters.
    assert!(
        m.composite.composite_encode_ns > 0 || m.composite.composite_sync_ns > 0 || total_ns > 0
    );
    eprintln!(
        "p4_f06_metrics 800x600 pressure pairs={} pixels={} wall_render_ns={} host_rgb8_payload_bytes={} host_float_plus_rgb8_reserve_bytes={} retained_wait_ok={} composite_encode_ns={} composite_sync_ns={} tile_peak={} sort_peak={} comp_peak={} (payload/reserve labels only; not RSS/VRAM/bus-moved-bytes)",
        m.composite.pair_count,
        m.composite.pixel_count,
        m.wall_render_ns,
        m.host_rgb8_payload_bytes,
        m.host_float_plus_rgb8_reserve_bytes,
        m.retained_wait_ok,
        m.composite.composite_encode_ns,
        m.composite.composite_sync_ns,
        m.composite.tile_accounted_peak_bytes,
        m.composite.sort_accounted_peak_bytes,
        m.composite.composite_accounted_peak_bytes,
    );
}
