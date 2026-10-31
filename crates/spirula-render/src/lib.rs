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

mod compositing;
mod gpu_compositing;
mod preview;
pub use compositing::{
    BackgroundPolicy, COMPOSITE_CONTRACT_VERSION, CompositeImage, CompositeOptions, CompositePixel,
    CpuCompositor, sh0_to_color,
};
pub use gpu_compositing::{CompositeMetrics, GpuCompositor};
pub use preview::{
    HeadlessPreviewMetrics, HeadlessPreviewResult, PREVIEW_FLOAT_BYTES_PER_PIXEL,
    PREVIEW_PACK_CONTRACT_VERSION, PREVIEW_RGB8_BYTES_PER_PIXEL, PreviewRgb8,
    pack_composite_to_preview_rgb8, preview_host_float_plus_rgb8_bytes, quantize_linear_channel_u8,
    run_headless_gpu_preview,
};
