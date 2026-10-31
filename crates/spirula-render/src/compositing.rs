//! Front-to-back color/transmittance compositing and ray/median depth (P4-F05).
//!
//! C++ reference (pinned `9a24f099…`):
//! - SH0: `harmonics.slang:1056-1067` / `generated/harmonics.cuh:133-135`
//!   `max(SH_C0 * dc + 0.5, 0)` — lower clamp only; upper may exceed 1.
//!   `GaussianSplat::base_color` is **not** the compositing truth (no clamp).
//! - Raster: `RasterizationEval3DFwd_kernel.cuh:145-322` — median half-crossing
//!   before early-out (`next_T < 0.5` strict); terminating splat not accumulated;
//!   expected depth is unconditional `depth_acc / max(1-T, 1e-10)`.
use crate::sort_raster::coverage_alpha;
use crate::tiles::error;
use crate::{
    CpuSortRaster, CpuTileBuilder, PixelCoverage, ProjectedTileSplat, RenderRequest,
    SortRasterOptions, SortedRaster, SortedTilePair, TileBins, TileBuildOptions, TileCoverage,
    TilePair, TileRange,
};
use spirula_core::{DomainError, ErrorCode, SH_C0};
use std::mem::size_of;

pub const COMPOSITE_CONTRACT_VERSION: u32 = 1;

/// SH0 → linear RGB for compositing. Matches C++ splat path, not `base_color`.
pub fn sh0_to_color(dc: [f64; 3]) -> Result<[f64; 3], DomainError> {
    let mut out = [0.0; 3];
    for i in 0..3 {
        if !dc[i].is_finite() {
            return Err(range("dc"));
        }
        let c = (dc[i] * SH_C0 + 0.5).max(0.0);
        if !c.is_finite() {
            return Err(range("sh0_color"));
        }
        out[i] = c;
    }
    Ok(out)
}

/// Pack f64→f32 bits. Exact `0` is allowed; nonzero underflow to `0` is rejected.
pub(crate) fn pack_f32(v: f64) -> Result<u32, DomainError> {
    let f = v as f32;
    if !v.is_finite() || !f.is_finite() {
        return Err(range("f32_pack"));
    }
    if v != 0.0 && f == 0.0 {
        return Err(range("f32_underflow"));
    }
    Ok(f.to_bits())
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BackgroundPolicy {
    Clear,
    Constant([f64; 3]),
}

impl BackgroundPolicy {
    pub fn rgb(self) -> [f64; 3] {
        match self {
            Self::Clear => [0.0, 0.0, 0.0],
            Self::Constant(rgb) => rgb,
        }
    }

    pub(crate) fn validate(self) -> Result<(), DomainError> {
        match self {
            Self::Clear => Ok(()),
            Self::Constant(rgb) => {
                if rgb.iter().any(|c| !c.is_finite()) {
                    return Err(error(
                        ErrorCode::ValueOutOfRange,
                        "background",
                        "background RGB must be finite",
                    ));
                }
                Ok(())
            }
        }
    }

    /// Reject background channels that cannot survive f32 packing (nonzero→0).
    pub(crate) fn preflight_f32(self) -> Result<(), DomainError> {
        self.validate()?;
        for c in self.rgb() {
            pack_f32(c)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompositeOptions {
    pub max_pairs: u64,
    pub max_tiles: u64,
    pub max_pixels: u64,
    pub max_host_bytes: u64,
    pub max_gpu_bytes: u64,
    pub tile: TileBuildOptions,
    pub sort: SortRasterOptions,
    pub background: BackgroundPolicy,
}

impl Default for CompositeOptions {
    fn default() -> Self {
        Self {
            max_pairs: 1_000_000,
            max_tiles: 1_000_000,
            max_pixels: 4_000_000,
            max_host_bytes: 512 << 20,
            max_gpu_bytes: 512 << 20,
            tile: TileBuildOptions::default(),
            sort: SortRasterOptions::default(),
            background: BackgroundPolicy::Clear,
        }
    }
}

impl CompositeOptions {
    /// Clamp nested tile/sort caps to this call's top-level budgets (strict min).
    pub(crate) fn capped(self) -> Self {
        let mut out = self;
        out.tile.max_pairs = out.tile.max_pairs.min(self.max_pairs);
        out.tile.max_tiles = out.tile.max_tiles.min(self.max_tiles);
        out.tile.max_host_bytes = out.tile.max_host_bytes.min(self.max_host_bytes);
        out.tile.max_gpu_bytes = out.tile.max_gpu_bytes.min(self.max_gpu_bytes);
        out.sort.max_pairs = out.sort.max_pairs.min(self.max_pairs);
        out.sort.max_tiles = out.sort.max_tiles.min(self.max_tiles);
        out.sort.max_pixels = out.sort.max_pixels.min(self.max_pixels);
        out.sort.max_host_bytes = out.sort.max_host_bytes.min(self.max_host_bytes);
        out.sort.max_gpu_bytes = out.sort.max_gpu_bytes.min(self.max_gpu_bytes);
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompositePixel {
    pub rgb: [f64; 3],
    pub transmittance: f64,
    pub alpha: f64,
    /// Expected arithmetic ray depth via `depth_acc / max(1-T, 1e-10)`.
    /// Tiny non-zero `vis` with `T` rounded to 1 must **not** clear to 0.
    pub expected_depth: f64,
    /// Median ray depth; virtual prev `(z=0,T=1)`; strict `next_T < 0.5`.
    pub median_depth: f64,
}

impl Default for CompositePixel {
    fn default() -> Self {
        Self {
            rgb: [0.0, 0.0, 0.0],
            transmittance: 1.0,
            alpha: 0.0,
            expected_depth: 0.0,
            median_depth: 0.0,
        }
    }
}

impl CompositePixel {
    fn from_raw(rgb: [f64; 3], t: f64, expected: f64, median: f64) -> Result<Self, DomainError> {
        let pixel = Self {
            rgb,
            transmittance: t,
            alpha: 1.0 - t,
            expected_depth: expected,
            median_depth: median,
        };
        pixel.validate_finite()?;
        Ok(pixel)
    }

    pub(crate) fn validate_finite(self) -> Result<(), DomainError> {
        if self.rgb.iter().any(|c| !c.is_finite())
            || !self.transmittance.is_finite()
            || !self.alpha.is_finite()
            || !self.expected_depth.is_finite()
            || !self.median_depth.is_finite()
        {
            return Err(range("pixel_nonfinite"));
        }
        if !(0.0..=1.0).contains(&self.transmittance) {
            return Err(range("transmittance"));
        }
        if (self.alpha - (1.0 - self.transmittance)).abs() > 0.0 {
            return Err(range("alpha"));
        }
        if self.expected_depth < 0.0 || self.median_depth < 0.0 {
            return Err(range("depth_sign"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompositeImage {
    pub contract_version: u32,
    pub width: u32,
    pub height: u32,
    pub alpha_cutoff: f64,
    pub effective_alpha_cutoff: f32,
    pub early_out_transmittance: f64,
    pub effective_early_out: f32,
    pub background: BackgroundPolicy,
    pub pixels: Vec<CompositePixel>,
}

pub struct CpuCompositor;

impl CpuCompositor {
    /// Empty / no-candidate image via direct host clear (CPU path only).
    ///
    /// Validates `tile_size` (no silent `max(1)`). Does **not** call sort with
    /// cutoff 0. Metadata stores the provided positive cutoffs. Constant
    /// background fills `rgb=bg`, `T=1`, depths 0.
    ///
    /// GPU clear is [`crate::GpuCompositor::compose_empty`] — a real shader
    /// write of every pixel; this CPU helper is not a GPU clear substitute.
    pub fn compose_empty(
        width: u32,
        height: u32,
        alpha_cutoff: f64,
        early_out_transmittance: f64,
        options: CompositeOptions,
    ) -> Result<CompositeImage, DomainError> {
        let options = options.capped();
        options.background.validate()?;
        options.background.preflight_f32()?;
        validate_cutoff(alpha_cutoff)?;
        validate_early_out(early_out_transmittance)?;
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
        // Empty peak: composite output only (+slack). No upstream Vecs.
        let peak = host_peak_bound(0, 0, t, m)?;
        if peak > options.max_host_bytes {
            return Err(range("host_bytes"));
        }
        if options.max_gpu_bytes == 0 {
            return Err(range("gpu_bytes"));
        }
        let bg = options.background.rgb();
        let pixel = CompositePixel::from_raw(bg, 1.0, 0.0, 0.0)?;
        Ok(CompositeImage {
            contract_version: COMPOSITE_CONTRACT_VERSION,
            width,
            height,
            alpha_cutoff,
            effective_alpha_cutoff: alpha_cutoff as f32,
            early_out_transmittance,
            effective_early_out: early_out_transmittance as f32,
            background: options.background,
            pixels: vec![pixel; m as usize],
        })
    }

    /// Public entry: re-validate `request`, chain CPU tiles → sort → composite.
    /// Prepared `(bins, sorted, colors)` stays **crate-private**.
    pub fn compose(
        request: &RenderRequest,
        options: CompositeOptions,
    ) -> Result<CompositeImage, DomainError> {
        request.validate()?;
        let mut options = options.capped();
        options.background.validate()?;
        options.background.preflight_f32()?;
        let cutoff = request.thresholds.alpha_cutoff;
        let early_out = request.thresholds.early_out_transmittance;
        validate_cutoff(cutoff)?;
        validate_early_out(early_out)?;
        // Allocation-free SH0/f32 preflight — no 24N color Vec yet.
        preflight_scene_colors_f32(request)?;
        pack_f32(cutoff)?;
        pack_f32(early_out)?;

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
        // Worst-case pairs for preflight only — not a source-count cap.
        let p_upper = n64
            .checked_mul(t)
            .ok_or_else(|| range("pairs"))?
            .min(options.max_pairs);
        let peak = host_peak_bound(n64, p_upper, t, m)?;
        if peak > options.max_host_bytes {
            return Err(range("host_bytes"));
        }

        let bins = CpuTileBuilder::build(request, options.tile)?;
        let p = bins.pairs.len() as u64;
        if p > options.max_pairs {
            return Err(range("pairs"));
        }
        let bins_bytes = bins_payload_bytes(&bins)?;
        // Nested sort deducts caller-owned bins (colors not allocated yet).
        options.sort.max_host_bytes = options
            .max_host_bytes
            .saturating_sub(bins_bytes)
            .min(options.sort.max_host_bytes);
        let mid_peak = host_peak_bound_with_bins(&bins, p, t, m)?;
        if mid_peak > options.max_host_bytes {
            return Err(range("host_bytes"));
        }

        let sorted = CpuSortRaster::build(&bins, cutoff, options.sort)?;
        // Colors collected only after F04; reserved in entry peak via during_comp.
        let colors = collect_scene_sh0_colors(request)?;
        compose_from_sorted(&bins, &sorted, &colors, cutoff, early_out, options)
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

/// Walk scene SH0 without allocating a color table.
pub(crate) fn preflight_scene_colors_f32(request: &RenderRequest) -> Result<(), DomainError> {
    for splat in request.scene.splats() {
        let c = sh0_to_color(splat.dc().to_array())?;
        for ch in c {
            pack_f32(ch)?;
        }
    }
    Ok(())
}

pub(crate) fn collect_scene_sh0_colors(
    request: &RenderRequest,
) -> Result<Vec<[f64; 3]>, DomainError> {
    // Exact capacity=N (not Result<Vec>::collect growth). Host 24N peak assumes this.
    let mut colors = Vec::with_capacity(request.scene.len());
    for splat in request.scene.splats() {
        colors.push(sh0_to_color(splat.dc().to_array())?);
    }
    Ok(colors)
}

fn checked_add(a: u64, b: u64) -> Result<u64, DomainError> {
    a.checked_add(b).ok_or_else(|| range("host_bytes"))
}

fn checked_mul(a: u64, b: u64) -> Result<u64, DomainError> {
    a.checked_mul(b).ok_or_else(|| range("host_bytes"))
}

/// Len-based bins body; pre-build estimates multiply by 2 for Vec capacity growth.
fn bins_owned_len_bytes(n: u64, p: u64) -> Result<u64, DomainError> {
    let per_n = (size_of::<Option<ProjectedTileSplat>>()
        + size_of::<TileCoverage>()
        + size_of::<u32>()) as u64;
    checked_add(
        checked_add(
            checked_mul(n, per_n)?,
            checked_mul(p, size_of::<TilePair>() as u64)?,
        )?,
        256,
    )
}

/// Actual TileBins Vec capacity payload (not len).
pub(crate) fn bins_payload_bytes(bins: &TileBins) -> Result<u64, DomainError> {
    let n = bins.projected.capacity() as u64;
    let p = bins.pairs.capacity() as u64;
    let cov = bins.coverage.capacity() as u64;
    let off = bins.inclusive_offsets.capacity() as u64;
    checked_add(
        checked_add(
            checked_add(
                checked_mul(n, size_of::<Option<ProjectedTileSplat>>() as u64)?,
                checked_mul(cov, size_of::<TileCoverage>() as u64)?,
            )?,
            checked_add(
                checked_mul(off, size_of::<u32>() as u64)?,
                checked_mul(p, size_of::<TilePair>() as u64)?,
            )?,
        )?,
        256,
    )
}

fn sorted_owned_bytes(p: u64, t: u64, m: u64) -> Result<u64, DomainError> {
    checked_add(
        checked_add(
            checked_add(
                checked_mul(p, size_of::<SortedTilePair>() as u64)?,
                checked_mul(t, size_of::<TileRange>() as u64)?,
            )?,
            checked_mul(m, size_of::<PixelCoverage>() as u64)?,
        )?,
        256,
    )
}

/// F03 `TileBuildOptions::host_preflight` shape: `256N + 32P + 4096`.
fn f03_host_preflight_bound(n: u64, p: u64) -> Result<u64, DomainError> {
    checked_add(
        checked_add(checked_mul(n, 256)?, checked_mul(p, 32)?)?,
        4096,
    )
}

/// F04 sort host preflight bound.
fn sort_host_preflight_bound(n: u64, p: u64, t: u64, m: u64) -> Result<u64, DomainError> {
    checked_add(
        checked_add(
            checked_add(
                checked_add(checked_mul(p, 80)?, checked_mul(n, 128)?)?,
                checked_mul(t, 24)?,
            )?,
            checked_mul(m, 48)?,
        )?,
        8192,
    )
}

/// Composite-stage temps after colors are (or will be) allocated:
/// colors + two pack copies + readback + output + slack.
fn composite_stage_temp_bytes(n: u64, p: u64, t: u64, m: u64) -> Result<u64, DomainError> {
    let colors = checked_mul(n, size_of::<[f64; 3]>() as u64)?;
    let pack = checked_add(
        checked_add(
            checked_add(checked_mul(n, 64)?, checked_mul(p, 16)?)?,
            checked_mul(t, 8)?,
        )?,
        64,
    )?;
    let readback = checked_mul(m, 32)?;
    let output = checked_mul(m, size_of::<CompositePixel>() as u64)?;
    checked_add(
        checked_add(
            checked_add(checked_add(colors, checked_mul(pack, 2)?)?, readback)?,
            output,
        )?,
        8192,
    )
}

/// Full-call **stage peak** with colors deferred until after F04.
///
/// - `during_f03 = F03 host_preflight (256N+32P+4096)`
/// - `during_sort = 2×bins_len + sort_preflight` (no color Vec yet)
/// - `during_comp = 2×bins_len + sorted + colors+pack×2+readback+output`
///
/// Excludes caller scene, allocator metadata, driver, RSS.
pub(crate) fn host_peak_bound(n: u64, p: u64, t: u64, m: u64) -> Result<u64, DomainError> {
    let f03 = f03_host_preflight_bound(n, p)?;
    let bins = checked_mul(bins_owned_len_bytes(n, p)?, 2)?;
    let sorted = sorted_owned_bytes(p, t, m)?;
    let sort_pf = sort_host_preflight_bound(n, p, t, m)?;
    let comp = composite_stage_temp_bytes(n, p, t, m)?;
    let during_sort = checked_add(bins, sort_pf)?;
    let during_comp = checked_add(checked_add(bins, sorted)?, comp)?;
    Ok(f03.max(during_sort).max(during_comp))
}

pub(crate) fn host_peak_bound_with_bins(
    bins: &TileBins,
    p: u64,
    t: u64,
    m: u64,
) -> Result<u64, DomainError> {
    let n = bins.projected.len() as u64;
    let f03 = f03_host_preflight_bound(n, p)?;
    let bins_b = bins_payload_bytes(bins)?;
    let sorted = sorted_owned_bytes(p, t, m)?;
    let sort_pf = sort_host_preflight_bound(n, p, t, m)?;
    let comp = composite_stage_temp_bytes(n, p, t, m)?;
    let during_sort = checked_add(bins_b, sort_pf)?;
    let during_comp = checked_add(checked_add(bins_b, sorted)?, comp)?;
    Ok(f03.max(during_sort).max(during_comp))
}

/// Pack input byte size (one copy) for GPU budget math.
pub(crate) fn composite_pack_bytes(n: u64, p: u64, t: u64) -> Result<u64, DomainError> {
    checked_add(
        checked_add(
            checked_add(checked_mul(n, 64)?, checked_mul(p, 16)?)?,
            checked_mul(t, 8)?,
        )?,
        64,
    )
}

/// Accounted F05 GPU stage peak: `2*pack + 2*(32M) + 4096`.
///
/// `P=0` is the known lower bound before F03 (`2*(64+64N+8T)+64M+4096`).
pub(crate) fn composite_gpu_peak_bytes(n: u64, p: u64, t: u64, m: u64) -> Result<u64, DomainError> {
    let input = composite_pack_bytes(n, p, t)?;
    let output = checked_mul(m, 32)?;
    checked_add(
        checked_add(checked_mul(input, 2)?, checked_mul(output, 2)?)?,
        4096,
    )
}

pub(crate) fn compose_from_sorted(
    bins: &TileBins,
    sorted: &SortedRaster,
    colors: &[[f64; 3]],
    cutoff: f64,
    early_out: f64,
    options: CompositeOptions,
) -> Result<CompositeImage, DomainError> {
    bins.validate()?;
    sorted.validate_against(bins)?;
    validate_cutoff(cutoff)?;
    validate_early_out(early_out)?;
    if colors.len() != bins.projected.len() {
        return Err(error(
            ErrorCode::InvalidArgument,
            "colors",
            "color count must match projected source slots",
        ));
    }
    let m = u64::from(sorted.width)
        .checked_mul(u64::from(sorted.height))
        .ok_or_else(|| range("pixels"))?;
    let t = u64::from(bins.tiles_x) * u64::from(bins.tiles_y);
    let n = bins.projected.len() as u64;
    let p = sorted.pairs.len() as u64;
    if m > options.max_pixels || t > options.max_tiles || p > options.max_pairs {
        return Err(range("budget"));
    }
    if host_peak_bound(n, p, t, m)? > options.max_host_bytes {
        return Err(range("host_bytes"));
    }

    let bg = options.background.rgb();
    let mut pixels = Vec::with_capacity(m as usize);
    for y in 0..sorted.height {
        for x in 0..sorted.width {
            pixels.push(composite_pixel(
                bins, sorted, colors, x, y, cutoff, early_out, bg,
            )?);
        }
    }
    Ok(CompositeImage {
        contract_version: COMPOSITE_CONTRACT_VERSION,
        width: sorted.width,
        height: sorted.height,
        alpha_cutoff: cutoff,
        effective_alpha_cutoff: cutoff as f32,
        early_out_transmittance: early_out,
        effective_early_out: early_out as f32,
        background: options.background,
        pixels,
    })
}

#[allow(clippy::too_many_arguments)]
fn composite_pixel(
    bins: &TileBins,
    sorted: &SortedRaster,
    colors: &[[f64; 3]],
    x: u32,
    y: u32,
    cutoff: f64,
    early_out: f64,
    background: [f64; 3],
) -> Result<CompositePixel, DomainError> {
    let tile = (y / sorted.tile_size) * bins.tiles_x + (x / sorted.tile_size);
    let tile_range = sorted.tile_ranges[tile as usize];
    let end = (tile_range.start + tile_range.count) as usize;
    let px = [f64::from(x) + 0.5, f64::from(y) + 0.5];

    let mut t = 1.0_f64;
    let mut rgb = [0.0_f64; 3];
    let mut depth_acc = 0.0_f64;
    let mut median = 0.0_f64;
    let mut median_prev_z = 0.0_f64;
    let mut median_prev_t = 1.0_f64;
    let mut done = false;

    for pair in &sorted.pairs[tile_range.start as usize..end] {
        if done {
            break;
        }
        let projected = bins.projected[pair.source_index as usize]
            .as_ref()
            .ok_or_else(|| range("pair_source"))?;
        let alpha = coverage_alpha(*projected, px)?;
        if alpha <= cutoff {
            continue;
        }
        let color = colors[pair.source_index as usize];
        let ray_z = projected.ray_depth;
        if !ray_z.is_finite() || ray_z <= 0.0 {
            return Err(range("ray_depth"));
        }
        let next_t = t * (1.0 - alpha);
        if !next_t.is_finite() {
            return Err(range("transmittance"));
        }

        if median_prev_t >= 0.5 && next_t < 0.5 {
            let a = median_prev_t.ln();
            let b = next_t.ln();
            let c = (0.5_f64).ln();
            let d = b - a;
            let f = if d.abs() > 1e-20 { (c - a) / d } else { 0.0 };
            median = median_prev_z + (ray_z - median_prev_z) * f;
        }
        median_prev_t = next_t;
        median_prev_z = ray_z;

        if next_t > early_out {
            let vis = alpha * t;
            rgb[0] += color[0] * vis;
            rgb[1] += color[1] * vis;
            rgb[2] += color[2] * vis;
            depth_acc += ray_z * vis;
            t = next_t;
        } else {
            done = true;
        }
    }

    let weight = 1.0 - t;
    let expected = depth_acc / weight.max(1e-10);
    CompositePixel::from_raw(
        [
            rgb[0] + t * background[0],
            rgb[1] + t * background[1],
            rgb[2] + t * background[2],
        ],
        t,
        expected,
        median,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ColorSpace, RenderPrimitive, RenderThresholds};
    use spirula_core::{
        CameraPose, GaussianSplat, OpenCvDistortion, PinholeCamera, Quat, SplatSet, Vec3,
    };

    #[test]
    fn empty_rejects_zero_tile_size() {
        let options = CompositeOptions {
            tile: TileBuildOptions {
                tile_size: 0,
                ..TileBuildOptions::default()
            },
            ..CompositeOptions::default()
        };
        let err = CpuCompositor::compose_empty(2, 1, 1e-4, 1e-4, options).unwrap_err();
        assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
    }

    #[test]
    fn pack_f32_rejects_nonzero_underflow() {
        let tiny = 1e-50_f64;
        assert_ne!(tiny, 0.0);
        assert_eq!(tiny as f32, 0.0);
        assert!(pack_f32(tiny).is_err());
        assert!(pack_f32(0.0).is_ok());
    }

    #[test]
    fn empty_constant_background_and_positive_cutoff_metadata() {
        let options = CompositeOptions {
            background: BackgroundPolicy::Constant([0.1, 0.2, 0.3]),
            ..CompositeOptions::default()
        };
        let img = CpuCompositor::compose_empty(2, 1, 1e-4, 1e-4, options).unwrap();
        assert_eq!(img.alpha_cutoff, 1e-4);
        assert_eq!(img.pixels[0].rgb, [0.1, 0.2, 0.3]);
        assert_eq!(img.pixels[0].transmittance, 1.0);
        assert_eq!(img.pixels[0].expected_depth, 0.0);
    }

    #[test]
    fn culled_behind_camera_p0_succeeds_under_pair_budget() {
        // z>0 → depth=-z≤0 → fully culled. N>max_pairs must still succeed (P=0).
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
        let splats = (0..8)
            .map(|i| {
                GaussianSplat::try_new(
                    Vec3::try_new(0.0, 0.0, 2.0 + i as f64 * 0.01).unwrap(),
                    Quat::identity(),
                    Vec3::try_new(0.0, 0.0, 0.0).unwrap(),
                    0.0,
                    Vec3::try_new(0.0, 0.0, 0.0).unwrap(),
                )
                .unwrap()
            })
            .collect();
        let request = RenderRequest::try_new(
            camera,
            SplatSet::try_from_splats(splats).unwrap(),
            RenderPrimitive::Gaussian3d,
            ColorSpace::LinearRgb,
            0,
            RenderThresholds::default(),
        )
        .unwrap();
        let options = CompositeOptions {
            max_pairs: 4,
            ..CompositeOptions::default()
        };
        let img = CpuCompositor::compose(&request, options).unwrap();
        assert_eq!(img.pixels.len(), 1);
        assert_eq!(img.pixels[0].transmittance, 1.0);
        assert_eq!(img.pixels[0].expected_depth, 0.0);
        assert_eq!(img.pixels[0].median_depth, 0.0);
    }

    #[test]
    fn color_table_capacity_equals_len_for_power_of_two_growth_sizes() {
        // Regression: Result<Vec>::collect grew N=10000→16384; 24N host peak
        // assumes exact capacity=N from with_capacity+push.
        for n in [65usize, 10_000] {
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
                        Vec3::try_new(0.0, 0.0, 2.0 + i as f64 * 1e-6).unwrap(),
                        Quat::identity(),
                        Vec3::try_new(0.0, 0.0, 0.0).unwrap(),
                        0.0,
                        Vec3::try_new(0.0, 0.0, 0.0).unwrap(),
                    )
                    .unwrap()
                })
                .collect();
            let request = RenderRequest::try_new(
                camera,
                SplatSet::try_from_splats(splats).unwrap(),
                RenderPrimitive::Gaussian3d,
                ColorSpace::LinearRgb,
                0,
                RenderThresholds::default(),
            )
            .unwrap();
            let colors = collect_scene_sh0_colors(&request).unwrap();
            assert_eq!(colors.len(), n);
            assert_eq!(
                colors.capacity(),
                n,
                "color Vec capacity must equal len for 24N host bound"
            );
        }
    }

    #[test]
    fn host_peak_p0_large_n_includes_f03_and_deferred_color_reservation() {
        let n = 10_000u64;
        let p = 0u64;
        let t = 1u64;
        let m = 1u64;
        let peak = host_peak_bound(n, p, t, m).unwrap();
        let f03 = f03_host_preflight_bound(n, p).unwrap();
        let bins2x = bins_owned_len_bytes(n, p).unwrap() * 2;
        let sort_pf = sort_host_preflight_bound(n, p, t, m).unwrap();
        let during_sort = bins2x + sort_pf; // no colors — deferred
        let colors = n * size_of::<[f64; 3]>() as u64;
        let during_comp = bins2x
            + sorted_owned_bytes(p, t, m).unwrap()
            + composite_stage_temp_bytes(n, p, t, m).unwrap();
        assert!(peak >= f03, "missing F03 bound peak={peak} f03={f03}");
        assert!(peak >= during_sort, "missing sort stage");
        assert!(peak >= during_comp, "missing composite stage");
        // Sort-stage-only accounting would miss the deferred color reservation
        // that still appears in the whole-call max via during_comp.
        assert!(
            peak >= during_sort + colors || peak >= during_comp,
            "peak={peak} during_sort={during_sort} colors={colors} during_comp={during_comp}"
        );
        assert!(
            during_comp > during_sort,
            "colors must enlarge composite stage"
        );
    }

    #[test]
    fn tiny_host_budget_rejects_without_needing_color_table() {
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
            Vec3::try_new(0.0, 0.0, -2.0).unwrap(),
            Quat::identity(),
            Vec3::try_new(0.0, 0.0, 0.0).unwrap(),
            0.0,
            Vec3::try_new(0.0, 0.0, 0.0).unwrap(),
        )
        .unwrap();
        let request = RenderRequest::try_new(
            camera,
            SplatSet::try_from_splats(vec![splat]).unwrap(),
            RenderPrimitive::Gaussian3d,
            ColorSpace::LinearRgb,
            0,
            RenderThresholds::default(),
        )
        .unwrap();
        let options = CompositeOptions {
            max_host_bytes: 64,
            ..CompositeOptions::default()
        };
        let err = CpuCompositor::compose(&request, options).unwrap_err();
        assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
    }
}
