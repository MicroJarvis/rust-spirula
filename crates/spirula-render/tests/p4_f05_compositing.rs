//! P4-F05 focused acceptance (not the parent-owned independent review suite).
use spirula_core::{
    CameraPose, CancelReason, CancelToken, ErrorCode, GaussianSplat, OpenCvDistortion,
    PinholeCamera, Quat, SplatSet, Vec3,
};
use spirula_gpu::{
    BufferDesc, BufferUsage, CapabilityRequest, ComputePipelineDesc, GpuContext, ResourceLabel,
    ScopeErrorInject, SpecConstants,
};
use spirula_render::{
    BackgroundPolicy, ColorSpace, CompositeOptions, CpuCompositor, GpuCompositor, RenderPrimitive,
    RenderRequest, RenderThresholds, TileBuildOptions,
};
use std::time::Instant;

const COMP_WGSL: &str = include_str!("../shaders/compositing.wgsl");
const TILE_WGSL: &str = include_str!("../shaders/projection-tiles.wgsl");

fn host() -> Option<GpuContext> {
    match GpuContext::open(&CapabilityRequest::diagnose_default()) {
        Ok(ctx) => Some(ctx),
        Err(err) => {
            assert!(
                std::env::var_os("SPIRULA_REQUIRE_RENDER_GPU").is_none(),
                "required GPU unavailable: {err:?}"
            );
            eprintln!("compositing GPU test skipped: {err:?}");
            None
        }
    }
}

fn label(name: &str) -> ResourceLabel {
    ResourceLabel::parse(name).unwrap()
}

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

fn culled_p0_request(n: usize) -> RenderRequest {
    let camera = PinholeCamera::try_new(
        1,
        1,
        1.0,
        1.0,
        0.5,
        0.5,
        OpenCvDistortion::try_new(0.0, 0.0, 0.0, 0.0).unwrap(),
        CameraPose::identity(),
    )
    .unwrap();
    let splats = (0..n)
        .map(|i| {
            GaussianSplat::try_new(
                // z>0 → axial depth = -z ≤ 0 → fully culled (P=0).
                Vec3::try_new(0.0, 0.0, 2.0 + i as f64 * 0.01).unwrap(),
                Quat::identity(),
                Vec3::try_new(0.0, 0.0, 0.0).unwrap(),
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
fn median_before_earlyout_keeps_median_when_t_stays_one() {
    let camera = PinholeCamera::try_new(
        1,
        1,
        1.0,
        1.0,
        0.5,
        0.5,
        OpenCvDistortion::try_new(0.0, 0.0, 0.0, 0.0).unwrap(),
        CameraPose::identity(),
    )
    .unwrap();
    let splat = GaussianSplat::try_new(
        Vec3::try_new(0.0, 0.0, -4.0).unwrap(),
        Quat::try_from_wxyz(1.0, 0.0, 0.0, 0.0).unwrap(),
        Vec3::try_new(-30.0, -30.0, -30.0).unwrap(),
        3.0_f64.ln(),
        Vec3::try_new(0.0, 0.0, 0.0).unwrap(),
    )
    .unwrap();
    let request = RenderRequest::try_new(
        camera,
        SplatSet::try_from_splats(vec![splat]).unwrap(),
        RenderPrimitive::Gaussian3d,
        ColorSpace::LinearRgb,
        0,
        RenderThresholds {
            alpha_cutoff: 1e-4,
            early_out_transmittance: 0.5,
        },
    )
    .unwrap();
    let img = CpuCompositor::compose(&request, CompositeOptions::default()).unwrap();
    let p = img.pixels[0];
    assert_eq!(p.transmittance, 1.0);
    assert_eq!(p.alpha, 0.0);
    assert_eq!(p.expected_depth, 0.0);
    assert!(
        (p.median_depth - 2.0).abs() < 1e-9,
        "median={}",
        p.median_depth
    );
}

#[test]
fn cpu_gpu_empty_clear_writes_background() {
    let options = CompositeOptions {
        background: BackgroundPolicy::Constant([0.1, 0.2, 0.3]),
        ..CompositeOptions::default()
    };
    let cpu = CpuCompositor::compose_empty(3, 2, 1e-4, 1e-4, options).unwrap();
    assert_eq!(cpu.pixels.len(), 6);
    assert_eq!(cpu.pixels[0].rgb, [0.1, 0.2, 0.3]);
    assert_eq!(cpu.pixels[0].transmittance, 1.0);

    let Some(mut ctx) = host() else {
        return;
    };
    let (gpu, metrics) = GpuCompositor::compose_empty(&mut ctx, 3, 2, 1e-4, 1e-4, options).unwrap();
    assert_eq!(gpu.pixels.len(), 6);
    for (c, g) in cpu.pixels.iter().zip(&gpu.pixels) {
        assert!((c.rgb[0] - g.rgb[0]).abs() < 1e-6);
        assert_eq!(g.transmittance, 1.0);
        assert_eq!(g.expected_depth, 0.0);
        assert_eq!(g.median_depth, 0.0);
    }
    assert!(
        metrics.dispatch_count >= 1,
        "GPU clear must dispatch composite"
    );
    assert_eq!(ctx.resource_accounting().current_bytes, 0);
    assert_eq!(ctx.retained_submission_count(), 0);
}

#[test]
fn pressure_chain_exact_pairs_pixels_alpha_and_releases() {
    let Some(mut ctx) = host() else {
        return;
    };
    let request = pressure_request();
    let options = CompositeOptions::default();
    let start = Instant::now();
    let cpu = CpuCompositor::compose(&request, options).unwrap();
    let cpu_ns = start.elapsed().as_nanos();
    let start = Instant::now();
    let (gpu, metrics) = GpuCompositor::compose(&mut ctx, &request, options).unwrap();
    let gpu_ns = start.elapsed().as_nanos();

    assert_eq!(metrics.pair_count, 487_500);
    assert_eq!(metrics.pixel_count, 480_000);
    assert_eq!(cpu.pixels.len(), 480_000);
    assert_eq!(gpu.pixels.len(), 480_000);
    assert_eq!(cpu.alpha_cutoff, request.thresholds.alpha_cutoff);
    assert_eq!(gpu.alpha_cutoff, cpu.alpha_cutoff);
    assert_eq!(gpu.effective_alpha_cutoff, cpu.effective_alpha_cutoff);
    assert_eq!(gpu.early_out_transmittance, cpu.early_out_transmittance);

    let mut max_err = 0.0_f64;
    for (i, (c, g)) in cpu.pixels.iter().zip(&gpu.pixels).enumerate() {
        for k in 0..3 {
            max_err = max_err.max((c.rgb[k] - g.rgb[k]).abs());
        }
        max_err = max_err
            .max((c.transmittance - g.transmittance).abs())
            .max((c.alpha - g.alpha).abs())
            .max((c.expected_depth - g.expected_depth).abs())
            .max((c.median_depth - g.median_depth).abs());
        assert!(
            max_err < 2e-4,
            "pixel={i} max_err={max_err} cpu_T={} gpu_T={}",
            c.transmittance,
            g.transmittance
        );
    }

    let peak = ctx.resource_accounting().peak_bytes;
    assert!(peak <= metrics.accounted_peak_bound_bytes);
    assert!(peak <= options.max_gpu_bytes);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);
    assert_eq!(ctx.retained_submission_count(), 0);
    let pending = ctx.pending_resource_counts();
    assert_eq!(pending.commands, 0);
    assert_eq!(pending.stagings, 0);
    assert_eq!(pending.buffers, 0);
    assert_eq!(pending.textures, 0);

    // Early-out truncates FTB contributions; F03/F04 still process the full
    // 487500-pair workload before compositing.
    eprintln!(
        "p4_f05_pressure pairs={} pixels={} host_peak={} accounted_peak_bound={} observed_accounted_peak={peak} ({:.2} MiB) tile={} sort={} comp={} encode_ns={} sync_ns={} cpu_ns={} gpu_ns={} adapter={} max_err={max_err} (earlyout may truncate FTB layers; F03/F04 still full pairs)",
        metrics.pair_count,
        metrics.pixel_count,
        metrics.host_peak_bound_bytes,
        metrics.accounted_peak_bound_bytes,
        peak as f64 / (1024.0 * 1024.0),
        metrics.tile_accounted_peak_bytes,
        metrics.sort_accounted_peak_bytes,
        metrics.composite_accounted_peak_bytes,
        metrics.composite_encode_ns,
        metrics.composite_sync_ns,
        cpu_ns,
        gpu_ns,
        metrics.adapter
    );
}

#[test]
fn many_culled_sources_p0_1x1_and_tight_host_cap() {
    let request = culled_p0_request(64);
    let options = CompositeOptions {
        max_pairs: 8, // N=64 > pair budget; must still succeed when P=0
        ..CompositeOptions::default()
    };
    let cpu = CpuCompositor::compose(&request, options).unwrap();
    assert_eq!(cpu.pixels.len(), 1);
    assert_eq!(cpu.pixels[0].transmittance, 1.0);
    assert_eq!(cpu.pixels[0].expected_depth, 0.0);
    assert_eq!(cpu.pixels[0].median_depth, 0.0);

    let Some(mut ctx) = host() else {
        return;
    };
    let before = ctx.resource_accounting().allocated_bytes;
    let (gpu, metrics) = GpuCompositor::compose(&mut ctx, &request, options).unwrap();
    assert_eq!(metrics.pair_count, 0);
    assert_eq!(metrics.pixel_count, 1);
    assert_eq!(gpu.pixels[0].transmittance, 1.0);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);

    // Tight host cap rejects at entry preflight — before GPU allocations.
    let tight = CompositeOptions {
        max_host_bytes: 64,
        ..CompositeOptions::default()
    };
    let allocated_before = ctx.resource_accounting().allocated_bytes;
    let err = GpuCompositor::compose(&mut ctx, &request, tight).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
    assert_eq!(ctx.resource_accounting().allocated_bytes, allocated_before);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);
    let _ = before;
}

#[test]
fn dispatch_compositing_family_layout_alias_cross_context_and_range() {
    let Some(mut ctx) = host() else {
        return;
    };
    let params = ctx
        .create_buffer(BufferDesc {
            label: label("spirula:render:buffer:c-params"),
            size: 16,
            usage: BufferUsage::UNIFORM_UPLOAD,
        })
        .unwrap();
    let inn = ctx
        .create_buffer(BufferDesc {
            label: label("spirula:render:buffer:c-in"),
            size: 256,
            usage: BufferUsage::STORAGE_COPY,
        })
        .unwrap();
    let out = ctx
        .create_buffer(BufferDesc {
            label: label("spirula:render:buffer:c-out"),
            size: 256,
            usage: BufferUsage::STORAGE_COPY,
        })
        .unwrap();
    ctx.upload_buffer(&params, 0, &[0u8; 16]).unwrap();
    ctx.upload_buffer(&inn, 0, &[0u8; 256]).unwrap();

    let composite = ctx
        .obtain_compute_pipeline(ComputePipelineDesc {
            source: COMP_WGSL.to_owned(),
            entry_point: "composite".into(),
            spec: SpecConstants::empty(),
            bind_layout_version: 1,
            label: label("spirula:render:pipeline:c-composite"),
        })
        .unwrap();
    ctx.dispatch_compositing(&composite, &params, &inn, &out, 256, 256, 64)
        .unwrap();

    let tiles = ctx
        .obtain_compute_pipeline(ComputePipelineDesc {
            source: TILE_WGSL.to_owned(),
            entry_point: "project".into(),
            spec: SpecConstants::empty(),
            bind_layout_version: 1,
            label: label("spirula:render:pipeline:c-wrong-family"),
        })
        .unwrap();
    assert_eq!(
        ctx.dispatch_compositing(&tiles, &params, &inn, &out, 256, 256, 64)
            .unwrap_err()
            .code(),
        ErrorCode::InvalidArgument
    );

    let bad_layout = ctx
        .obtain_compute_pipeline(ComputePipelineDesc {
            source: COMP_WGSL.to_owned(),
            entry_point: "composite".into(),
            spec: SpecConstants::empty(),
            bind_layout_version: 2,
            label: label("spirula:render:pipeline:c-bad-layout"),
        })
        .unwrap();
    assert_eq!(
        ctx.dispatch_compositing(&bad_layout, &params, &inn, &out, 256, 256, 64)
            .unwrap_err()
            .code(),
        ErrorCode::InvalidArgument
    );

    let v2_src = COMP_WGSL.replacen("version: 1", "version: 2", 1);
    let bad_version = ctx
        .obtain_compute_pipeline(ComputePipelineDesc {
            source: v2_src,
            entry_point: "composite".into(),
            spec: SpecConstants::empty(),
            bind_layout_version: 1,
            label: label("spirula:render:pipeline:c-bad-version"),
        })
        .unwrap();
    assert_eq!(
        ctx.dispatch_compositing(&bad_version, &params, &inn, &out, 256, 256, 64)
            .unwrap_err()
            .code(),
        ErrorCode::InvalidArgument
    );

    let other_src = COMP_WGSL.replace("fn composite(", "fn other_composite(");
    let bad_entry = ctx
        .obtain_compute_pipeline(ComputePipelineDesc {
            source: other_src,
            entry_point: "other_composite".into(),
            spec: SpecConstants::empty(),
            bind_layout_version: 1,
            label: label("spirula:render:pipeline:c-bad-entry"),
        })
        .unwrap();
    assert_eq!(
        ctx.dispatch_compositing(&bad_entry, &params, &inn, &out, 256, 256, 64)
            .unwrap_err()
            .code(),
        ErrorCode::InvalidArgument
    );

    assert_eq!(
        ctx.dispatch_compositing(&composite, &params, &inn, &inn, 256, 256, 64)
            .unwrap_err()
            .code(),
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        ctx.dispatch_compositing(&composite, &params, &inn, &out, 0, 256, 64)
            .unwrap_err()
            .code(),
        ErrorCode::ValueOutOfRange
    );
    assert_eq!(
        ctx.dispatch_compositing(&composite, &params, &inn, &out, 512, 256, 64)
            .unwrap_err()
            .code(),
        ErrorCode::ValueOutOfRange
    );
    assert_eq!(
        ctx.dispatch_compositing(&composite, &params, &inn, &out, 256, 256, 0)
            .unwrap_err()
            .code(),
        ErrorCode::ValueOutOfRange
    );

    let Some(mut other) = host() else {
        return;
    };
    assert_eq!(
        other
            .dispatch_compositing(&composite, &params, &inn, &out, 256, 256, 64)
            .unwrap_err()
            .code(),
        ErrorCode::InvalidArgument
    );

    let submission = ctx.submit().unwrap();
    ctx.wait(submission, None).unwrap();

    let params_closed = params;
    let inn_closed = inn;
    let out_closed = out;
    let composite_closed = composite;
    ctx.close();
    assert_eq!(
        ctx.dispatch_compositing(
            &composite_closed,
            &params_closed,
            &inn_closed,
            &out_closed,
            256,
            256,
            64
        )
        .unwrap_err()
        .code(),
        ErrorCode::DeviceLost
    );
    drop(params_closed);
    drop(inn_closed);
    drop(out_closed);
    drop(composite_closed);
    drop(tiles);
    drop(bad_layout);
    drop(bad_version);
    drop(bad_entry);
    drop(other);

    let Some(mut live) = host() else {
        return;
    };
    let params2 = live
        .create_buffer(BufferDesc {
            label: label("spirula:render:buffer:c-params2"),
            size: 16,
            usage: BufferUsage::UNIFORM_UPLOAD,
        })
        .unwrap();
    let inn2 = live
        .create_buffer(BufferDesc {
            label: label("spirula:render:buffer:c-in2"),
            size: 64,
            usage: BufferUsage::STORAGE_COPY,
        })
        .unwrap();
    let out2 = live
        .create_buffer(BufferDesc {
            label: label("spirula:render:buffer:c-out2"),
            size: 64,
            usage: BufferUsage::STORAGE_COPY,
        })
        .unwrap();
    let composite2 = live
        .obtain_compute_pipeline(ComputePipelineDesc {
            source: COMP_WGSL.to_owned(),
            entry_point: "composite".into(),
            spec: SpecConstants::empty(),
            bind_layout_version: 1,
            label: label("spirula:render:pipeline:c-composite2"),
        })
        .unwrap();
    let gen_before = live.generation();
    live.reopen(&CapabilityRequest::diagnose_default())
        .expect("host reopen");
    assert_ne!(live.generation(), gen_before);
    assert_eq!(live.status(), spirula_gpu::ContextStatus::Live);
    assert_eq!(
        live.dispatch_compositing(&composite2, &params2, &inn2, &out2, 64, 64, 16)
            .unwrap_err()
            .code(),
        ErrorCode::DeviceLost
    );
    drop(params2);
    drop(inn2);
    drop(out2);
    drop(composite2);
    assert_eq!(live.resource_accounting().current_bytes, 0);
}

#[test]
fn compose_empty_path_oom_timeout_pending_cancel_lost_closed() {
    // compose_empty: build_empty is host-only (0 GPU submits). First inject
    // scope / wait therefore lands on the NEW F05 composite stage — not F03.
    let Some(mut ctx) = host() else {
        return;
    };
    let options = CompositeOptions::default();

    ctx.inject_next_scope_error(ScopeErrorInject::OutOfMemory {
        requested_bytes: Some(4096),
        current_bytes: Some(0),
        budget_bytes: Some(1024),
    });
    let oom = GpuCompositor::compose_empty(&mut ctx, 8, 8, 1e-4, 1e-4, options).unwrap_err();
    assert_eq!(oom.code(), ErrorCode::AllocationFailed);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);
    assert_eq!(ctx.retained_submission_count(), 0);

    ctx.inject_next_wait_timeout();
    let timeout = GpuCompositor::compose_empty(&mut ctx, 8, 8, 1e-4, 1e-4, options).unwrap_err();
    assert_eq!(timeout.code(), ErrorCode::SubmitFailed);
    assert!(
        timeout.message().contains("timed out"),
        "timeout message={timeout}"
    );
    assert_eq!(ctx.retained_submission_count(), 1);
    ctx.wait_retained_submissions(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    assert_eq!(ctx.retained_submission_count(), 0);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);

    let token = CancelToken::new();
    token.request(CancelReason::User);
    assert_eq!(
        GpuCompositor::compose_empty_cancellable(&mut ctx, 8, 8, 1e-4, 1e-4, options, &token)
            .unwrap_err()
            .code(),
        ErrorCode::Canceled
    );
    assert_eq!(ctx.resource_accounting().current_bytes, 0);

    let owner = ctx
        .create_buffer(BufferDesc {
            label: label("spirula:render:buffer:other-owner"),
            size: 16,
            usage: BufferUsage::STORAGE_COPY,
        })
        .unwrap();
    ctx.upload_buffer(&owner, 0, &[9; 16]).unwrap();
    let before = ctx.pending_resource_counts();
    assert_eq!(
        GpuCompositor::compose_empty(&mut ctx, 8, 8, 1e-4, 1e-4, options)
            .unwrap_err()
            .code(),
        ErrorCode::InvalidArgument
    );
    assert_eq!(ctx.pending_resource_counts(), before);
    let submission = ctx.submit().unwrap();
    ctx.wait(submission, None).unwrap();
    drop(owner);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);

    let Some(mut lost) = host() else {
        return;
    };
    lost.inject_lost("p4-f05 compositing deterministic lost");
    assert_eq!(
        GpuCompositor::compose_empty(&mut lost, 8, 8, 1e-4, 1e-4, options)
            .unwrap_err()
            .code(),
        ErrorCode::DeviceLost
    );

    let Some(mut closed) = host() else {
        return;
    };
    closed.close();
    assert_eq!(
        GpuCompositor::compose_empty(&mut closed, 8, 8, 1e-4, 1e-4, options)
            .unwrap_err()
            .code(),
        ErrorCode::DeviceLost
    );
}

#[test]
fn unsupported_tile_size_typed_on_cpu_and_gpu_empty() {
    for tile_size in [0u32, 3, 256] {
        let options = CompositeOptions {
            tile: TileBuildOptions {
                tile_size,
                ..TileBuildOptions::default()
            },
            ..CompositeOptions::default()
        };
        let err = CpuCompositor::compose_empty(8, 8, 1e-4, 1e-4, options).unwrap_err();
        assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
        if let Some(mut ctx) = host() {
            let err =
                GpuCompositor::compose_empty(&mut ctx, 8, 8, 1e-4, 1e-4, options).unwrap_err();
            assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
        }
    }
}
