//! Gaussian scene representation, render graph, and preview rendering.

mod gpu_tiles;
mod scene;
mod splat_buffer;
mod tiles;
pub use gpu_tiles::{GpuTileBuilder, TileBuildMetrics};

pub use scene::{
    ColorSpace, CpuReference, ProjectedGaussian, RENDER_CONTRACT_VERSION, RenderPrimitive,
    RenderRequest, RenderThresholds,
};
pub use splat_buffer::{
    GpuSplatBuffer, PackedSplatData, SPLAT_GPU_LAYOUT_VERSION, SPLAT_GPU_STRIDE_BYTES,
    SplatGpuLayout,
};
pub use tiles::{
    CpuTileBuilder, ProjectedTileSplat, TILE_CONTRACT_VERSION, TileBins, TileBuildOptions,
    TileCoverage, TilePair,
};

mod gpu_sort_raster;
mod sort_raster;
pub use gpu_sort_raster::{GpuSortRaster, SortRasterMetrics};
pub use sort_raster::{
    CpuSortRaster, NO_SOURCE, PixelCoverage, SORT_RASTER_VERSION, SortRasterOptions, SortedRaster,
    SortedTilePair, TileRange, canonical_depth_key,
};
