//! Stable canonical depth keys, tile ranges, and pre-compositing raster coverage.
//!
//! P4-F04 sorts P4-F03 source-major tile pairs by `(tile_id, depth_key)` using the
//! same f32 key bits the GPU radix path consumes. Raster coverage evaluates the
//! projected conic/alpha support only; transmittance, color and depth output are
//! P4-F05.
use crate::tiles::error;
use crate::{ProjectedTileSplat, TileBins};
use spirula_core::{DomainError, ErrorCode};

pub const SORT_RASTER_VERSION: u32 = 1;
/// Sentinel `first_source` for pixels with no supporting splat.
pub const NO_SOURCE: u32 = u32::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SortRasterOptions {
    pub max_pairs: u64,
    pub max_tiles: u64,
    pub max_pixels: u64,
    pub max_raster_tests: u64,
    pub max_host_bytes: u64,
    pub max_gpu_bytes: u64,
}

impl Default for SortRasterOptions {
    fn default() -> Self {
        Self {
            max_pairs: 1_000_000,
            max_tiles: 1_000_000,
            max_pixels: 4_000_000,
            max_raster_tests: 100_000_000,
            max_host_bytes: 256 << 20,
            max_gpu_bytes: 256 << 20,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SortedTilePair {
    pub tile_id: u32,
    pub depth_key: u32,
    pub source_index: u32,
    /// Index into the unsorted P4-F03 `TileBins::pairs` input order.
    pub input_index: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileRange {
    pub start: u32,
    pub count: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PixelCoverage {
    pub count: u32,
    pub first_source: u32,
    pub max_alpha: f64,
}

impl Default for PixelCoverage {
    fn default() -> Self {
        Self {
            count: 0,
            first_source: NO_SOURCE,
            max_alpha: 0.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SortedRaster {
    pub contract_version: u32,
    pub width: u32,
    pub height: u32,
    pub tile_size: u32,
    /// Caller-requested raster cutoff (f64). Identical for CPU/GPU and for
    /// empty/non-empty inputs; never silently replaced by an f32 round-trip.
    pub alpha_cutoff: f64,
    /// `alpha_cutoff as f32`: the value GPU coverage compares use after pack.
    /// CPU coverage compares use `alpha_cutoff` in f64. Recorded on every path
    /// so metadata stays consistent.
    pub effective_alpha_cutoff: f32,
    pub pairs: Vec<SortedTilePair>,
    pub tile_ranges: Vec<TileRange>,
    pub pixels: Vec<PixelCoverage>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RasterPlan {
    pub n: u32,
    pub p: u32,
    pub t: u32,
    pub m: u32,
    pub groups: u32,
    pub state_bytes: u64,
    pub input_bytes: u64,
    pub sort_readback_bytes: u64,
    pub first_gpu_peak: u64,
    pub second_gpu_peak: u64,
    pub raster_tests: u64,
}

pub(crate) fn range(field: &str) -> DomainError {
    error(
        ErrorCode::ValueOutOfRange,
        field,
        "sort/raster range or resource limit exceeded",
    )
}

/// Positive axial depth → abs f32 bits XOR `0x80000000` (IntersectTile.cu:228-235).
pub fn canonical_depth_key(depth: f64) -> Result<u32, DomainError> {
    let d = depth as f32;
    if !depth.is_finite() || depth <= 0.0 || !d.is_finite() || d <= 0.0 {
        return Err(range("depth_key"));
    }
    Ok(d.abs().to_bits() ^ 0x8000_0000)
}

impl SortRasterOptions {
    pub(crate) fn plan(self, bins: &TileBins, cutoff: f64) -> Result<RasterPlan, DomainError> {
        bins.validate().map_err(|err| {
            if bins.contract_version == 1 {
                error(
                    ErrorCode::UnsupportedVersion,
                    "contract_version",
                    "TileBins v1 was a non-persistent internal buffer without projection_alpha_cutoff; P4-F04 requires v2",
                )
            } else {
                err
            }
        })?;
        if !cutoff.is_finite() || cutoff <= 0.0 || cutoff > 1.0 {
            return Err(range("alpha_cutoff"));
        }
        let cutoff_f32 = cutoff as f32;
        if !cutoff_f32.is_finite() || cutoff_f32 <= 0.0 || (cutoff != 0.0 && cutoff_f32 == 0.0) {
            return Err(range("alpha_cutoff_f32"));
        }
        // Raster cutoff below the projection cutoff would silently miss support
        // outside the already-truncated P4-F03 candidate tiles.
        if cutoff < bins.projection_alpha_cutoff {
            return Err(range("projection_cutoff_mismatch"));
        }
        let n = bins.projected.len() as u64;
        let p = bins.pairs.len() as u64;
        let t = bins.tile_count() as u64;
        let m = u64::from(bins.width)
            .checked_mul(u64::from(bins.height))
            .ok_or_else(|| range("pixels"))?;
        if p > self.max_pairs
            || p > i32::MAX as u64
            || t > self.max_tiles
            || t > u32::MAX as u64
            || m > self.max_pixels
            || m > u32::MAX as u64
        {
            return Err(range("extent"));
        }
        for projected in bins.projected.iter().flatten() {
            canonical_depth_key(projected.depth)?;
        }
        let mut tests = 0u64;
        for pair in &bins.pairs {
            let x = (pair.tile_id as u32) % bins.tiles_x;
            let y = (pair.tile_id as u32) / bins.tiles_x;
            let w = bins
                .tile_size
                .min(bins.width.saturating_sub(x * bins.tile_size));
            let h = bins
                .tile_size
                .min(bins.height.saturating_sub(y * bins.tile_size));
            tests = tests
                .checked_add(
                    u64::from(w)
                        .checked_mul(u64::from(h))
                        .ok_or_else(|| range("raster_tests"))?,
                )
                .ok_or_else(|| range("raster_tests"))?;
        }
        if tests > self.max_raster_tests {
            return Err(range("raster_tests"));
        }
        let host = p
            .checked_mul(80)
            .and_then(|v| v.checked_add(n.checked_mul(128)?))
            .and_then(|v| v.checked_add(t.checked_mul(24)?))
            .and_then(|v| v.checked_add(m.checked_mul(48)?))
            .and_then(|v| v.checked_add(8192))
            .ok_or_else(|| range("host_bytes"))?;
        if host > self.max_host_bytes {
            return Err(range("host_bytes"));
        }
        let groups = p.div_ceil(64);
        let state = 32 + 16 * p + 64 * groups + 8 * t + 64 * n;
        let input = 32 + 64 * n + 8 * p;
        let readback = 16 * p + 8 * t;
        // Conservative accounted peaks: live ping-pong state, input (+upload
        // staging), uniforms, and sort/raster readback staging. Not VRAM.
        let first_gpu_peak = input
            .checked_mul(2)
            .and_then(|v| v.checked_add(state.checked_mul(2)?))
            .and_then(|v| v.checked_add(readback))
            .and_then(|v| v.checked_add(4096))
            .ok_or_else(|| range("gpu_bytes"))?;
        let second_gpu_peak = state
            .checked_add(32 * m)
            .and_then(|v| v.checked_add(256))
            .ok_or_else(|| range("gpu_bytes"))?;
        Ok(RasterPlan {
            n: n as u32,
            p: p as u32,
            t: t as u32,
            m: m as u32,
            groups: groups as u32,
            state_bytes: state,
            input_bytes: input,
            sort_readback_bytes: readback,
            first_gpu_peak,
            second_gpu_peak,
            raster_tests: tests,
        })
    }
}

pub struct CpuSortRaster;

impl CpuSortRaster {
    pub fn build(
        bins: &TileBins,
        cutoff: f64,
        options: SortRasterOptions,
    ) -> Result<SortedRaster, DomainError> {
        let plan = options.plan(bins, cutoff)?;
        let mut pairs: Vec<SortedTilePair> = bins
            .pairs
            .iter()
            .enumerate()
            .map(|(i, pair)| {
                let projected = bins.projected[pair.source_index]
                    .as_ref()
                    .ok_or_else(|| range("pair_source"))?;
                Ok(SortedTilePair {
                    tile_id: pair.tile_id as u32,
                    depth_key: canonical_depth_key(projected.depth)?,
                    source_index: pair.source_index as u32,
                    input_index: i as u32,
                })
            })
            .collect::<Result<_, DomainError>>()?;
        // Stable on equal (tile, depth) keys → preserves P4-F03 source-major order.
        pairs.sort_by_key(|pair| (pair.tile_id, pair.depth_key));
        let mut tile_ranges = vec![TileRange { start: 0, count: 0 }; plan.t as usize];
        let mut next = 0usize;
        for (tile, range) in tile_ranges.iter_mut().enumerate() {
            let start = next;
            while next < pairs.len() && pairs[next].tile_id == tile as u32 {
                next += 1;
            }
            *range = TileRange {
                start: start as u32,
                count: (next - start) as u32,
            };
        }
        let mut pixels = vec![PixelCoverage::default(); plan.m as usize];
        for (y, row) in pixels.chunks_mut(bins.width as usize).enumerate() {
            for (x, out) in row.iter_mut().enumerate() {
                let tile = (y as u32 / bins.tile_size) * bins.tiles_x + (x as u32 / bins.tile_size);
                let range = tile_ranges[tile as usize];
                let end = (range.start + range.count) as usize;
                for pair in &pairs[range.start as usize..end] {
                    let projected = bins.projected[pair.source_index as usize]
                        .as_ref()
                        .expect("visible pair source");
                    let alpha = coverage_alpha(*projected, [x as f64 + 0.5, y as f64 + 0.5])?;
                    if alpha > cutoff {
                        if out.count == 0 {
                            out.first_source = pair.source_index;
                        }
                        out.count += 1;
                        out.max_alpha = out.max_alpha.max(alpha);
                    }
                }
            }
        }
        let out = SortedRaster {
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
        out.validate_against(bins)?;
        Ok(out)
    }
}

/// Primitive3DGS.cuh:252-261 conic Gaussian; alpha clipped to 0.999; sigma<0 → 0.
/// Crate-private: rejects non-finite / out-of-range opacity so NaN cannot ride
/// `f64::min` into a false 0.999 success.
pub(crate) fn coverage_alpha(
    projected: ProjectedTileSplat,
    pixel: [f64; 2],
) -> Result<f64, DomainError> {
    if !projected.opacity.is_finite() || !(0.0..=1.0).contains(&projected.opacity) {
        return Err(range("raster_opacity"));
    }
    let [xx, xy, yy] = projected.covariance;
    let det = xx * yy - xy * xy;
    if !det.is_finite() || det <= 0.0 {
        return Err(range("raster_covariance"));
    }
    let dx = projected.pixel[0] - pixel[0];
    let dy = projected.pixel[1] - pixel[1];
    // conic = (yy/det, -xy/det, xx/det); sigma = 0.5*(cx*dx²+cz*dy²)+cy*dx*dy.
    let sigma = 0.5 * ((yy / det) * dx * dx + (xx / det) * dy * dy) - (xy / det) * dx * dy;
    if !sigma.is_finite() {
        return Err(range("raster_sigma"));
    }
    if sigma < 0.0 {
        return Ok(0.0);
    }
    let alpha = projected.opacity * (-sigma).exp();
    if !alpha.is_finite() {
        return Err(range("raster_alpha"));
    }
    Ok(alpha.min(0.999))
}

impl SortedRaster {
    pub fn validate_against(&self, bins: &TileBins) -> Result<(), DomainError> {
        bins.validate()?;
        if self.contract_version != SORT_RASTER_VERSION {
            return Err(error(
                ErrorCode::UnsupportedVersion,
                "sort_raster_version",
                "unsupported sort/raster contract",
            ));
        }
        if (self.width, self.height, self.tile_size) != (bins.width, bins.height, bins.tile_size)
            || self.pairs.len() != bins.pairs.len()
            || self.tile_ranges.len() != bins.tile_count()
            || self.pixels.len() != self.width as usize * self.height as usize
            || !self.alpha_cutoff.is_finite()
            || self.alpha_cutoff <= 0.0
            || self.alpha_cutoff > 1.0
            || self.alpha_cutoff < bins.projection_alpha_cutoff
            || !self.effective_alpha_cutoff.is_finite()
            || self.effective_alpha_cutoff <= 0.0
            || (self.alpha_cutoff as f32) != self.effective_alpha_cutoff
        {
            return Err(range("sort_raster_structure"));
        }
        let mut seen = vec![false; self.pairs.len()];
        let mut previous = None;
        for pair in &self.pairs {
            let i = pair.input_index as usize;
            let Some(input) = bins.pairs.get(i) else {
                return Err(range("input_index"));
            };
            let depth = bins.projected[input.source_index]
                .as_ref()
                .ok_or_else(|| range("pair_source"))?
                .depth;
            if seen[i]
                || input.tile_id != pair.tile_id as usize
                || input.source_index != pair.source_index as usize
                || canonical_depth_key(depth)? != pair.depth_key
            {
                return Err(range("sorted_pairs"));
            }
            seen[i] = true;
            let key = (pair.tile_id, pair.depth_key, pair.input_index);
            if previous.is_some_and(|old| old > key) {
                return Err(range("sort_stability"));
            }
            previous = Some(key);
        }
        if seen.iter().any(|v| !v) {
            return Err(range("sorted_pairs"));
        }
        let mut end = 0usize;
        for (tile, tile_range) in self.tile_ranges.iter().enumerate() {
            if tile_range.start as usize != end {
                return Err(range("tile_ranges"));
            }
            end = end
                .checked_add(tile_range.count as usize)
                .ok_or_else(|| range("tile_ranges"))?;
            if end > self.pairs.len()
                || self.pairs[tile_range.start as usize..end]
                    .iter()
                    .any(|pair| pair.tile_id != tile as u32)
            {
                return Err(range("tile_ranges"));
            }
        }
        if end != self.pairs.len() {
            return Err(range("tile_ranges"));
        }
        for (i, pixel) in self.pixels.iter().enumerate() {
            let x = (i as u32) % self.width;
            let y = (i as u32) / self.width;
            let tile = (y / self.tile_size) * bins.tiles_x + (x / self.tile_size);
            let tile_range = self.tile_ranges[tile as usize];
            if pixel.count > tile_range.count
                || !pixel.max_alpha.is_finite()
                || pixel.max_alpha < 0.0
                || pixel.max_alpha > 0.999 + 1e-7
            {
                return Err(range("pixel_coverage"));
            }
            if pixel.count == 0 {
                if pixel.first_source != NO_SOURCE || pixel.max_alpha != 0.0 {
                    return Err(range("empty_pixel"));
                }
            } else if pixel.max_alpha <= self.alpha_cutoff
                || !self.pairs
                    [tile_range.start as usize..(tile_range.start + tile_range.count) as usize]
                    .iter()
                    .any(|pair| pair.source_index == pixel.first_source)
            {
                return Err(range("covered_pixel"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TILE_CONTRACT_VERSION, TileCoverage, TilePair};

    fn bins(width: u32, height: u32, depths: &[f64]) -> TileBins {
        let tiles_x = width.div_ceil(8);
        let tiles_y = height.div_ceil(8);
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
            tile_size: 8,
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

    #[test]
    fn empty_and_v1_contract_are_typed() {
        let empty = TileBins::empty(17, 9, crate::TileBuildOptions::default()).unwrap();
        assert_eq!(empty.projection_alpha_cutoff, 0.0);
        let out = CpuSortRaster::build(&empty, 0.01, SortRasterOptions::default()).unwrap();
        assert!(out.pairs.is_empty());
        assert_eq!(out.pixels.len(), 153);
        assert!(out.pixels.iter().all(|p| p.count == 0));

        let mut v1 = bins(8, 8, &[1.0]);
        v1.contract_version = 1;
        let err = CpuSortRaster::build(&v1, 0.01, SortRasterOptions::default()).unwrap_err();
        assert_eq!(err.code(), ErrorCode::UnsupportedVersion);
    }

    #[test]
    fn raster_cutoff_below_projection_is_rejected() {
        let data = bins(8, 8, &[1.0]);
        let err = CpuSortRaster::build(&data, 1e-5, SortRasterOptions::default()).unwrap_err();
        assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
        assert_eq!(
            err.context_value(spirula_core::ContextKey::Field),
            Some("projection_cutoff_mismatch")
        );
    }
}
