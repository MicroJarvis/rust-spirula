//! Real GPU stable LSD radix sort and conic raster coverage for P4-F04.
//!
//! Sixteen 4-bit histogram/prefix/scatter rounds run on device; host code only
//! packs inputs, submits, and reads back. CPU sorting never substitutes for the
//! GPU path. Timings are wall-clock encode/sync around submit/wait/map — not
//! GPU timestamps.
use crate::sort_raster::{RasterPlan, range};
use crate::tiles::error;
use crate::{
    PixelCoverage, SORT_RASTER_VERSION, SortRasterOptions, SortedRaster, SortedTilePair, TileBins,
    TileRange,
};
use spirula_core::{CancelToken, DomainError, ErrorCode};
use spirula_gpu::{
    BufferDesc, BufferHandle, BufferUsage, ComputePipelineDesc, ComputePipelineHandle, GpuContext,
    ResourceLabel, SpecConstants,
};
use std::time::{Duration, Instant};

const WGSL: &str = include_str!("../shaders/sort-raster.wgsl");

#[derive(Clone, Debug)]
pub struct SortRasterMetrics {
    pub pair_count: u64,
    pub raster_tests: u64,
    pub radix_scratch_bytes: u64,
    pub tile_range_bytes: u64,
    pub raster_output_bytes: u64,
    /// Upper bound for this call's GPU buffers + staging; not physical VRAM.
    pub accounted_peak_bound_bytes: u64,
    pub sort_readback_bytes: u64,
    pub sort_encode_ns: u128,
    pub sort_sync_ns: u128,
    pub raster_sync_ns: u128,
    pub dispatch_count: u32,
    pub adapter: String,
}

pub struct GpuSortRaster;

impl GpuSortRaster {
    pub fn build(
        ctx: &mut GpuContext,
        bins: &TileBins,
        cutoff: f64,
        options: SortRasterOptions,
    ) -> Result<(SortedRaster, SortRasterMetrics), DomainError> {
        Self::build_cancellable(ctx, bins, cutoff, options, &CancelToken::new())
    }

    /// Exclusive `&mut GpuContext` owns the pending queue for this call.
    /// Unrelated unsubmitted work is rejected unchanged; accepted submissions
    /// stay context-owned across timeout until `wait_retained_submissions`.
    pub fn build_cancellable(
        ctx: &mut GpuContext,
        bins: &TileBins,
        cutoff: f64,
        options: SortRasterOptions,
        cancel: &CancelToken,
    ) -> Result<(SortedRaster, SortRasterMetrics), DomainError> {
        let plan = options.plan(bins, cutoff)?;
        cancel.check()?;
        ctx.poll_host_lost();
        ctx.device_handle()?;
        if !ctx.backend_is_host() {
            return Err(error(
                ErrorCode::UnimplementedOption,
                "gpu_context",
                "sort/raster requires a real GPU",
            ));
        }
        let pending = ctx.pending_resource_counts();
        if pending.commands != 0
            || pending.stagings != 0
            || pending.buffers != 0
            || pending.textures != 0
        {
            return Err(error(
                ErrorCode::InvalidArgument,
                "pending_queue",
                "sort/raster requires an empty unsubmitted queue",
            ));
        }
        let result = execute(ctx, bins, cutoff, options, plan, cancel);
        if result.is_err() {
            ctx.abandon_unsubmitted_pending();
        }
        result
    }
}

fn make_buffer(
    ctx: &mut GpuContext,
    name: &str,
    bytes: u64,
    usage: BufferUsage,
) -> Result<BufferHandle, DomainError> {
    ctx.create_buffer(BufferDesc {
        label: ResourceLabel::parse(&format!("spirula:render:buffer:{name}"))
            .expect("static sort-raster label"),
        size: bytes,
        usage,
    })
}

fn pipe(ctx: &mut GpuContext, entry: &str) -> Result<ComputePipelineHandle, DomainError> {
    ctx.obtain_compute_pipeline(ComputePipelineDesc {
        source: WGSL.to_owned(),
        entry_point: entry.to_owned(),
        spec: SpecConstants::empty(),
        bind_layout_version: 1,
        label: ResourceLabel::parse(&format!("spirula:render:pipeline:sort-{entry}"))
            .expect("pipeline label"),
    })
}

fn uniform(
    ctx: &mut GpuContext,
    plan: RasterPlan,
    shift: u32,
) -> Result<BufferHandle, DomainError> {
    let handle = make_buffer(ctx, "sort-params", 16, BufferUsage::UNIFORM_UPLOAD)?;
    let bytes: Vec<u8> = [plan.p, plan.n, shift, plan.t]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    ctx.upload_buffer(&handle, 0, &bytes)?;
    Ok(handle)
}

fn budget(
    ctx: &GpuContext,
    options: SortRasterOptions,
    operation_peak: u64,
    additional: u64,
) -> Result<(), DomainError> {
    let current = ctx.resource_accounting();
    if operation_peak > options.max_gpu_bytes
        || current
            .current_bytes
            .checked_add(additional)
            .is_none_or(|n| current.budget_limit.is_some_and(|cap| n > cap))
    {
        return Err(error(
            ErrorCode::AllocationFailed,
            "gpu_budget",
            "sort/raster peak exceeds operation/context accounted budget",
        ));
    }
    Ok(())
}

fn f32_bits(v: f64) -> Result<u32, DomainError> {
    let f = v as f32;
    if !v.is_finite() || !f.is_finite() || (v != 0.0 && f == 0.0) {
        return Err(range("f32_projection"));
    }
    Ok(f.to_bits())
}

fn input_words(bins: &TileBins, cutoff: f64, plan: RasterPlan) -> Result<Vec<u32>, DomainError> {
    let mut words = Vec::with_capacity(plan.input_bytes as usize / 4);
    words.extend([
        bins.width,
        bins.height,
        bins.tile_size,
        bins.tiles_x,
        f32_bits(cutoff)?,
        bins.tiles_y,
        plan.n,
        plan.p,
    ]);
    for item in &bins.projected {
        if let Some(p) = item {
            let xx = p.covariance[0] as f32;
            let xy = p.covariance[1] as f32;
            let yy = p.covariance[2] as f32;
            let det = xx * yy - xy * xy;
            if !det.is_finite()
                || det <= 0.0
                || !(yy / det).is_finite()
                || !(xy / det).is_finite()
                || !(xx / det).is_finite()
            {
                return Err(range("f32_covariance"));
            }
            let opacity = p.opacity as f32;
            if !opacity.is_finite() || (p.opacity != 0.0 && opacity == 0.0) || opacity < 0.0 {
                return Err(range("f32_opacity"));
            }
            let values = [
                p.pixel[0],
                p.pixel[1],
                p.depth,
                p.ray_depth,
                p.radius_px[0],
                p.radius_px[1],
                p.covariance[0],
                p.covariance[1],
                p.covariance[2],
                p.opacity,
            ];
            for value in values {
                words.push(f32_bits(value)?);
            }
            words.extend(p.tile_bounds);
            words.extend([0, 1]);
        } else {
            words.extend([0u32; 16]);
        }
    }
    for pair in &bins.pairs {
        words.extend([pair.tile_id as u32, pair.source_index as u32]);
    }
    Ok(words)
}

fn wait_map(
    ctx: &mut GpuContext,
    ticket: &spirula_gpu::ReadbackTicket,
) -> Result<Vec<u8>, DomainError> {
    let submission = ctx.submit()?;
    // Timeout retains accepted resources; caller may wait_retained_submissions.
    ctx.wait(submission, Some(Duration::from_secs(30)))?;
    ctx.map_readback(ticket)
}

fn execute(
    ctx: &mut GpuContext,
    bins: &TileBins,
    cutoff: f64,
    options: SortRasterOptions,
    plan: RasterPlan,
    cancel: &CancelToken,
) -> Result<(SortedRaster, SortRasterMetrics), DomainError> {
    let device = ctx.report().expect("live report");
    let adapter = format!("{:?}/{}", device.backend, device.adapter.identity.name);
    let mut metrics = SortRasterMetrics {
        pair_count: u64::from(plan.p),
        raster_tests: plan.raster_tests,
        radix_scratch_bytes: 2 * (u64::from(plan.p) * 16 + u64::from(plan.groups) * 64),
        tile_range_bytes: u64::from(plan.t) * 8,
        raster_output_bytes: u64::from(plan.m) * 16,
        accounted_peak_bound_bytes: 0,
        sort_readback_bytes: plan.sort_readback_bytes,
        sort_encode_ns: 0,
        sort_sync_ns: 0,
        raster_sync_ns: 0,
        dispatch_count: 0,
        adapter,
    };
    if plan.p == 0 {
        let out = SortedRaster {
            contract_version: SORT_RASTER_VERSION,
            width: bins.width,
            height: bins.height,
            tile_size: bins.tile_size,
            alpha_cutoff: cutoff,
            effective_alpha_cutoff: cutoff as f32,
            pairs: vec![],
            tile_ranges: vec![TileRange { start: 0, count: 0 }; plan.t as usize],
            pixels: vec![PixelCoverage::default(); plan.m as usize],
        };
        out.validate_against(bins)?;
        return Ok((out, metrics));
    }

    let limits = device.device_limits;
    let invocations = plan.p.max(plan.n).max(plan.t).max(plan.m).max(16);
    if invocations.div_ceil(64) > limits.max_compute_workgroups_per_dimension
        || plan.state_bytes > limits.max_storage_buffer_binding_size
        || plan.input_bytes > limits.max_storage_buffer_binding_size
        || u64::from(plan.m) * 16 > limits.max_storage_buffer_binding_size
    {
        return Err(error(
            ErrorCode::NoAdapter,
            "gpu_limits",
            "sort/raster storage or dispatch exceeds enabled device limits",
        ));
    }

    metrics.accounted_peak_bound_bytes = plan.first_gpu_peak.max(plan.second_gpu_peak);
    budget(
        ctx,
        options,
        metrics.accounted_peak_bound_bytes,
        metrics.accounted_peak_bound_bytes,
    )?;

    let words = input_words(bins, cutoff, plan)?;
    let data: Vec<u8> = words.into_iter().flat_map(u32::to_le_bytes).collect();
    let keys = pipe(ctx, "keys")?;
    let histogram = pipe(ctx, "histogram")?;
    let prefix = pipe(ctx, "prefix")?;
    let scatter = pipe(ctx, "scatter")?;
    let ranges = pipe(ctx, "ranges")?;
    let raster = pipe(ctx, "raster")?;

    let source = make_buffer(
        ctx,
        "sort-input",
        plan.input_bytes,
        BufferUsage::STORAGE_COPY,
    )?;
    let mut a = make_buffer(
        ctx,
        "sort-state-a",
        plan.state_bytes,
        BufferUsage::STORAGE_COPY,
    )?;
    let mut b = make_buffer(
        ctx,
        "sort-state-b",
        plan.state_bytes,
        BufferUsage::STORAGE_COPY,
    )?;

    let started = Instant::now();
    ctx.upload_buffer(&source, 0, &data)?;
    let key_uniform = uniform(ctx, plan, 0)?;
    let work = plan.p.max(plan.n).max(16);
    ctx.dispatch_sort_raster(
        &keys,
        &key_uniform,
        &source,
        &a,
        plan.input_bytes,
        plan.state_bytes,
        work,
    )?;
    let mut uniforms = vec![key_uniform];
    for shift in (0..64).step_by(4) {
        cancel.check()?;
        for pipeline in [&histogram, &prefix, &scatter] {
            let params = uniform(ctx, plan, shift)?;
            ctx.dispatch_sort_raster(
                pipeline,
                &params,
                &a,
                &b,
                plan.state_bytes,
                plan.state_bytes,
                work,
            )?;
            uniforms.push(params);
            std::mem::swap(&mut a, &mut b);
        }
    }
    cancel.check()?;
    let params = uniform(ctx, plan, 0)?;
    ctx.dispatch_sort_raster(
        &ranges,
        &params,
        &a,
        &b,
        plan.state_bytes,
        plan.state_bytes,
        work.max(plan.t),
    )?;
    uniforms.push(params);
    std::mem::swap(&mut a, &mut b);
    metrics.sort_encode_ns = started.elapsed().as_nanos();
    // keys + 16*(histogram+prefix+scatter) + ranges
    metrics.dispatch_count = 50;

    let sync = Instant::now();
    let keys_ticket = ctx.readback_buffer(&a, 32, u64::from(plan.p) * 16)?;
    let range_offset = 32 + 16 * u64::from(plan.p) + 64 * u64::from(plan.groups);
    let ranges_ticket = ctx.readback_buffer(&a, range_offset, u64::from(plan.t) * 8)?;
    let key_bytes = wait_map(ctx, &keys_ticket)?;
    let range_bytes = ctx.map_readback(&ranges_ticket)?;
    metrics.sort_sync_ns = sync.elapsed().as_nanos();
    drop(keys_ticket);
    drop(ranges_ticket);
    drop(uniforms);
    drop(b);
    drop(source);
    drop(data);
    cancel.check()?;

    let pairs = key_bytes
        .as_chunks::<16>()
        .0
        .iter()
        .map(|row| {
            let w = row.as_chunks::<4>().0;
            SortedTilePair {
                tile_id: u32::from_le_bytes(w[0]),
                depth_key: u32::from_le_bytes(w[1]),
                source_index: u32::from_le_bytes(w[2]),
                input_index: u32::from_le_bytes(w[3]),
            }
        })
        .collect();
    drop(key_bytes);
    let tile_ranges = range_bytes
        .as_chunks::<8>()
        .0
        .iter()
        .map(|row| {
            let w = row.as_chunks::<4>().0;
            TileRange {
                start: u32::from_le_bytes(w[0]),
                count: u32::from_le_bytes(w[1]),
            }
        })
        .collect();
    drop(range_bytes);

    let raster_additional = plan.second_gpu_peak.saturating_sub(plan.state_bytes);
    budget(ctx, options, plan.second_gpu_peak, raster_additional)?;

    let started = Instant::now();
    let out = make_buffer(
        ctx,
        "raster-coverage",
        u64::from(plan.m) * 16,
        BufferUsage::STORAGE_COPY,
    )?;
    let params = uniform(ctx, plan, 0)?;
    ctx.dispatch_sort_raster(
        &raster,
        &params,
        &a,
        &out,
        plan.state_bytes,
        u64::from(plan.m) * 16,
        plan.m,
    )?;
    let ticket = ctx.readback_buffer(&out, 0, u64::from(plan.m) * 16)?;
    let bytes = wait_map(ctx, &ticket)?;
    metrics.raster_sync_ns = started.elapsed().as_nanos();
    metrics.dispatch_count += 1;
    cancel.check()?;
    drop(params);
    drop(out);
    drop(a);
    drop(ticket);

    let mut pixels = Vec::with_capacity(plan.m as usize);
    for row in bytes.as_chunks::<16>().0 {
        let w = row.as_chunks::<4>().0;
        if u32::from_le_bytes(w[3]) != 0 {
            return Err(range("gpu_raster_numeric"));
        }
        pixels.push(PixelCoverage {
            count: u32::from_le_bytes(w[0]),
            first_source: u32::from_le_bytes(w[1]),
            max_alpha: f64::from(f32::from_bits(u32::from_le_bytes(w[2]))),
        });
    }
    // Requested f64 cutoff is preserved; effective_alpha_cutoff records the
    // packed f32 the shader compares against. Empty and non-empty share this.
    let result = SortedRaster {
        contract_version: SORT_RASTER_VERSION,
        width: bins.width,
        height: bins.height,
        tile_size: bins.tile_size,
        alpha_cutoff: cutoff,
        effective_alpha_cutoff: cutoff as f32,
        pairs,
        tile_ranges,
        pixels,
    };
    result.validate_against(bins)?;
    Ok((result, metrics))
}
