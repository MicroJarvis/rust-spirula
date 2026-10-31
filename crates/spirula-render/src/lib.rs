//! Gaussian scene representation, render graph, and preview rendering.

mod scene;
mod splat_buffer;

pub use scene::{
    ColorSpace, CpuReference, ProjectedGaussian, RENDER_CONTRACT_VERSION, RenderPrimitive,
    RenderRequest, RenderThresholds,
};
pub use splat_buffer::{
    GpuSplatBuffer, PackedSplatData, SPLAT_GPU_LAYOUT_VERSION, SPLAT_GPU_STRIDE_BYTES,
    SplatGpuLayout,
};
