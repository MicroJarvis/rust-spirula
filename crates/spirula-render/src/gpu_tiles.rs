//! Synchronous, headless P4-F03 GPU execution through opaque resource handles.
use crate::tiles::{RECORD_WORDS, error, finish, validate_request};
use crate::{
    PackedSplatData, ProjectedTileSplat, RenderRequest, TileBins, TileBuildOptions, TilePair,
};
use spirula_core::{CancelToken, DomainError, ErrorCode};
use spirula_gpu::{
    BufferDesc, BufferHandle, BufferUsage, ComputePipelineDesc, ComputePipelineHandle, GpuContext,
    ResourceLabel, SpecConstants,
};
use std::time::{Duration, Instant};
const SOURCE: &str = include_str!("../shaders/projection-tiles.wgsl");

#[derive(Clone, Debug)]
pub struct TileBuildMetrics {
    pub intersection_peak: u64,
    pub scan_scratch_bytes: u64,
    /// Upper bound for this call's GPU buffers + staging; not physical VRAM.
    pub accounted_peak_bound_bytes: u64,
    pub projection_scan_encode_ns: u128,
    pub count_sync_ns: u128,
    pub emission_sync_ns: u128,
    pub dispatch_count: u32,
    pub adapter: String,
}
pub struct GpuTileBuilder;
impl GpuTileBuilder {
    pub fn build_empty(
        context: &mut GpuContext,
        width: u32,
        height: u32,
        options: TileBuildOptions,
    ) -> Result<(TileBins, TileBuildMetrics), DomainError> {
        let bins = TileBins::empty(width, height, options)?;
        context.poll_host_lost();
        context.device_handle()?;
        if !context.backend_is_host() {
            return Err(error(
                ErrorCode::UnimplementedOption,
                "gpu_context",
                "projection tiles need a real GPU",
            ));
        }
        let report = context
            .report()
            .ok_or_else(|| error(ErrorCode::DeviceLost, "context", "missing device"))?;
        Ok((
            bins,
            TileBuildMetrics {
                intersection_peak: 0,
                scan_scratch_bytes: 0,
                accounted_peak_bound_bytes: 0,
                projection_scan_encode_ns: 0,
                count_sync_ns: 0,
                emission_sync_ns: 0,
                dispatch_count: 0,
                adapter: format!("{:?}/{}", report.backend, report.adapter.identity.name),
            },
        ))
    }
    pub fn build(
        context: &mut GpuContext,
        request: &RenderRequest,
        options: TileBuildOptions,
    ) -> Result<(TileBins, TileBuildMetrics), DomainError> {
        Self::build_cancellable(context, request, options, &CancelToken::new())
    }
    /// The call exclusively owns the pending queue until completion. Existing
    /// in-flight work is allowed; unrelated unsubmitted work is rejected intact.
    pub fn build_cancellable(
        context: &mut GpuContext,
        request: &RenderRequest,
        options: TileBuildOptions,
        cancel: &CancelToken,
    ) -> Result<(TileBins, TileBuildMetrics), DomainError> {
        validate_request(request)?;
        let (tx, _) = options.dimensions(
            request.camera.width(),
            request.camera.height(),
            request.scene.len(),
        )?;
        cancelled(cancel)?;
        let pending = context.pending_resource_counts();
        if pending.commands != 0
            || pending.stagings != 0
            || pending.buffers != 0
            || pending.textures != 0
        {
            return Err(error(
                ErrorCode::InvalidArgument,
                "pending_queue",
                "projection tiles require an empty unsubmitted queue",
            ));
        }
        context.poll_host_lost();
        context.device_handle()?;
        if !context.backend_is_host() {
            return Err(error(
                ErrorCode::UnimplementedOption,
                "gpu_context",
                "projection tiles need a real GPU",
            ));
        }
        let result = execute(context, request, options, tx, cancel);
        // Admission above and exclusive &mut guarantee that this removes only
        // this operation's unsubmitted commands. Accepted work stays context-owned.
        if result.is_err() {
            context.abandon_unsubmitted_pending();
        }
        result
    }
}
fn cancelled(token: &CancelToken) -> Result<(), DomainError> {
    if token.is_cancelled() {
        Err(error(
            ErrorCode::Canceled,
            "projection_tiles",
            "tile build cancelled",
        ))
    } else {
        Ok(())
    }
}
fn label(name: &str) -> ResourceLabel {
    ResourceLabel::parse(&format!("spirula:render:buffer:{name}")).expect("static tile label")
}
fn buffer(
    ctx: &mut GpuContext,
    name: &str,
    bytes: u64,
    usage: BufferUsage,
) -> Result<BufferHandle, DomainError> {
    ctx.create_buffer(BufferDesc {
        label: label(name),
        size: bytes,
        usage,
    })
}
fn pipeline(ctx: &mut GpuContext, entry: &str) -> Result<ComputePipelineHandle, DomainError> {
    ctx.obtain_compute_pipeline(ComputePipelineDesc {
        source: SOURCE.to_owned(),
        entry_point: entry.to_owned(),
        spec: SpecConstants::empty(),
        bind_layout_version: 1,
        label: ResourceLabel::parse(&format!("spirula:render:pipeline:tiles-{entry}"))
            .expect("pipeline label"),
    })
}
fn params(ctx: &mut GpuContext, n: u32, stride: u32, tx: u32) -> Result<BufferHandle, DomainError> {
    let handle = buffer(ctx, "tile-params", 16, BufferUsage::UNIFORM_UPLOAD)?;
    let bytes: Vec<u8> = [n, stride, tx, 0]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    ctx.upload_buffer(&handle, 0, &bytes)?;
    Ok(handle)
}
fn preflight(
    ctx: &GpuContext,
    options: TileBuildOptions,
    additional: u64,
) -> Result<(), DomainError> {
    let snapshot = ctx.resource_accounting();
    if additional > options.max_gpu_bytes
        || snapshot
            .current_bytes
            .checked_add(additional)
            .is_none_or(|v| snapshot.budget_limit.is_some_and(|b| v > b))
    {
        return Err(error(
            ErrorCode::AllocationFailed,
            "gpu_budget",
            "projection-tile peak exceeds explicit/accounted GPU budget",
        ));
    }
    Ok(())
}
fn wait_map(
    ctx: &mut GpuContext,
    ticket: &spirula_gpu::ReadbackTicket,
) -> Result<Vec<u8>, DomainError> {
    let submission = ctx.submit()?;
    // Bounded first wait; accepted resources remain retained by context on a
    // timeout, never released while device work may still reference them.
    ctx.wait(submission, Some(Duration::from_secs(30)))?;
    ctx.map_readback(ticket)
}
fn execute(
    ctx: &mut GpuContext,
    request: &RenderRequest,
    options: TileBuildOptions,
    tx: u32,
    cancel: &CancelToken,
) -> Result<(TileBins, TileBuildMetrics), DomainError> {
    let n = request.scene.len() as u32;
    let steps = if n <= 1 {
        0
    } else {
        32 - (n - 1).leading_zeros()
    };
    let record_bytes = u64::from(n) * 64;
    let first_peak = record_bytes
        .checked_mul(5)
        .and_then(|v| v.checked_add(256 + 32 * u64::from(steps + 1)))
        .ok_or_else(|| {
            error(
                ErrorCode::ValueOutOfRange,
                "gpu_bytes",
                "projection byte count overflow",
            )
        })?;
    preflight(ctx, options, first_peak)?;
    let report = ctx
        .report()
        .ok_or_else(|| error(ErrorCode::DeviceLost, "context", "missing device"))?;
    let max_storage = report.device_limits.max_storage_buffer_binding_size;
    if record_bytes + 128 > max_storage
        || n.div_ceil(64) > report.device_limits.max_compute_workgroups_per_dimension
    {
        return Err(error(
            ErrorCode::NoAdapter,
            "gpu_limits",
            "projection input or dispatch exceeds enabled limits",
        ));
    }
    let adapter = format!("{:?}/{}", report.backend, report.adapter.identity.name);
    let mut input = Vec::<u8>::with_capacity((record_bytes + 128) as usize);
    let mut header = [0f64; 32];
    header[..9].copy_from_slice(&request.camera.pose().rotation().transpose().row_major());
    header[9..12].copy_from_slice(&request.camera.pose().translation().to_array());
    header[12..16].copy_from_slice(&request.camera.intrinsics());
    header[16] = f64::from(request.camera.width());
    header[17] = f64::from(request.camera.height());
    header[18] = f64::from(options.tile_size);
    header[19] = request.thresholds.alpha_cutoff;
    for value in header {
        if value != 0.0 && (value as f32) == 0.0 {
            return Err(error(
                ErrorCode::ValueOutOfRange,
                "camera",
                "nonzero camera input underflows f32",
            ));
        }
        let value = value as f32;
        if !value.is_finite() {
            return Err(error(
                ErrorCode::ValueOutOfRange,
                "camera",
                "camera not representable in f32",
            ));
        }
        input.extend_from_slice(&value.to_le_bytes());
    }
    if (request.thresholds.alpha_cutoff as f32) <= 0.0 {
        return Err(error(
            ErrorCode::ValueOutOfRange,
            "alpha_cutoff",
            "cutoff underflows f32",
        ));
    }
    for splat in request.scene.splats() {
        for value in splat
            .mean()
            .to_array()
            .into_iter()
            .chain(splat.scale_log().to_array())
            .chain(splat.rotation().wxyz())
        {
            if value != 0.0 && (value as f32) == 0.0 {
                return Err(error(
                    ErrorCode::ValueOutOfRange,
                    "splat_value",
                    "nonzero projection input underflows f32",
                ));
            }
        }
    }
    let packed = PackedSplatData::from_scene(&request.scene)?;
    if packed.splat_count != n as usize || packed.bytes.len() != n as usize * 64 {
        return Err(error(
            ErrorCode::IntegrityMismatch,
            "splat_bytes",
            "splat count/byte length mismatch",
        ));
    }
    input.extend_from_slice(&packed.bytes);
    drop(packed);
    let project_pipe = pipeline(ctx, "project")?;
    let scan_pipe = pipeline(ctx, "scan")?;
    let emit_pipe = pipeline(ctx, "emit")?;
    let source = buffer(
        ctx,
        "tile-input",
        input.len() as u64,
        BufferUsage::STORAGE_COPY,
    )?;
    let mut a = buffer(ctx, "tile-scan-a", record_bytes, BufferUsage::STORAGE_COPY)?;
    let mut b = buffer(ctx, "tile-scan-b", record_bytes, BufferUsage::STORAGE_COPY)?;
    let encode = Instant::now();
    ctx.upload_buffer(&source, 0, &input)?;
    let uniform = params(ctx, n, 0, tx)?;
    ctx.dispatch_projection_tiles(
        &project_pipe,
        &uniform,
        &source,
        &a,
        input.len() as u64,
        record_bytes,
        n,
    )?;
    let mut uniforms = vec![uniform];
    let mut stride = 1u32;
    while stride < n {
        cancelled(cancel)?;
        let uniform = params(ctx, n, stride, tx)?;
        ctx.dispatch_projection_tiles(&scan_pipe, &uniform, &a, &b, record_bytes, record_bytes, n)?;
        uniforms.push(uniform);
        std::mem::swap(&mut a, &mut b);
        stride *= 2;
    }
    let projection_scan_encode_ns = encode.elapsed().as_nanos();
    let sync = Instant::now();
    let ticket = ctx.readback_buffer(&a, 0, record_bytes)?;
    let raw = wait_map(ctx, &ticket)?;
    let count_sync_ns = sync.elapsed().as_nanos();
    drop(ticket);
    drop(b);
    drop(source);
    drop(uniforms);
    drop(input);
    cancelled(cancel)?;
    let words: Vec<u32> = raw
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| u32::from_le_bytes(*v))
        .collect();
    drop(raw);
    let mut projected = Vec::with_capacity(n as usize);
    let mut offsets = Vec::with_capacity(n as usize);
    for (i, r) in words.as_chunks::<RECORD_WORDS>().0.iter().enumerate() {
        if r[15] > 1 {
            return Err(error(
                ErrorCode::ValueOutOfRange,
                "gpu_projection",
                "GPU projection produced a nonfinite/invalid record",
            ));
        }
        offsets.push(r[14]);
        let f = |k: usize| f64::from(f32::from_bits(r[k]));
        projected.push(if r[15] == 0 {
            None
        } else {
            Some(ProjectedTileSplat {
                source_index: i,
                pixel: [f(0), f(1)],
                depth: f(2),
                ray_depth: f(3),
                radius_px: [f(4), f(5)],
                covariance: [f(6), f(7), f(8)],
                opacity: f(9),
                tile_bounds: [r[10], r[11], r[12], r[13]],
            })
        });
    }
    let total = u64::from(*offsets.last().expect("nonempty render scene"));
    options.pair_preflight(total, n as usize)?;
    let second_peak = record_bytes + total * 16 + 32;
    if second_peak > options.max_gpu_bytes {
        return Err(error(
            ErrorCode::AllocationFailed,
            "gpu_budget",
            "emission operation peak exceeds explicit GPU budget",
        ));
    }
    preflight(ctx, options, second_peak - record_bytes)?;
    if total * 8 > max_storage {
        return Err(error(
            ErrorCode::NoAdapter,
            "gpu_limits",
            "intersection storage exceeds enabled limits",
        ));
    }
    let mut pairs = Vec::with_capacity(total as usize);
    let emission = Instant::now();
    if total > 0 {
        let out = buffer(ctx, "tile-pairs", total * 8, BufferUsage::STORAGE_COPY)?;
        let uniform = params(ctx, n, 0, tx)?;
        ctx.dispatch_projection_tiles(&emit_pipe, &uniform, &a, &out, record_bytes, total * 8, n)?;
        let ticket = ctx.readback_buffer(&out, 0, total * 8)?;
        let bytes = wait_map(ctx, &ticket)?;
        for pair in bytes.as_chunks::<8>().0 {
            pairs.push(TilePair {
                tile_id: u32::from_le_bytes(pair[..4].try_into().expect("tile")) as usize,
                source_index: u32::from_le_bytes(pair[4..].try_into().expect("source")) as usize,
            });
        }
    }
    let emission_sync_ns = emission.elapsed().as_nanos();
    cancelled(cancel)?;
    let bins = finish(
        request.camera.width(),
        request.camera.height(),
        options,
        projected,
        offsets,
        pairs,
    )?;
    Ok((
        bins,
        TileBuildMetrics {
            intersection_peak: total,
            scan_scratch_bytes: record_bytes * 2,
            accounted_peak_bound_bytes: first_peak.max(second_peak),
            projection_scan_encode_ns,
            count_sync_ns,
            emission_sync_ns,
            dispatch_count: 1 + steps + u32::from(total > 0),
            adapter,
        },
    ))
}
