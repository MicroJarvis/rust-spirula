//! P4-F03 projection and conservative AABB tile intersections, before sorting.
use crate::RenderRequest;
use spirula_core::{ContextKey, DomainError, ErrorCode};

pub const TILE_CONTRACT_VERSION: u32 = 2;
pub(crate) const RECORD_WORDS: usize = 16;

pub(crate) fn error(code: ErrorCode, field: &str, message: impl Into<String>) -> DomainError {
    DomainError::try_new(code, message)
        .expect("tile error")
        .with_context(ContextKey::Field, field)
        .expect("tile field")
}
fn range(field: &str) -> DomainError {
    error(
        ErrorCode::ValueOutOfRange,
        field,
        "tile extent or resource budget exceeded",
    )
}

/// Explicit limits apply before allocation; no automatic tile coarsening.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileBuildOptions {
    pub tile_size: u32,
    pub max_pairs: u64,
    pub max_tiles: u64,
    /// All owned host output/scratch allocations, excluding caller scene.
    pub max_host_bytes: u64,
    /// Accounted GPU buffers and staging, excluding existing caller resources.
    pub max_gpu_bytes: u64,
}
impl Default for TileBuildOptions {
    fn default() -> Self {
        Self {
            tile_size: 8,
            max_pairs: 1_000_000,
            max_tiles: 1_000_000,
            max_host_bytes: 256 << 20,
            max_gpu_bytes: 256 << 20,
        }
    }
}
impl TileBuildOptions {
    pub(crate) fn dimensions(
        self,
        width: u32,
        height: u32,
        n: usize,
    ) -> Result<(u32, u32), DomainError> {
        if self.tile_size == 0 || self.tile_size > 128 || !self.tile_size.is_power_of_two() {
            return Err(range("tile_size"));
        }
        if width == 0 || height == 0 || width > 65535 || height > 65535 {
            return Err(range("image_size"));
        }
        if self.max_pairs == 0 || self.max_pairs > i32::MAX as u64 || self.max_tiles == 0 {
            return Err(range("pair_or_tile_budget"));
        }
        let tx = width.div_ceil(self.tile_size);
        let ty = height.div_ceil(self.tile_size);
        if u64::from(tx) * u64::from(ty) > self.max_tiles || n > i32::MAX as usize {
            return Err(range("tiles_or_splats"));
        }
        self.host_preflight(n, 0)?;
        Ok((tx, ty))
    }
    pub(crate) fn host_preflight(self, n: usize, pairs: u64) -> Result<u64, DomainError> {
        // Includes projected records, two GPU readback/packing copies, count/scan,
        // and output pairs. This upper bound deliberately exceeds Vec payloads.
        let bytes = (n as u64)
            .checked_mul(256)
            .and_then(|v| v.checked_add(pairs.checked_mul(32)?))
            .and_then(|v| v.checked_add(4096))
            .ok_or_else(|| range("host_bytes"))?;
        if bytes > self.max_host_bytes {
            return Err(range("host_bytes"));
        }
        Ok(bytes)
    }
    pub(crate) fn pair_preflight(self, total: u64, n: usize) -> Result<(), DomainError> {
        if total > self.max_pairs || total > i32::MAX as u64 {
            return Err(range("max_pairs"));
        }
        self.host_preflight(n, total)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectedTileSplat {
    pub source_index: usize,
    pub pixel: [f64; 2],
    /// Positive axial depth for sorting. Ray distance is separate.
    pub depth: f64,
    pub ray_depth: f64,
    pub radius_px: [f64; 2],
    /// Symmetric blurred 2D covariance [xx, xy, yy].
    pub covariance: [f64; 3],
    pub opacity: f64,
    /// Half-open integer tile bounds [xmin, ymin, xmax, ymax].
    pub tile_bounds: [u32; 4],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TilePair {
    pub tile_id: usize,
    pub source_index: usize,
}
/// One source's contiguous pair range. Source-major order is intentional;
/// P4-F04 owns tile/depth sorting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileCoverage {
    pub source_index: usize,
    pub pair_offset: usize,
    pub pair_count: usize,
}
#[derive(Clone, Debug, PartialEq)]
pub struct TileBins {
    pub contract_version: u32,
    pub width: u32,
    pub height: u32,
    /// Cutoff used to cull/project this candidate set; zero only for empty input.
    pub projection_alpha_cutoff: f64,
    pub tile_size: u32,
    pub tiles_x: u32,
    pub tiles_y: u32,
    /// One entry per source; culled entries are None, indices remain stable.
    pub projected: Vec<Option<ProjectedTileSplat>>,
    pub coverage: Vec<TileCoverage>,
    pub pairs: Vec<TilePair>,
    /// Inclusive scan of each source's intersection count.
    pub inclusive_offsets: Vec<u32>,
}
impl TileBins {
    pub fn empty(width: u32, height: u32, options: TileBuildOptions) -> Result<Self, DomainError> {
        finish(width, height, options, vec![], vec![], vec![], 0.0)
    }
    pub fn tile_count(&self) -> usize {
        self.tiles_x as usize * self.tiles_y as usize
    }
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.contract_version != TILE_CONTRACT_VERSION {
            return Err(error(
                ErrorCode::UnsupportedVersion,
                "contract_version",
                "unsupported tile contract",
            ));
        }
        if !self.projection_alpha_cutoff.is_finite()
            || self.projection_alpha_cutoff < 0.0
            || self.projection_alpha_cutoff > 1.0
            || (!self.projected.is_empty() && self.projection_alpha_cutoff == 0.0)
        {
            return Err(range("projection_alpha_cutoff"));
        }
        let options = TileBuildOptions {
            tile_size: self.tile_size,
            max_pairs: i32::MAX as u64,
            max_tiles: u64::MAX,
            max_host_bytes: u64::MAX,
            ..TileBuildOptions::default()
        };
        let (tx, ty) = options.dimensions(self.width, self.height, self.projected.len())?;
        if (tx, ty) != (self.tiles_x, self.tiles_y)
            || self.coverage.len() != self.projected.len()
            || self.inclusive_offsets.len() != self.projected.len()
        {
            return Err(range("tile_structure"));
        }
        let mut end = 0usize;
        for (i, c) in self.coverage.iter().enumerate() {
            if c.source_index != i || c.pair_offset != end {
                return Err(range("coverage"));
            }
            let expected = if let Some(p) = self.projected[i] {
                let b = p.tile_bounds;
                if p.source_index != i
                    || b[0] >= b[2]
                    || b[1] >= b[3]
                    || b[2] > tx
                    || b[3] > ty
                    || !p
                        .pixel
                        .iter()
                        .chain(p.radius_px.iter())
                        .chain(p.covariance.iter())
                        .all(|v| v.is_finite())
                    || p.radius_px.iter().any(|v| *v < 0.0)
                    || !p.depth.is_finite()
                    || p.depth <= 0.0
                    || !p.ray_depth.is_finite()
                    || p.ray_depth < p.depth
                    || !p.opacity.is_finite()
                    || !(0.0..=1.0).contains(&p.opacity)
                    || p.covariance[0] <= 0.0
                    || p.covariance[2] <= 0.0
                    || !(p.covariance[0] * p.covariance[2] - p.covariance[1] * p.covariance[1])
                        .is_finite()
                    || p.covariance[0] * p.covariance[2] - p.covariance[1] * p.covariance[1] <= 0.0
                {
                    return Err(range("projected"));
                }
                ((b[2] - b[0]) as usize) * ((b[3] - b[1]) as usize)
            } else {
                0
            };
            end = end.checked_add(expected).ok_or_else(|| range("coverage"))?;
            if c.pair_count != expected
                || end > self.pairs.len()
                || self.inclusive_offsets[i] as usize != end
            {
                return Err(range("scan"));
            }
            if let Some(p) = self.projected[i] {
                let b = p.tile_bounds;
                for (k, pair) in self.pairs[c.pair_offset..end].iter().enumerate() {
                    let x = b[0] + (k as u32) % (b[2] - b[0]);
                    let y = b[1] + (k as u32) / (b[2] - b[0]);
                    if pair.source_index != i || pair.tile_id != (y * tx + x) as usize {
                        return Err(range("pairs"));
                    }
                }
            }
        }
        if end != self.pairs.len() {
            return Err(range("pairs"));
        }
        Ok(())
    }
}
pub struct CpuTileBuilder;
impl CpuTileBuilder {
    pub fn build(
        request: &RenderRequest,
        options: TileBuildOptions,
    ) -> Result<TileBins, DomainError> {
        validate_request(request)?;
        let (tx, ty) = options.dimensions(
            request.camera.width(),
            request.camera.height(),
            request.scene.len(),
        )?;
        let mut projected = Vec::with_capacity(request.scene.len());
        let mut offsets = Vec::with_capacity(request.scene.len());
        let mut total = 0u64;
        for i in 0..request.scene.len() {
            let p = project(request, i, options.tile_size, tx, ty)?;
            if let Some(p) = p {
                let b = p.tile_bounds;
                total = total
                    .checked_add(u64::from(b[2] - b[0]) * u64::from(b[3] - b[1]))
                    .ok_or_else(|| range("max_pairs"))?;
            }
            options.pair_preflight(total, request.scene.len())?;
            offsets.push(total as u32);
            projected.push(p);
        }
        let mut pairs = Vec::with_capacity(total as usize);
        for p in projected.iter().flatten() {
            let b = p.tile_bounds;
            for y in b[1]..b[3] {
                for x in b[0]..b[2] {
                    pairs.push(TilePair {
                        tile_id: (y * tx + x) as usize,
                        source_index: p.source_index,
                    });
                }
            }
        }
        finish(
            request.camera.width(),
            request.camera.height(),
            options,
            projected,
            offsets,
            pairs,
            request.thresholds.alpha_cutoff,
        )
    }
    pub fn build_empty(
        width: u32,
        height: u32,
        options: TileBuildOptions,
    ) -> Result<TileBins, DomainError> {
        TileBins::empty(width, height, options)
    }
}
pub(crate) fn validate_request(request: &RenderRequest) -> Result<(), DomainError> {
    request.validate()?;
    if request.camera.distortion().coefficients() != [0.0; 4] {
        return Err(error(
            ErrorCode::UnimplementedOption,
            "distortion",
            "projection tiles currently require undistorted pinhole camera",
        ));
    }
    if request.thresholds.alpha_cutoff <= 0.0 {
        return Err(error(
            ErrorCode::ValueOutOfRange,
            "alpha_cutoff",
            "projection requires positive alpha cutoff",
        ));
    }
    Ok(())
}
fn project(
    request: &RenderRequest,
    i: usize,
    tile: u32,
    tx: u32,
    ty: u32,
) -> Result<Option<ProjectedTileSplat>, DomainError> {
    let pose = request.camera.pose();
    let view = pose.rotation().transpose();
    let splat = &request.scene.splats()[i];
    let gl = view.try_mul_vec((splat.mean() - pose.translation())?)?;
    let depth = -gl.z();
    if depth <= 0.0 {
        return Ok(None);
    }
    let opacity = splat.opacity();
    if opacity < request.thresholds.alpha_cutoff {
        return Ok(None);
    }
    let [fx, fy, cx, cy] = request.camera.intrinsics();
    let pixel = [fx * gl.x() / depth + cx, -fy * gl.y() / depth + cy];
    let jx = [
        fx / depth,
        0.0,
        (fx * gl.x() / depth).clamp(
            -0.15 * f64::from(request.camera.width()) - cx,
            1.15 * f64::from(request.camera.width()) - cx,
        ) / depth,
    ];
    // Camera y is upward and z negative; pixel y is downward.
    let jy = [
        0.0,
        -fy / depth,
        (-fy * gl.y() / depth).clamp(
            -0.15 * f64::from(request.camera.height()) - cy,
            1.15 * f64::from(request.camera.height()) - cy,
        ) / depth,
    ];
    let axis = view.try_mul(splat.rotation().to_rotation())?.row_major();
    let scales = splat.scales().to_array();
    let mut cov = [0.3, 0.0, 0.3];
    for k in 0..3 {
        let ax = (jx[0] * axis[k] + jx[1] * axis[3 + k] + jx[2] * axis[6 + k]) * scales[k];
        let ay = (jy[0] * axis[k] + jy[1] * axis[3 + k] + jy[2] * axis[6 + k]) * scales[k];
        cov[0] += ax * ax;
        cov[1] += ax * ay;
        cov[2] += ay * ay;
    }
    let extend = (2.0 * (opacity / request.thresholds.alpha_cutoff).ln())
        .sqrt()
        .min(3.33);
    let radius = [extend * cov[0].sqrt(), extend * cov[2].sqrt()];
    let det = cov[0] * cov[2] - cov[1] * cov[1];
    if !det.is_finite()
        || !pixel
            .iter()
            .chain(radius.iter())
            .chain(cov.iter())
            .all(|v| v.is_finite())
        || det <= 0.0
    {
        return Err(range("projection"));
    }
    let bounds = [
        pixel[0] - radius[0],
        pixel[1] - radius[1],
        pixel[0] + radius[0],
        pixel[1] + radius[1],
    ];
    let w = f64::from(request.camera.width());
    let h = f64::from(request.camera.height());
    if bounds[2] <= 0.0 || bounds[3] <= 0.0 || bounds[0] >= w || bounds[1] >= h {
        return Ok(None);
    }
    // Conservative full precision AABB, with the reference pixel-center shift.
    // Reference 16-bit quantization and ellipse rejection are not applied.
    let ts = f64::from(tile);
    let b = [
        ((bounds[0].max(0.0) + 0.5) / ts).floor().min(f64::from(tx)) as u32,
        ((bounds[1].max(0.0) + 0.5) / ts).floor().min(f64::from(ty)) as u32,
        ((bounds[2].min(w) + 0.5) / ts).ceil().min(f64::from(tx)) as u32,
        ((bounds[3].min(h) + 0.5) / ts).ceil().min(f64::from(ty)) as u32,
    ];
    if b[0] >= b[2] || b[1] >= b[3] {
        return Ok(None);
    }
    Ok(Some(ProjectedTileSplat {
        source_index: i,
        pixel,
        depth,
        ray_depth: (gl.x() * gl.x() + gl.y() * gl.y() + gl.z() * gl.z()).sqrt(),
        radius_px: radius,
        covariance: cov,
        opacity,
        tile_bounds: b,
    }))
}
pub(crate) fn finish(
    width: u32,
    height: u32,
    options: TileBuildOptions,
    projected: Vec<Option<ProjectedTileSplat>>,
    inclusive_offsets: Vec<u32>,
    pairs: Vec<TilePair>,
    projection_alpha_cutoff: f64,
) -> Result<TileBins, DomainError> {
    let (tx, ty) = options.dimensions(width, height, projected.len())?;
    options.pair_preflight(pairs.len() as u64, projected.len())?;
    let mut start = 0;
    let coverage = inclusive_offsets
        .iter()
        .enumerate()
        .map(|(source_index, &end)| {
            let end = end as usize;
            let c = TileCoverage {
                source_index,
                pair_offset: start,
                pair_count: end.saturating_sub(start),
            };
            start = end;
            c
        })
        .collect();
    let bins = TileBins {
        contract_version: TILE_CONTRACT_VERSION,
        projection_alpha_cutoff,
        width,
        height,
        tile_size: options.tile_size,
        tiles_x: tx,
        tiles_y: ty,
        projected,
        coverage,
        pairs,
        inclusive_offsets,
    };
    bins.validate()?;
    Ok(bins)
}
