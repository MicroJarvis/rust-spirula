use spirula_core::{
    CameraPose, CancelReason, CancelToken, ErrorCode, GaussianSplat, OpenCvDistortion,
    PinholeCamera, Quat, SplatSet, Vec3,
};
use spirula_gpu::{BufferDesc, BufferUsage, CapabilityRequest, GpuContext, ResourceLabel};
use spirula_render::{
    ColorSpace, CpuTileBuilder, GpuTileBuilder, RenderPrimitive, RenderRequest, RenderThresholds,
    TileBuildOptions,
};
fn request(n: usize, scale: [f64; 3], mean: [f64; 3]) -> RenderRequest {
    let camera = PinholeCamera::try_new(
        64,
        48,
        32.0,
        32.0,
        32.0,
        24.0,
        OpenCvDistortion::try_new(0.0, 0.0, 0.0, 0.0).unwrap(),
        CameraPose::identity(),
    )
    .unwrap();
    let splats = (0..n)
        .map(|i| {
            GaussianSplat::try_new(
                Vec3::try_new(mean[0] + (i % 3) as f64 * 0.001, mean[1], mean[2]).unwrap(),
                Quat::try_from_wxyz(0.9238795325, 0.0, 0.0, 0.3826834324).unwrap(),
                Vec3::try_new(scale[0].ln(), scale[1].ln(), scale[2].ln()).unwrap(),
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
fn host() -> Option<GpuContext> {
    match GpuContext::open(&CapabilityRequest::default()) {
        Ok(ctx) => Some(ctx),
        Err(err) => {
            assert!(
                std::env::var_os("SPIRULA_REQUIRE_RENDER_GPU").is_none(),
                "required GPU unavailable: {err:?}"
            );
            eprintln!("GPU tile test skipped: {err:?}");
            None
        }
    }
}
#[test]
fn source_order_scan_culling_and_integrity() {
    let request = request(65, [0.3, 0.05, 0.02], [0.0, 0.0, -2.0]);
    let bins = CpuTileBuilder::build(&request, TileBuildOptions::default()).unwrap();
    bins.validate().unwrap();
    assert_eq!(bins.coverage.len(), 65);
    assert_eq!(
        bins.inclusive_offsets.last().copied().unwrap() as usize,
        bins.pairs.len()
    );
    let mut broken = bins.clone();
    broken.pairs[0].source_index = 64;
    assert!(broken.validate().is_err());
    let mut broken = bins.clone();
    broken.pairs[0].tile_id = 9999;
    assert!(broken.validate().is_err());
    let mut broken = bins.clone();
    broken.inclusive_offsets[0] += 1;
    assert!(broken.validate().is_err());
    let mut broken = bins.clone();
    broken.coverage[0].pair_count += 1;
    assert!(broken.validate().is_err());
    let mut broken = bins.clone();
    broken.projected[0].as_mut().unwrap().tile_bounds[2] = 999;
    assert!(broken.validate().is_err());
    for mean in [
        [-100.0, 0.0, -2.0],
        [100.0, 0.0, -2.0],
        [0.0, 100.0, -2.0],
        [0.0, -100.0, -2.0],
        [0.0, 0.0, 0.0],
        [0.0, 0.0, 2.0],
    ] {
        let bins = CpuTileBuilder::build(
            &self::request(1, [0.001; 3], mean),
            TileBuildOptions::default(),
        )
        .unwrap();
        assert!(bins.pairs.is_empty(), "{mean:?}");
        assert_eq!(bins.projected, vec![None]);
    }
    let empty = CpuTileBuilder::build_empty(63, 47, TileBuildOptions::default()).unwrap();
    assert_eq!(empty.tile_count(), 48);
    assert!(empty.pairs.is_empty());
}
#[test]
fn budgets_and_mutated_requests_are_checked() {
    let mut req = request(1, [100.0; 3], [0.0, 0.0, -2.0]);
    let opts = TileBuildOptions {
        max_pairs: 47,
        ..TileBuildOptions::default()
    };
    assert_eq!(
        CpuTileBuilder::build(&req, opts).unwrap_err().code(),
        ErrorCode::ValueOutOfRange
    );
    let opts = TileBuildOptions {
        max_host_bytes: 100,
        ..TileBuildOptions::default()
    };
    assert_eq!(
        CpuTileBuilder::build(&req, opts).unwrap_err().code(),
        ErrorCode::ValueOutOfRange
    );
    req.sh_degree = 1;
    assert_eq!(
        CpuTileBuilder::build(&req, TileBuildOptions::default())
            .unwrap_err()
            .code(),
        ErrorCode::UnimplementedOption
    );
    req.sh_degree = 0;
    req.contract_version += 1;
    assert_eq!(
        CpuTileBuilder::build(&req, TileBuildOptions::default())
            .unwrap_err()
            .code(),
        ErrorCode::UnsupportedVersion
    );
}
/// Minimum ellipse quadratic over a closed tile rectangle. The stationary
/// interior or one edge minimum is exact for a positive-definite quadratic.
fn ellipse_min(pixel: [f64; 2], cov: [f64; 3], rect: [f64; 4]) -> f64 {
    let [xx, xy, yy] = cov;
    let det = xx * yy - xy * xy;
    let (a, b, c) = (yy / det, -xy / det, xx / det);
    let lo = [rect[0] - pixel[0], rect[1] - pixel[1]];
    let hi = [rect[2] - pixel[0], rect[3] - pixel[1]];
    if lo[0] <= 0.0 && hi[0] >= 0.0 && lo[1] <= 0.0 && hi[1] >= 0.0 {
        return 0.0;
    }
    let mut best = f64::INFINITY;
    for x in [lo[0], hi[0]] {
        let y = (-b * x / c).clamp(lo[1], hi[1]);
        best = best.min(a * x * x + 2.0 * b * x * y + c * y * y);
    }
    for y in [lo[1], hi[1]] {
        let x = (-b * y / a).clamp(lo[0], hi[0]);
        best = best.min(a * x * x + 2.0 * b * x * y + c * y * y);
    }
    best
}
#[test]
fn conservative_bins_cover_the_ellipse_and_measure_extra_pairs() {
    for mean in [
        [0.0, 0.0, -2.0],
        [-2.0, 0.0, -2.0],
        [2.0, 0.0, -2.0],
        [0.0, -1.5, -2.0],
        [0.0, 1.5, -2.0],
        [-1.9, 1.4, -2.0],
    ] {
        let req = request(1, [0.5, 0.025, 0.01], mean);
        let bins = CpuTileBuilder::build(&req, TileBuildOptions::default()).unwrap();
        let projected = bins.projected[0].unwrap();
        let limit = (2.0 * (projected.opacity / req.thresholds.alpha_cutoff).ln()).min(3.33 * 3.33);
        let mut ellipse_pairs = 0;
        for y in 0..bins.tiles_y {
            for x in 0..bins.tiles_x {
                let rect = [
                    (f64::from(x * 8) - 0.5).max(0.0),
                    (f64::from(y * 8) - 0.5).max(0.0),
                    (f64::from((x + 1) * 8) - 0.5).min(64.0),
                    (f64::from((y + 1) * 8) - 0.5).min(48.0),
                ];
                if ellipse_min(projected.pixel, projected.covariance, rect) <= limit {
                    ellipse_pairs += 1;
                    assert!(
                        bins.pairs
                            .iter()
                            .any(|p| p.tile_id == (y * bins.tiles_x + x) as usize),
                        "missed ellipse tile {x},{y} mean={mean:?}"
                    );
                }
            }
        }
        assert!(bins.pairs.len() >= ellipse_pairs);
        eprintln!(
            "aabb_conservative mean={mean:?} pairs={} exact_ellipse={} extra={} overhead_bytes={}",
            bins.pairs.len(),
            ellipse_pairs,
            bins.pairs.len() - ellipse_pairs,
            (bins.pairs.len() - ellipse_pairs) * 8
        );
    }
}
#[test]
fn gpu_non_block_scan_and_fullscreen_pressure_match_cpu() {
    let options = TileBuildOptions::default();
    for (n, scale, mean) in [
        (65, [0.3, 0.05, 0.02], [0.0, 0.0, -2.0]),
        (257, [100.0; 3], [0.0, 0.0, -2.0]),
        (1, [0.001; 3], [-100.0, 0.0, -2.0]),
        (1, [0.001; 3], [0.0, 0.0, 2.0]),
    ] {
        let Some(mut ctx) = host() else {
            return;
        };
        let req = request(n, scale, mean);
        let cpu = CpuTileBuilder::build(&req, options).unwrap();
        let (gpu, metrics) = GpuTileBuilder::build(&mut ctx, &req, options).unwrap();
        assert_eq!(gpu.pairs, cpu.pairs);
        assert_eq!(gpu.inclusive_offsets, cpu.inclusive_offsets);
        gpu.validate().unwrap();
        assert!(metrics.accounted_peak_bound_bytes <= options.max_gpu_bytes);
        assert!(ctx.resource_accounting().peak_bytes <= metrics.accounted_peak_bound_bytes);
        assert_eq!(ctx.resource_accounting().current_bytes, 0);
        assert_eq!(ctx.retained_submission_count(), 0);
        eprintln!(
            "gpu_pressure sources={n} pairs={} scratch={} bound={} encode_ns={} count_sync_ns={} emit_sync_ns={}",
            gpu.pairs.len(),
            metrics.scan_scratch_bytes,
            metrics.accounted_peak_bound_bytes,
            metrics.projection_scan_encode_ns,
            metrics.count_sync_ns,
            metrics.emission_sync_ns
        );
    }
    let Some(mut ctx) = host() else {
        return;
    };
    let (empty, metrics) = GpuTileBuilder::build_empty(&mut ctx, 63, 47, options).unwrap();
    assert!(empty.pairs.is_empty());
    assert_eq!(metrics.dispatch_count, 0);
}
#[test]
fn gpu_preflight_failure_cleanup_timeout_and_pending_ownership() {
    let Some(mut ctx) = host() else {
        return;
    };
    let req = request(1, [100.0; 3], [0.0, 0.0, -2.0]);
    let options = TileBuildOptions::default();
    // First phase bound is 608; second phase is 864, but incremental 800.
    let err = GpuTileBuilder::build(
        &mut ctx,
        &req,
        TileBuildOptions {
            max_gpu_bytes: 820,
            ..options
        },
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::AllocationFailed);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);
    let err = GpuTileBuilder::build(
        &mut ctx,
        &req,
        TileBuildOptions {
            max_pairs: 47,
            ..options
        },
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);
    let token = CancelToken::new();
    token.request(CancelReason::User);
    assert_eq!(
        GpuTileBuilder::build_cancellable(&mut ctx, &req, options, &token)
            .unwrap_err()
            .code(),
        ErrorCode::Canceled
    );
    ctx.inject_next_wait_timeout();
    assert!(GpuTileBuilder::build(&mut ctx, &req, options).is_err());
    assert_eq!(ctx.retained_submission_count(), 1);
    ctx.wait_retained_submissions(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    assert_eq!(ctx.retained_submission_count(), 0);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);
    let owner = ctx
        .create_buffer(BufferDesc {
            label: ResourceLabel::parse("spirula:render:buffer:other-owner").unwrap(),
            size: 16,
            usage: BufferUsage::STORAGE_COPY,
        })
        .unwrap();
    ctx.upload_buffer(&owner, 0, &[7; 16]).unwrap();
    let before = ctx.pending_resource_counts();
    assert_eq!(
        GpuTileBuilder::build(&mut ctx, &req, options)
            .unwrap_err()
            .code(),
        ErrorCode::InvalidArgument
    );
    assert_eq!(ctx.pending_resource_counts(), before);
    let submission = ctx.submit().unwrap();
    ctx.wait(submission, None).unwrap();
    drop(owner);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);
}
#[test]
fn per_source_and_scan_signed_overflow_are_rejected_before_pair_allocation() {
    let Some(mut ctx) = host() else {
        return;
    };
    let options = TileBuildOptions {
        tile_size: 1,
        max_tiles: u64::MAX,
        max_pairs: i32::MAX as u64,
        ..TileBuildOptions::default()
    };
    for n in [1, 3] {
        let mut req = request(n, [1000.0; 3], [0.0, 0.0, -2.0]);
        req.camera = PinholeCamera::try_new(
            65535,
            65535,
            32.0,
            32.0,
            32767.5,
            32767.5,
            OpenCvDistortion::try_new(0.0, 0.0, 0.0, 0.0).unwrap(),
            CameraPose::identity(),
        )
        .unwrap();
        assert_eq!(
            CpuTileBuilder::build(&req, options).unwrap_err().code(),
            ErrorCode::ValueOutOfRange
        );
        assert_eq!(
            GpuTileBuilder::build(&mut ctx, &req, options)
                .unwrap_err()
                .code(),
            ErrorCode::ValueOutOfRange
        );
        assert_eq!(ctx.resource_accounting().current_bytes, 0);
        assert_eq!(ctx.retained_submission_count(), 0);
        assert!(
            ctx.resource_accounting().peak_bytes < 4096,
            "overflow allocated pairs before preflight"
        );
    }
}

#[test]
fn half_million_intersection_pressure_has_measured_accounted_bound() {
    let Some(mut ctx) = host() else {
        return;
    };
    let mut req = request(65, [100.0; 3], [0.0, 0.0, -2.0]);
    req.camera = PinholeCamera::try_new(
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
    let options = TileBuildOptions::default();
    let cpu = CpuTileBuilder::build(&req, options).unwrap();
    let (gpu, metrics) = GpuTileBuilder::build(&mut ctx, &req, options).unwrap();
    assert_eq!(gpu.pairs.len(), 487500);
    assert_eq!(gpu.pairs, cpu.pairs);
    assert_eq!(gpu.inclusive_offsets, cpu.inclusive_offsets);
    let peak = ctx.resource_accounting().peak_bytes;
    assert!(peak <= metrics.accounted_peak_bound_bytes);
    assert!(peak <= options.max_gpu_bytes);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);
    eprintln!(
        "gpu_large_pressure sources=65 viewport=800x600 pairs={} scratch={} bound={} observed_accounted_peak={peak} encode_ns={} count_sync_ns={} emit_sync_ns={}",
        metrics.intersection_peak,
        metrics.scan_scratch_bytes,
        metrics.accounted_peak_bound_bytes,
        metrics.projection_scan_encode_ns,
        metrics.count_sync_ns,
        metrics.emission_sync_ns
    );
    let before = ctx.resource_accounting().allocated_bytes;
    assert_eq!(
        GpuTileBuilder::build(
            &mut ctx,
            &req,
            TileBuildOptions {
                max_pairs: 487499,
                ..options
            }
        )
        .unwrap_err()
        .code(),
        ErrorCode::ValueOutOfRange
    );
    assert!(
        ctx.resource_accounting().allocated_bytes - before < 100000,
        "budget rejection allocated pair buffer"
    );
    assert_eq!(ctx.resource_accounting().current_bytes, 0);
}

#[test]
fn supported_tile_sizes_and_execution_capabilities_are_explicit() {
    let Some(mut ctx) = host() else {
        return;
    };
    let mut req = request(3, [0.2, 0.07, 0.03], [0.0, 0.0, -2.0]);
    req.camera = PinholeCamera::try_new(
        63,
        47,
        32.0,
        32.0,
        31.5,
        23.5,
        OpenCvDistortion::try_new(0.0, 0.0, 0.0, 0.0).unwrap(),
        CameraPose::identity(),
    )
    .unwrap();
    for tile_size in [1, 2, 4, 8, 16, 32, 64, 128] {
        let options = TileBuildOptions {
            tile_size,
            ..TileBuildOptions::default()
        };
        let cpu = CpuTileBuilder::build(&req, options).unwrap();
        let (gpu, _) = GpuTileBuilder::build(&mut ctx, &req, options).unwrap();
        assert_eq!(gpu.pairs, cpu.pairs);
        assert_eq!(gpu.inclusive_offsets, cpu.inclusive_offsets);
    }
    for tile_size in [0, 3, 256] {
        let options = TileBuildOptions {
            tile_size,
            ..TileBuildOptions::default()
        };
        assert_eq!(
            CpuTileBuilder::build(&req, options).unwrap_err().code(),
            ErrorCode::ValueOutOfRange
        );
        assert_eq!(
            GpuTileBuilder::build(&mut ctx, &req, options)
                .unwrap_err()
                .code(),
            ErrorCode::ValueOutOfRange
        );
    }
    req.thresholds.alpha_cutoff = 0.0;
    assert_eq!(
        GpuTileBuilder::build(&mut ctx, &req, TileBuildOptions::default())
            .unwrap_err()
            .code(),
        ErrorCode::ValueOutOfRange
    );
    assert!(CpuTileBuilder::build(&req, TileBuildOptions::default()).is_err());
    req.thresholds = RenderThresholds::default();
    req.camera = PinholeCamera::try_new(
        63,
        47,
        32.0,
        32.0,
        31.5,
        23.5,
        OpenCvDistortion::try_new(0.01, 0.0, 0.0, 0.0).unwrap(),
        CameraPose::identity(),
    )
    .unwrap();
    assert_eq!(
        CpuTileBuilder::build(&req, TileBuildOptions::default())
            .unwrap_err()
            .code(),
        ErrorCode::UnimplementedOption
    );
    assert_eq!(
        GpuTileBuilder::build(&mut ctx, &req, TileBuildOptions::default())
            .unwrap_err()
            .code(),
        ErrorCode::UnimplementedOption
    );
    let extreme = request(1, [1e150; 3], [0.0, 0.0, -2.0]);
    assert_eq!(
        GpuTileBuilder::build(&mut ctx, &extreme, TileBuildOptions::default())
            .unwrap_err()
            .code(),
        ErrorCode::ValueOutOfRange
    );
    assert_eq!(ctx.resource_accounting().current_bytes, 0);
    ctx.inject_lost("renderer lost regression");
    assert_eq!(
        GpuTileBuilder::build(
            &mut ctx,
            &self::request(1, [0.1; 3], [0.0, 0.0, -2.0]),
            TileBuildOptions::default()
        )
        .unwrap_err()
        .code(),
        ErrorCode::DeviceLost
    );
    assert_eq!(
        GpuTileBuilder::build_empty(&mut ctx, 63, 47, TileBuildOptions::default())
            .unwrap_err()
            .code(),
        ErrorCode::DeviceLost
    );
    let Some(mut ctx) = host() else {
        return;
    };
    ctx.close();
    assert_eq!(
        GpuTileBuilder::build(
            &mut ctx,
            &self::request(1, [0.1; 3], [0.0, 0.0, -2.0]),
            TileBuildOptions::default()
        )
        .unwrap_err()
        .code(),
        ErrorCode::DeviceLost
    );
    assert_eq!(
        GpuTileBuilder::build_empty(&mut ctx, 63, 47, TileBuildOptions::default())
            .unwrap_err()
            .code(),
        ErrorCode::DeviceLost
    );
}
