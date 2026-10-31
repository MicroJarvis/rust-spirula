//! Real GPU front-to-back compositing for P4-F05.
//!
//! Chains GpuTileBuilder → GpuSortRaster → WGSL `composite`. Host packs
//! inputs and readbacks; it never fills RGB/T/depth from the CPU oracle.
//! Empty / clear pixels are written by the composite shader for every pixel
//! index — not by host CPU fill or relying on zero-initialized buffers.
use crate::compositing::{
    BackgroundPolicy, COMPOSITE_CONTRACT_VERSION, CompositeImage, CompositeOptions, CompositePixel,
    bins_payload_bytes, collect_scene_sh0_colors, composite_gpu_peak_bytes, composite_pack_bytes,
    host_peak_bound, host_peak_bound_with_bins, pack_f32, preflight_scene_colors_f32,
};
use crate::tiles::error;
use crate::{CpuSortRaster, GpuSortRaster, GpuTileBuilder, RenderRequest, SortedRaster, TileBins};
use spirula_core::{CancelToken, DomainError, ErrorCode};
use spirula_gpu::{
    BufferDesc, BufferHandle, BufferUsage, ComputePipelineDesc, ComputePipelineHandle, GpuContext,
    ResourceLabel, SpecConstants,
};
use std::time::{Duration, Instant};

const WGSL: &str = include_str!("../shaders/compositing.wgsl");

#[derive(Clone, Debug)]
pub struct CompositeMetrics {
    pub pair_count: u64,
    pub pixel_count: u64,
    pub tile_accounted_peak_bytes: u64,
    pub sort_accounted_peak_bytes: u64,
    pub composite_accounted_peak_bytes: u64,
    /// Max of stage accounted GPU peaks (stages release before the next).
    pub accounted_peak_bound_bytes: u64,
    /// Host **stage peak** bound used for preflight (not mere alive finals).
    pub host_peak_bound_bytes: u64,
    pub composite_encode_ns: u128,
    pub composite_sync_ns: u128,
    pub dispatch_count: u32,
    pub adapter: String,
}

pub struct GpuCompositor;

impl GpuCompositor {
    pub fn compose(
        ctx: &mut GpuContext,
        request: &RenderRequest,
        options: CompositeOptions,
    ) -> Result<(CompositeImage, CompositeMetrics), DomainError> {
        Self::compose_cancellable(ctx, request, options, &CancelToken::new())
    }

    /// Empty-bins GPU clear: real `composite` dispatch writes every pixel
    /// (`rgb=bg`, `T=1`, depths 0). Not a host CPU fill.
    pub fn compose_empty(
        ctx: &mut GpuContext,
        width: u32,
        height: u32,
        alpha_cutoff: f64,
        early_out_transmittance: f64,
        options: CompositeOptions,
    ) -> Result<(CompositeImage, CompositeMetrics), DomainError> {
        Self::compose_empty_cancellable(
            ctx,
            width,
            height,
            alpha_cutoff,
            early_out_transmittance,
            options,
            &CancelToken::new(),
        )
    }

    pub fn compose_empty_cancellable(
        ctx: &mut GpuContext,
        width: u32,
        height: u32,
        alpha_cutoff: f64,
        early_out_transmittance: f64,
        options: CompositeOptions,
        cancel: &CancelToken,
    ) -> Result<(CompositeImage, CompositeMetrics), DomainError> {
        let options = options.capped();
        options.background.validate()?;
        options.background.preflight_f32()?;
        validate_cutoff(alpha_cutoff)?;
        validate_early_out(early_out_transmittance)?;
        pack_f32(alpha_cutoff)?;
        pack_f32(early_out_transmittance)?;

        cancel.check()?;
        ctx.poll_host_lost();
        ctx.device_handle()?;
        if !ctx.backend_is_host() {
            return Err(error(
                ErrorCode::UnimplementedOption,
                "gpu_context",
                "compositing requires a real GPU",
            ));
        }
        require_empty_pending(ctx)?;

        let (tiles_x, tiles_y) = options.tile.dimensions(width, height, 0)?;
        let m = u64::from(width)
            .checked_mul(u64::from(height))
            .ok_or_else(|| range("pixels"))?;
        if m > options.max_pixels {
            return Err(range("pixels"));
        }
        let t = u64::from(tiles_x)
            .checked_mul(u64::from(tiles_y))
            .ok_or_else(|| range("tiles"))?;
        let peak = host_peak_bound(0, 0, t, m)?;
        if peak > options.max_host_bytes || options.max_gpu_bytes == 0 {
            return Err(range("budget"));
        }
        // Known F05 GPU peak with N=P=0 before any work.
        preflight_composite_gpu(ctx, 0, 0, t, m, options.max_gpu_bytes)?;

        let result = (|| {
            let (bins, tile_metrics) =
                GpuTileBuilder::build_empty(ctx, width, height, options.tile)?;
            require_empty_pending(ctx)?;
            cancel.check()?;
            // Empty sort is host-side (zero pairs / zero GPU sort dispatches).
            let sorted = CpuSortRaster::build(&bins, alpha_cutoff, options.sort)?;
            dispatch_composite(
                ctx,
                &bins,
                &sorted,
                &[],
                alpha_cutoff,
                early_out_transmittance,
                options,
                tile_metrics.accounted_peak_bound_bytes,
                0,
                tile_metrics.dispatch_count,
                peak,
                cancel,
            )
        })();
        if result.is_err() {
            ctx.abandon_unsubmitted_pending();
        }
        result
    }

    pub fn compose_cancellable(
        ctx: &mut GpuContext,
        request: &RenderRequest,
        options: CompositeOptions,
        cancel: &CancelToken,
    ) -> Result<(CompositeImage, CompositeMetrics), DomainError> {
        request.validate()?;
        let options = options.capped();
        options.background.validate()?;
        options.background.preflight_f32()?;
        let cutoff = request.thresholds.alpha_cutoff;
        let early_out = request.thresholds.early_out_transmittance;
        validate_cutoff(cutoff)?;
        validate_early_out(early_out)?;
        // Allocation-free f32 preflight before F03/F04. Color Vec deferred until
        // after sort; entry peak still reserves 24N via during_comp.
        preflight_scene_colors_f32(request)?;
        pack_f32(cutoff)?;
        pack_f32(early_out)?;

        cancel.check()?;
        ctx.poll_host_lost();
        ctx.device_handle()?;
        if !ctx.backend_is_host() {
            return Err(error(
                ErrorCode::UnimplementedOption,
                "gpu_context",
                "compositing requires a real GPU",
            ));
        }
        require_empty_pending(ctx)?;

        let width = request.camera.width();
        let height = request.camera.height();
        let n = request.scene.len();
        let (tiles_x, tiles_y) = options.tile.dimensions(width, height, n)?;
        let m = u64::from(width)
            .checked_mul(u64::from(height))
            .ok_or_else(|| range("pixels"))?;
        if m > options.max_pixels {
            return Err(range("pixels"));
        }
        let t = u64::from(tiles_x)
            .checked_mul(u64::from(tiles_y))
            .ok_or_else(|| range("tiles"))?;
        let n64 = n as u64;
        let p_upper = n64
            .checked_mul(t)
            .ok_or_else(|| range("pairs"))?
            .min(options.max_pairs);
        let peak = host_peak_bound(n64, p_upper, t, m)?;
        if peak > options.max_host_bytes || options.max_gpu_bytes == 0 {
            return Err(range("budget"));
        }
        // P=0 lower-bound F05 GPU peak — reject before F03/F04 when even the
        // empty-pair composite stage cannot fit. Do not use worst-case P here.
        preflight_composite_gpu(ctx, n64, 0, t, m, options.max_gpu_bytes)?;

        let result = execute(ctx, request, options, cutoff, early_out, peak, cancel);
        if result.is_err() {
            ctx.abandon_unsubmitted_pending();
        }
        result
    }
}

fn range(field: &str) -> DomainError {
    error(
        ErrorCode::ValueOutOfRange,
        field,
        "composite range or resource limit exceeded",
    )
}

fn validate_cutoff(cutoff: f64) -> Result<(), DomainError> {
    if !cutoff.is_finite() || cutoff <= 0.0 || cutoff > 1.0 {
        return Err(range("alpha_cutoff"));
    }
    pack_f32(cutoff)?;
    Ok(())
}

fn validate_early_out(early_out: f64) -> Result<(), DomainError> {
    if !early_out.is_finite() || !(0.0..=1.0).contains(&early_out) || early_out == 0.0 {
        return Err(range("early_out"));
    }
    pack_f32(early_out)?;
    Ok(())
}

fn require_empty_pending(ctx: &GpuContext) -> Result<(), DomainError> {
    let pending = ctx.pending_resource_counts();
    if pending.commands != 0
        || pending.stagings != 0
        || pending.buffers != 0
        || pending.textures != 0
    {
        return Err(error(
            ErrorCode::InvalidArgument,
            "pending_queue",
            "compositing requires an empty unsubmitted queue",
        ));
    }
    Ok(())
}

/// Known F05 GPU stage peak vs operation/context budget — no packing, no GPU alloc.
fn preflight_composite_gpu(
    ctx: &GpuContext,
    n: u64,
    p: u64,
    t: u64,
    m: u64,
    max_gpu_bytes: u64,
) -> Result<u64, DomainError> {
    let peak = composite_gpu_peak_bytes(n, p, t, m)?;
    if peak > max_gpu_bytes {
        return Err(error(
            ErrorCode::AllocationFailed,
            "gpu_bytes",
            "compositing peak exceeds operation GPU budget",
        ));
    }
    let current = ctx.resource_accounting();
    if current
        .current_bytes
        .checked_add(peak)
        .is_none_or(|total| current.budget_limit.is_some_and(|cap| total > cap))
    {
        return Err(error(
            ErrorCode::AllocationFailed,
            "gpu_budget",
            "compositing peak exceeds operation/context accounted budget",
        ));
    }
    Ok(peak)
}

fn make_buffer(
    ctx: &mut GpuContext,
    name: &str,
    bytes: u64,
    usage: BufferUsage,
) -> Result<BufferHandle, DomainError> {
    ctx.create_buffer(BufferDesc {
        label: ResourceLabel::parse(&format!("spirula:render:buffer:{name}"))
            .expect("static compositing label"),
        size: bytes,
        usage,
    })
}

fn pipe(ctx: &mut GpuContext) -> Result<ComputePipelineHandle, DomainError> {
    ctx.obtain_compute_pipeline(ComputePipelineDesc {
        source: WGSL.to_owned(),
        entry_point: "composite".to_owned(),
        spec: SpecConstants::empty(),
        bind_layout_version: 1,
        label: ResourceLabel::parse("spirula:render:pipeline:compositing").expect("pipeline label"),
    })
}

fn pack_input(
    bins: &TileBins,
    sorted: &SortedRaster,
    colors: &[[f64; 3]],
    cutoff: f64,
    early_out: f64,
    background: [f64; 3],
) -> Result<Vec<u32>, DomainError> {
    let n = bins.projected.len() as u32;
    let p = sorted.pairs.len() as u32;
    let t = bins.tile_count() as u32;
    if colors.len() != bins.projected.len() && !(colors.is_empty() && n == 0) {
        return Err(error(
            ErrorCode::InvalidArgument,
            "colors",
            "color count must match projected source slots",
        ));
    }
    let mut words =
        Vec::with_capacity((16 + 16 * n as usize + 4 * p as usize + 2 * t as usize).max(16));
    words.extend([
        bins.width,
        bins.height,
        bins.tile_size,
        bins.tiles_x,
        pack_f32(cutoff)?,
        pack_f32(early_out)?,
        n,
        p,
    ]);
    words.push(bins.tiles_y);
    words.push(pack_f32(background[0])?);
    words.push(pack_f32(background[1])?);
    words.push(pack_f32(background[2])?);
    words.extend([0u32; 4]);

    for (idx, item) in bins.projected.iter().enumerate() {
        if let Some(proj) = item {
            let xx = proj.covariance[0] as f32;
            let xy = proj.covariance[1] as f32;
            let yy = proj.covariance[2] as f32;
            let det = xx * yy - xy * xy;
            if !det.is_finite() || det <= 0.0 {
                return Err(range("f32_covariance"));
            }
            words.push(pack_f32(proj.pixel[0])?);
            words.push(pack_f32(proj.pixel[1])?);
            words.push(pack_f32(proj.depth)?);
            words.push(pack_f32(proj.ray_depth)?);
            words.push(pack_f32(proj.radius_px[0])?);
            words.push(pack_f32(proj.radius_px[1])?);
            words.push(pack_f32(proj.covariance[0])?);
            words.push(pack_f32(proj.covariance[1])?);
            words.push(pack_f32(proj.covariance[2])?);
            words.push(pack_f32(proj.opacity)?);
            let c = colors[idx];
            words.push(pack_f32(c[0])?);
            words.push(pack_f32(c[1])?);
            words.push(pack_f32(c[2])?);
            words.extend([0u32; 3]);
        } else {
            words.extend([0u32; 16]);
        }
    }
    for pair in &sorted.pairs {
        words.extend([
            pair.tile_id,
            pair.depth_key,
            pair.source_index,
            pair.input_index,
        ]);
    }
    for tr in &sorted.tile_ranges {
        words.extend([tr.start, tr.count]);
    }
    Ok(words)
}

fn wait_map(
    ctx: &mut GpuContext,
    ticket: &spirula_gpu::ReadbackTicket,
) -> Result<Vec<u8>, DomainError> {
    let submission = ctx.submit()?;
    ctx.wait(submission, Some(Duration::from_secs(30)))?;
    ctx.map_readback(ticket)
}

fn decode_image(
    bytes: &[u8],
    sorted: &SortedRaster,
    cutoff: f64,
    early_out: f64,
    background: BackgroundPolicy,
) -> Result<CompositeImage, DomainError> {
    let m = (sorted.width * sorted.height) as usize;
    if bytes.len() < m * 32 {
        return Err(range("readback"));
    }
    let mut pixels = Vec::with_capacity(m);
    for i in 0..m {
        let base = i * 32;
        let mut words = [0u32; 8];
        let (chunks, _) = bytes[base..base + 32].as_chunks::<4>();
        for (w, chunk) in words.iter_mut().zip(chunks) {
            *w = u32::from_le_bytes(*chunk);
        }
        if words[6] != 0 {
            return Err(range("gpu_status"));
        }
        let rgb = [
            f32::from_bits(words[0]) as f64,
            f32::from_bits(words[1]) as f64,
            f32::from_bits(words[2]) as f64,
        ];
        let t = f32::from_bits(words[3]) as f64;
        let expected = f32::from_bits(words[4]) as f64;
        let median = f32::from_bits(words[5]) as f64;
        let pixel = CompositePixel {
            rgb,
            transmittance: t,
            alpha: 1.0 - t,
            expected_depth: expected,
            median_depth: median,
        };
        pixel.validate_finite()?;
        pixels.push(pixel);
    }
    Ok(CompositeImage {
        contract_version: COMPOSITE_CONTRACT_VERSION,
        width: sorted.width,
        height: sorted.height,
        alpha_cutoff: cutoff,
        effective_alpha_cutoff: cutoff as f32,
        early_out_transmittance: early_out,
        effective_early_out: early_out as f32,
        background,
        pixels,
    })
}

#[allow(clippy::too_many_arguments)]
fn dispatch_composite(
    ctx: &mut GpuContext,
    bins: &TileBins,
    sorted: &SortedRaster,
    colors: &[[f64; 3]],
    cutoff: f64,
    early_out: f64,
    options: CompositeOptions,
    tile_peak: u64,
    sort_peak: u64,
    prior_dispatches: u32,
    host_peak: u64,
    cancel: &CancelToken,
) -> Result<(CompositeImage, CompositeMetrics), DomainError> {
    let report = ctx.report().expect("live report");
    let adapter = format!("{:?}/{}", report.backend, report.adapter.identity.name);
    let n = bins.projected.len() as u64;
    let p = sorted.pairs.len() as u64;
    let t = bins.tile_count() as u64;
    let m = u64::from(bins.width) * u64::from(bins.height);

    let mut metrics = CompositeMetrics {
        pair_count: p,
        pixel_count: m,
        tile_accounted_peak_bytes: tile_peak,
        sort_accounted_peak_bytes: sort_peak,
        composite_accounted_peak_bytes: 0,
        accounted_peak_bound_bytes: tile_peak.max(sort_peak),
        host_peak_bound_bytes: host_peak,
        composite_encode_ns: 0,
        composite_sync_ns: 0,
        dispatch_count: prior_dispatches,
        adapter,
    };

    cancel.check()?;
    // Known sizes / budget / device limits before host packing (still before
    // GPU alloc). Avoids building a large pack_input Vec that is already rejected.
    let input_bytes = composite_pack_bytes(n, p, t)?;
    let output_bytes = m.checked_mul(32).ok_or_else(|| range("pixels"))?;
    let composite_peak = preflight_composite_gpu(ctx, n, p, t, m, options.max_gpu_bytes)?;
    metrics.composite_accounted_peak_bytes = composite_peak;
    metrics.accounted_peak_bound_bytes = metrics.accounted_peak_bound_bytes.max(composite_peak);

    let limits = report.device_limits;
    let invocations = m.max(1) as u32;
    if invocations.div_ceil(64) > limits.max_compute_workgroups_per_dimension
        || input_bytes > limits.max_storage_buffer_binding_size
        || output_bytes > limits.max_storage_buffer_binding_size
        || invocations == 0
    {
        return Err(error(
            ErrorCode::NoAdapter,
            "gpu_limits",
            "compositing storage or dispatch exceeds enabled device limits",
        ));
    }

    let words = pack_input(
        bins,
        sorted,
        colors,
        cutoff,
        early_out,
        options.background.rgb(),
    )?;
    debug_assert_eq!(input_bytes, (words.len() * 4) as u64);

    let encode_start = Instant::now();
    let pipeline = pipe(ctx)?;
    let params = make_buffer(ctx, "composite-params", 16, BufferUsage::UNIFORM_UPLOAD)?;
    let params_bytes: Vec<u8> = [invocations, n as u32, p as u32, t as u32]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    ctx.upload_buffer(&params, 0, &params_bytes)?;

    let input = make_buffer(
        ctx,
        "composite-input",
        input_bytes,
        BufferUsage::STORAGE_COPY,
    )?;
    let data: Vec<u8> = words.into_iter().flat_map(u32::to_le_bytes).collect();
    ctx.upload_buffer(&input, 0, &data)?;

    let output = make_buffer(
        ctx,
        "composite-output",
        output_bytes,
        BufferUsage::STORAGE_COPY,
    )?;
    metrics.composite_encode_ns = encode_start.elapsed().as_nanos();

    let sync_start = Instant::now();
    ctx.dispatch_compositing(
        &pipeline,
        &params,
        &input,
        &output,
        input_bytes,
        output_bytes,
        invocations,
    )?;
    metrics.dispatch_count += 1;
    let ticket = ctx.readback_buffer(&output, 0, output_bytes)?;
    let bytes = wait_map(ctx, &ticket)?;
    metrics.composite_sync_ns = sync_start.elapsed().as_nanos();

    drop(ticket);
    drop(output);
    drop(input);
    drop(params);
    drop(data);

    let image = decode_image(&bytes, sorted, cutoff, early_out, options.background)?;
    Ok((image, metrics))
}

#[allow(clippy::too_many_arguments)]
fn execute(
    ctx: &mut GpuContext,
    request: &RenderRequest,
    mut options: CompositeOptions,
    cutoff: f64,
    early_out: f64,
    entry_peak: u64,
    cancel: &CancelToken,
) -> Result<(CompositeImage, CompositeMetrics), DomainError> {
    let (bins, tile_metrics) =
        GpuTileBuilder::build_cancellable(ctx, request, options.tile, cancel)?;
    require_empty_pending(ctx)?;
    cancel.check()?;

    let n = bins.projected.len() as u64;
    let p = bins.pairs.len() as u64;
    let t = bins.tile_count() as u64;
    let m = u64::from(bins.width) * u64::from(bins.height);
    if p > options.max_pairs {
        return Err(range("pairs"));
    }
    let mid_peak = host_peak_bound_with_bins(&bins, p, t, m)?;
    if mid_peak > options.max_host_bytes {
        return Err(range("host_bytes"));
    }
    // Exact F05 composite peak with realized P — before F04 GPU work.
    preflight_composite_gpu(ctx, n, p, t, m, options.max_gpu_bytes)?;
    // Deduct caller-owned bins capacity; colors still deferred.
    let bins_bytes = bins_payload_bytes(&bins)?;
    options.sort.max_host_bytes = options
        .max_host_bytes
        .saturating_sub(bins_bytes)
        .min(options.sort.max_host_bytes);

    let (sorted, sort_metrics) =
        GpuSortRaster::build_cancellable(ctx, &bins, cutoff, options.sort, cancel)?;
    require_empty_pending(ctx)?;
    cancel.check()?;

    let colors = collect_scene_sh0_colors(request)?;
    // Always dispatch composite — including P=0 — so every pixel is shader-written.
    dispatch_composite(
        ctx,
        &bins,
        &sorted,
        &colors,
        cutoff,
        early_out,
        options,
        tile_metrics.accounted_peak_bound_bytes,
        sort_metrics.accounted_peak_bound_bytes,
        tile_metrics.dispatch_count + sort_metrics.dispatch_count,
        entry_peak.max(mid_peak),
        cancel,
    )
}
