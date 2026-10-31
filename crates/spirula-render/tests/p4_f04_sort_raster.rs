//! P4-F04 focused acceptance: stable GPU radix, tile ranges, conic coverage.
use spirula_core::{
    CameraPose, CancelReason, CancelToken, ErrorCode, GaussianSplat, OpenCvDistortion,
    PinholeCamera, Quat, SplatSet, Vec3,
};
use spirula_gpu::{
    BufferDesc, BufferUsage, CapabilityRequest, ComputePipelineDesc, GpuContext, ResourceLabel,
    ScopeErrorInject, SpecConstants,
};
use spirula_render::{
    ColorSpace, CpuSortRaster, CpuTileBuilder, GpuSortRaster, GpuTileBuilder, NO_SOURCE,
    ProjectedTileSplat, RenderPrimitive, RenderRequest, RenderThresholds, SORT_RASTER_VERSION,
    SortRasterOptions, TILE_CONTRACT_VERSION, TileBins, TileBuildOptions, TileCoverage, TilePair,
    canonical_depth_key,
};
use std::time::Instant;

const SORT_WGSL: &str = include_str!("../shaders/sort-raster.wgsl");
const TILE_WGSL: &str = include_str!("../shaders/projection-tiles.wgsl");

fn host() -> Option<GpuContext> {
    match GpuContext::open(&CapabilityRequest::diagnose_default()) {
        Ok(ctx) => Some(ctx),
        Err(err) => {
            assert!(
                std::env::var_os("SPIRULA_REQUIRE_RENDER_GPU").is_none(),
                "required GPU unavailable: {err:?}"
            );
            eprintln!("sort_raster GPU test skipped: {err:?}");
            None
        }
    }
}

fn label(name: &str) -> ResourceLabel {
    ResourceLabel::parse(name).unwrap()
}

fn full_tile_fixture(width: u32, height: u32, depths: &[f64]) -> TileBins {
    let tile_size = 8u32;
    let tiles_x = width.div_ceil(tile_size);
    let tiles_y = height.div_ceil(tile_size);
    let tile_count = (tiles_x * tiles_y) as usize;
    let projected = depths
        .iter()
        .enumerate()
        .map(|(source_index, &depth)| {
            Some(ProjectedTileSplat {
                source_index,
                pixel: [0.5, 0.5],
                depth,
                ray_depth: depth,
                radius_px: [64.0, 64.0],
                covariance: [2.0, 0.5, 1.0],
                opacity: 0.8,
                tile_bounds: [0, 0, tiles_x, tiles_y],
            })
        })
        .collect();
    let bins = TileBins {
        contract_version: TILE_CONTRACT_VERSION,
        projection_alpha_cutoff: 1e-4,
        width,
        height,
        tile_size,
        tiles_x,
        tiles_y,
        projected,
        coverage: (0..depths.len())
            .map(|source_index| TileCoverage {
                source_index,
                pair_offset: source_index * tile_count,
                pair_count: tile_count,
            })
            .collect(),
        pairs: (0..depths.len())
            .flat_map(|source_index| {
                (0..tile_count).map(move |tile_id| TilePair {
                    source_index,
                    tile_id,
                })
            })
            .collect(),
        inclusive_offsets: (1..=depths.len())
            .map(|n| (n * tile_count) as u32)
            .collect(),
    };
    bins.validate().unwrap();
    bins
}

fn p4_f03_pressure_request() -> RenderRequest {
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

fn assert_cpu_gpu_match(cpu: &spirula_render::SortedRaster, gpu: &spirula_render::SortedRaster) {
    assert_eq!(cpu.alpha_cutoff, gpu.alpha_cutoff);
    assert_eq!(cpu.effective_alpha_cutoff, gpu.effective_alpha_cutoff);
    assert_eq!(cpu.pairs, gpu.pairs);
    assert_eq!(cpu.tile_ranges, gpu.tile_ranges);
    assert_eq!(cpu.pixels.len(), gpu.pixels.len());
    for (i, (c, g)) in cpu.pixels.iter().zip(&gpu.pixels).enumerate() {
        assert_eq!(c.count, g.count, "pixel={i}");
        assert_eq!(c.first_source, g.first_source, "pixel={i}");
        assert!(
            (c.max_alpha - g.max_alpha).abs() < 2e-5,
            "pixel={i} cpu={} gpu={}",
            c.max_alpha,
            g.max_alpha
        );
    }
}

#[test]
fn empty_non_block_duplicate_and_f32_tie_paths_match() {
    let options = SortRasterOptions::default();
    let empty = TileBins::empty(17, 9, TileBuildOptions::default()).unwrap();
    assert_eq!(empty.projection_alpha_cutoff, 0.0);
    let cpu_empty = CpuSortRaster::build(&empty, 0.01, options).unwrap();
    assert!(cpu_empty.pairs.is_empty());
    assert_eq!(cpu_empty.contract_version, SORT_RASTER_VERSION);
    assert_eq!(cpu_empty.alpha_cutoff, 0.01);
    assert_eq!(cpu_empty.effective_alpha_cutoff, 0.01_f64 as f32);
    assert!(
        cpu_empty
            .pixels
            .iter()
            .all(|p| p.count == 0 && p.first_source == NO_SOURCE)
    );

    let mut depths: Vec<f64> = (0..65).map(|i| ((i % 7) + 1) as f64).collect();
    depths[3] = 2.0 + 1e-9;
    depths[4] = 2.0;
    let bins = full_tile_fixture(17, 9, &depths);
    let cpu = CpuSortRaster::build(&bins, 0.01, options).unwrap();
    for range in &cpu.tile_ranges {
        let pairs = &cpu.pairs[range.start as usize..(range.start + range.count) as usize];
        for window in pairs.windows(2) {
            let a = (
                window[0].tile_id,
                window[0].depth_key,
                window[0].input_index,
            );
            let b = (
                window[1].tile_id,
                window[1].depth_key,
                window[1].input_index,
            );
            assert!(a <= b);
        }
        let tie: Vec<_> = pairs
            .iter()
            .filter(|p| p.source_index == 3 || p.source_index == 4)
            .map(|p| p.source_index)
            .collect();
        if tie.len() == 2 {
            assert_eq!(tie, [3, 4]);
        }
    }
    assert_eq!(
        canonical_depth_key(2.0 + 1e-9).unwrap(),
        canonical_depth_key(2.0).unwrap()
    );

    let Some(mut ctx) = host() else {
        return;
    };
    let (gpu_empty, empty_metrics) = GpuSortRaster::build(&mut ctx, &empty, 0.01, options).unwrap();
    assert_eq!(gpu_empty.pairs, cpu_empty.pairs);
    assert_eq!(gpu_empty.alpha_cutoff, cpu_empty.alpha_cutoff);
    assert_eq!(
        gpu_empty.effective_alpha_cutoff,
        cpu_empty.effective_alpha_cutoff
    );
    assert_eq!(empty_metrics.dispatch_count, 0);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);

    let (gpu, metrics) = GpuSortRaster::build(&mut ctx, &bins, 0.01, options).unwrap();
    assert_cpu_gpu_match(&cpu, &gpu);
    assert_eq!(metrics.dispatch_count, 51);
    assert!(ctx.resource_accounting().peak_bytes <= metrics.accounted_peak_bound_bytes);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);
    assert_eq!(ctx.retained_submission_count(), 0);
    eprintln!(
        "sort_raster_non_block sources=65 pairs={} tests={} scratch={} ranges={} raster_out={} bound={} peak={} encode_ns={} sort_sync_ns={} raster_sync_ns={} adapter={}",
        metrics.pair_count,
        metrics.raster_tests,
        metrics.radix_scratch_bytes,
        metrics.tile_range_bytes,
        metrics.raster_output_bytes,
        metrics.accounted_peak_bound_bytes,
        ctx.resource_accounting().peak_bytes,
        metrics.sort_encode_ns,
        metrics.sort_sync_ns,
        metrics.raster_sync_ns,
        metrics.adapter
    );
}

#[test]
fn partial_tile_last_pixel_has_positive_coverage_cpu_gpu() {
    // 19x11 → last pixel (18,10) sits in a partial 3x3 tile. Place the mean on
    // that pixel center so coverage is forced, not inferred from the first pixel.
    let options = SortRasterOptions::default();
    let mut bins = full_tile_fixture(19, 11, &[1.0]);
    let projected = bins.projected[0].as_mut().unwrap();
    projected.pixel = [18.5, 10.5];
    projected.covariance = [1.0, 0.0, 1.0];
    projected.opacity = 0.8;
    bins.validate().unwrap();

    let cpu = CpuSortRaster::build(&bins, 0.01, options).unwrap();
    let last = cpu.pixels.len() - 1;
    assert_eq!(last, 19 * 11 - 1);
    assert_eq!(cpu.pixels[last].count, 1);
    assert_eq!(cpu.pixels[last].first_source, 0);
    assert!((cpu.pixels[last].max_alpha - 0.8).abs() < 1e-12);
    // First pixel is far from the mean; must not be the only proof of coverage.
    assert_eq!(cpu.pixels[0].count, 0);

    let Some(mut ctx) = host() else {
        return;
    };
    let (gpu, metrics) = GpuSortRaster::build(&mut ctx, &bins, 0.01, options).unwrap();
    assert_cpu_gpu_match(&cpu, &gpu);
    assert_eq!(gpu.pixels[last].count, 1);
    assert_eq!(gpu.pixels[last].first_source, 0);
    assert!((gpu.pixels[last].max_alpha - 0.8).abs() < 2e-5);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);
    eprintln!(
        "sort_raster_partial_tail last_alpha={} tests={} bound={} peak={}",
        gpu.pixels[last].max_alpha,
        metrics.raster_tests,
        metrics.accounted_peak_bound_bytes,
        ctx.resource_accounting().peak_bytes
    );
}

#[test]
fn half_million_pair_pressure_matches_cpu_and_releases() {
    // Reuse the P4-F03 800x600 / 65 fullscreen footprint → 487500 pairs.
    // Each pair covers an 8x8 tile → 487500 * 64 = 31_200_000 raster tests.
    // CPU reference: CpuTileBuilder → CpuSortRaster (single sort).
    // GPU chain: GpuTileBuilder → GpuSortRaster(gpu_tiles) (single sort; no
    // second sort on CPU bins).
    let Some(mut ctx) = host() else {
        return;
    };
    let req = p4_f03_pressure_request();
    let tile_options = TileBuildOptions::default();
    let cpu_bins = CpuTileBuilder::build(&req, tile_options).unwrap();
    assert_eq!(cpu_bins.pairs.len(), 487_500);
    assert_eq!(cpu_bins.width, 800);
    assert_eq!(cpu_bins.height, 600);
    // Exact f32 cutoff above projection 1e-4; still not a global count guarantee
    // because conic/exp f32 rounding can move alphas across the threshold.
    let cutoff = 1.0 / 256.0;
    assert_eq!(cutoff as f32 as f64, cutoff);
    assert!(cutoff >= cpu_bins.projection_alpha_cutoff);
    let options = SortRasterOptions::default();

    let cpu_started = Instant::now();
    let cpu = CpuSortRaster::build(&cpu_bins, cutoff, options).unwrap();
    let cpu_ns = cpu_started.elapsed().as_nanos();
    assert_eq!(cpu.pairs.len(), 487_500);
    assert_eq!(
        cpu.tile_ranges.iter().map(|r| r.count as u64).sum::<u64>(),
        487_500
    );

    let (gpu_tiles, _) = GpuTileBuilder::build(&mut ctx, &req, tile_options).unwrap();
    assert_eq!(gpu_tiles.pairs, cpu_bins.pairs);
    assert_eq!(gpu_tiles.inclusive_offsets, cpu_bins.inclusive_offsets);
    assert_eq!(gpu_tiles.pairs.len(), 487_500);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);

    let (gpu, metrics) = GpuSortRaster::build(&mut ctx, &gpu_tiles, cutoff, options).unwrap();
    assert_eq!(metrics.pair_count, 487_500);
    assert_eq!(metrics.raster_tests, 31_200_000);
    assert_eq!(metrics.dispatch_count, 51);
    assert_cpu_gpu_match(&cpu, &gpu);
    let peak = ctx.resource_accounting().peak_bytes;
    assert!(peak <= metrics.accounted_peak_bound_bytes);
    assert!(peak <= options.max_gpu_bytes);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);
    assert_eq!(ctx.retained_submission_count(), 0);
    eprintln!(
        "sort_raster_pressure sources=65 viewport=800x600 pairs={} tests={} scratch={} ranges={} raster_out={} bound={} observed_accounted_peak={peak} cpu_build_ns={cpu_ns} encode_ns={} sort_sync_ns={} raster_sync_ns={} adapter={} (GPU F03→F04 chained on gpu_tiles; accounted buffers+staging only; not RSS/VRAM/driver/pipeline-cache)",
        metrics.pair_count,
        metrics.raster_tests,
        metrics.radix_scratch_bytes,
        metrics.tile_range_bytes,
        metrics.raster_output_bytes,
        metrics.accounted_peak_bound_bytes,
        metrics.sort_encode_ns,
        metrics.sort_sync_ns,
        metrics.raster_sync_ns,
        metrics.adapter
    );
}

#[test]
fn f32_pack_rejects_before_allocation_and_typed_failures() {
    let options = SortRasterOptions::default();
    let bins = full_tile_fixture(8, 8, &[1.0, 2.0, 2.0]);
    assert_eq!(
        CpuSortRaster::build(&bins, 1e-5, options)
            .unwrap_err()
            .code(),
        ErrorCode::ValueOutOfRange
    );
    let mut v1 = bins.clone();
    v1.contract_version = 1;
    assert_eq!(
        CpuSortRaster::build(&v1, 0.01, options).unwrap_err().code(),
        ErrorCode::UnsupportedVersion
    );

    let Some(mut ctx) = host() else {
        return;
    };
    let before = ctx.resource_accounting().allocated_bytes;

    // Domain-legal f64 pixel that overflows f32 on pack.
    let mut overflow = bins.clone();
    overflow.projected[0].as_mut().unwrap().pixel = [1e40, 0.5];
    overflow.validate().unwrap();
    assert_eq!(
        GpuSortRaster::build(&mut ctx, &overflow, 0.01, options)
            .unwrap_err()
            .code(),
        ErrorCode::ValueOutOfRange
    );
    assert_eq!(ctx.resource_accounting().allocated_bytes, before);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);

    let mut radius = bins.clone();
    radius.projected[0].as_mut().unwrap().radius_px = [1e40, 1.0];
    radius.validate().unwrap();
    assert_eq!(
        GpuSortRaster::build(&mut ctx, &radius, 0.01, options)
            .unwrap_err()
            .code(),
        ErrorCode::ValueOutOfRange
    );
    assert_eq!(ctx.resource_accounting().allocated_bytes, before);

    // f64 SPD, f32 rounds xy→1 so det becomes 0.
    let mut degenerate = bins.clone();
    degenerate.projected[0].as_mut().unwrap().covariance = [1.0, 1.0 - 1e-9, 1.0];
    degenerate.validate().unwrap();
    let det_f64 = 1.0 * 1.0 - (1.0 - 1e-9) * (1.0 - 1e-9);
    assert!(det_f64 > 0.0);
    let xy_f32 = (1.0 - 1e-9) as f32;
    assert_eq!(xy_f32, 1.0);
    assert_eq!(
        GpuSortRaster::build(&mut ctx, &degenerate, 0.01, options)
            .unwrap_err()
            .code(),
        ErrorCode::ValueOutOfRange
    );
    assert_eq!(ctx.resource_accounting().allocated_bytes, before);

    // Domain-legal finite radius that underflows to 0.0 as f32.
    let mut underflow = bins.clone();
    underflow.projected[0].as_mut().unwrap().radius_px = [1e-60, 1.0];
    underflow.validate().unwrap();
    assert_eq!(
        GpuSortRaster::build(&mut ctx, &underflow, 0.01, options)
            .unwrap_err()
            .code(),
        ErrorCode::ValueOutOfRange
    );
    assert_eq!(ctx.resource_accounting().allocated_bytes, before);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);
}

#[test]
fn build_path_closed_lost_oom_pending_cancel_timeout() {
    let options = SortRasterOptions::default();
    let bins = full_tile_fixture(8, 8, &[1.0, 2.0]);
    let Some(mut ctx) = host() else {
        return;
    };

    let err = GpuSortRaster::build(
        &mut ctx,
        &bins,
        0.01,
        SortRasterOptions {
            max_gpu_bytes: 64,
            ..options
        },
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::AllocationFailed);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);

    ctx.inject_next_scope_error(ScopeErrorInject::OutOfMemory {
        requested_bytes: Some(4096),
        current_bytes: Some(0),
        budget_bytes: Some(1024),
    });
    let oom = GpuSortRaster::build(&mut ctx, &bins, 0.01, options).unwrap_err();
    assert_eq!(oom.code(), ErrorCode::AllocationFailed);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);
    assert_eq!(ctx.retained_submission_count(), 0);

    let token = CancelToken::new();
    token.request(CancelReason::User);
    assert_eq!(
        GpuSortRaster::build_cancellable(&mut ctx, &bins, 0.01, options, &token)
            .unwrap_err()
            .code(),
        ErrorCode::Canceled
    );
    assert_eq!(ctx.resource_accounting().current_bytes, 0);

    ctx.inject_next_wait_timeout();
    assert!(GpuSortRaster::build(&mut ctx, &bins, 0.01, options).is_err());
    assert_eq!(ctx.retained_submission_count(), 1);
    ctx.wait_retained_submissions(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    assert_eq!(ctx.retained_submission_count(), 0);
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
        GpuSortRaster::build(&mut ctx, &bins, 0.01, options)
            .unwrap_err()
            .code(),
        ErrorCode::InvalidArgument
    );
    assert_eq!(ctx.pending_resource_counts(), before);
    let submission = ctx.submit().unwrap();
    ctx.wait(submission, None).unwrap();
    drop(owner);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);

    ctx.close();
    assert_eq!(
        GpuSortRaster::build(&mut ctx, &bins, 0.01, options)
            .unwrap_err()
            .code(),
        ErrorCode::DeviceLost
    );

    let Some(mut lost) = host() else {
        return;
    };
    lost.inject_lost("p4-f04 sort-raster deterministic lost");
    assert_eq!(lost.status(), spirula_gpu::ContextStatus::Lost);
    assert_eq!(
        GpuSortRaster::build(&mut lost, &bins, 0.01, options)
            .unwrap_err()
            .code(),
        ErrorCode::DeviceLost
    );
    assert_eq!(lost.resource_accounting().current_bytes, 0);
}

#[test]
fn dispatch_sort_raster_family_layout_alias_cross_context_and_range() {
    let Some(mut ctx) = host() else {
        return;
    };
    let params = ctx
        .create_buffer(BufferDesc {
            label: label("spirula:render:buffer:sr-params"),
            size: 16,
            usage: BufferUsage::UNIFORM_UPLOAD,
        })
        .unwrap();
    let inn = ctx
        .create_buffer(BufferDesc {
            label: label("spirula:render:buffer:sr-in"),
            size: 256,
            usage: BufferUsage::STORAGE_COPY,
        })
        .unwrap();
    let out = ctx
        .create_buffer(BufferDesc {
            label: label("spirula:render:buffer:sr-out"),
            size: 256,
            usage: BufferUsage::STORAGE_COPY,
        })
        .unwrap();
    ctx.upload_buffer(&params, 0, &[0u8; 16]).unwrap();
    ctx.upload_buffer(&inn, 0, &[0u8; 256]).unwrap();

    let keys = ctx
        .obtain_compute_pipeline(ComputePipelineDesc {
            source: SORT_WGSL.to_owned(),
            entry_point: "keys".into(),
            spec: SpecConstants::empty(),
            bind_layout_version: 1,
            label: label("spirula:render:pipeline:sr-keys"),
        })
        .unwrap();
    // Happy path admission for the sort-raster family.
    ctx.dispatch_sort_raster(&keys, &params, &inn, &out, 256, 256, 64)
        .unwrap();

    let tiles = ctx
        .obtain_compute_pipeline(ComputePipelineDesc {
            source: TILE_WGSL.to_owned(),
            entry_point: "project".into(),
            spec: SpecConstants::empty(),
            bind_layout_version: 1,
            label: label("spirula:render:pipeline:sr-wrong-family"),
        })
        .unwrap();
    assert_eq!(
        ctx.dispatch_sort_raster(&tiles, &params, &inn, &out, 256, 256, 64)
            .unwrap_err()
            .code(),
        ErrorCode::InvalidArgument
    );

    // Layout epoch 2 obtains (affine ABI admits 1/2) but sort-raster dispatch
    // requires bind_layout_version == 1.
    let bad_layout = ctx
        .obtain_compute_pipeline(ComputePipelineDesc {
            source: SORT_WGSL.to_owned(),
            entry_point: "histogram".into(),
            spec: SpecConstants::empty(),
            bind_layout_version: 2,
            label: label("spirula:render:pipeline:sr-bad-layout"),
        })
        .unwrap();
    assert_eq!(
        ctx.dispatch_sort_raster(&bad_layout, &params, &inn, &out, 256, 256, 64)
            .unwrap_err()
            .code(),
        ErrorCode::InvalidArgument
    );

    let v2_src = SORT_WGSL.replacen("version: 1", "version: 2", 1);
    let bad_version = ctx
        .obtain_compute_pipeline(ComputePipelineDesc {
            source: v2_src,
            entry_point: "prefix".into(),
            spec: SpecConstants::empty(),
            bind_layout_version: 1,
            label: label("spirula:render:pipeline:sr-bad-version"),
        })
        .unwrap();
    assert_eq!(
        ctx.dispatch_sort_raster(&bad_version, &params, &inn, &out, 256, 256, 64)
            .unwrap_err()
            .code(),
        ErrorCode::InvalidArgument
    );

    // Valid shader entry outside the admitted sort-raster set.
    let other_keys_src = SORT_WGSL.replace("fn keys(", "fn other_keys(");
    let bad_entry = ctx
        .obtain_compute_pipeline(ComputePipelineDesc {
            source: other_keys_src,
            entry_point: "other_keys".into(),
            spec: SpecConstants::empty(),
            bind_layout_version: 1,
            label: label("spirula:render:pipeline:sr-bad-entry"),
        })
        .unwrap();
    assert_eq!(
        ctx.dispatch_sort_raster(&bad_entry, &params, &inn, &out, 256, 256, 64)
            .unwrap_err()
            .code(),
        ErrorCode::InvalidArgument
    );

    assert_eq!(
        ctx.dispatch_sort_raster(&keys, &params, &inn, &inn, 256, 256, 64)
            .unwrap_err()
            .code(),
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        ctx.dispatch_sort_raster(&keys, &params, &inn, &out, 0, 256, 64)
            .unwrap_err()
            .code(),
        ErrorCode::ValueOutOfRange
    );
    assert_eq!(
        ctx.dispatch_sort_raster(&keys, &params, &inn, &out, 512, 256, 64)
            .unwrap_err()
            .code(),
        ErrorCode::ValueOutOfRange
    );
    assert_eq!(
        ctx.dispatch_sort_raster(&keys, &params, &inn, &out, 256, 256, 0)
            .unwrap_err()
            .code(),
        ErrorCode::ValueOutOfRange
    );

    let Some(mut other) = host() else {
        return;
    };
    assert_eq!(
        other
            .dispatch_sort_raster(&keys, &params, &inn, &out, 256, 256, 64)
            .unwrap_err()
            .code(),
        ErrorCode::InvalidArgument
    );

    let submission = ctx.submit().unwrap();
    ctx.wait(submission, None).unwrap();

    // Closed rejects old handles.
    let params_closed = params;
    let inn_closed = inn;
    let out_closed = out;
    let keys_closed = keys;
    ctx.close();
    assert_eq!(
        ctx.dispatch_sort_raster(
            &keys_closed,
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
    drop(keys_closed);
    drop(tiles);
    drop(bad_layout);
    drop(bad_version);
    drop(bad_entry);
    drop(other);

    // Reopen to a new live generation; prior-generation handles must stay dead.
    let Some(mut live) = host() else {
        return;
    };
    let params2 = live
        .create_buffer(BufferDesc {
            label: label("spirula:render:buffer:sr-params2"),
            size: 16,
            usage: BufferUsage::UNIFORM_UPLOAD,
        })
        .unwrap();
    let inn2 = live
        .create_buffer(BufferDesc {
            label: label("spirula:render:buffer:sr-in2"),
            size: 64,
            usage: BufferUsage::STORAGE_COPY,
        })
        .unwrap();
    let out2 = live
        .create_buffer(BufferDesc {
            label: label("spirula:render:buffer:sr-out2"),
            size: 64,
            usage: BufferUsage::STORAGE_COPY,
        })
        .unwrap();
    let keys2 = live
        .obtain_compute_pipeline(ComputePipelineDesc {
            source: SORT_WGSL.to_owned(),
            entry_point: "scatter".into(),
            spec: SpecConstants::empty(),
            bind_layout_version: 1,
            label: label("spirula:render:pipeline:sr-scatter"),
        })
        .unwrap();
    let gen_before = live.generation();
    live.reopen(&CapabilityRequest::diagnose_default())
        .expect("host reopen");
    assert_ne!(live.generation(), gen_before);
    assert_eq!(live.status(), spirula_gpu::ContextStatus::Live);
    assert_eq!(
        live.dispatch_sort_raster(&keys2, &params2, &inn2, &out2, 64, 64, 64)
            .unwrap_err()
            .code(),
        ErrorCode::DeviceLost
    );
    drop(params2);
    drop(inn2);
    drop(out2);
    drop(keys2);
    assert_eq!(live.resource_accounting().current_bytes, 0);
}
