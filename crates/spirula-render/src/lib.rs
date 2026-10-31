//! Gaussian scene representation, render graph, and preview rendering.

mod scene;

pub use scene::{
    ColorSpace, CpuReference, ProjectedGaussian, RENDER_CONTRACT_VERSION, RenderPrimitive,
    RenderRequest, RenderThresholds,
};
