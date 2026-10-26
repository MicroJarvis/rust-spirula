//! Spirula GPU boundary (`spirula-gpu`).
//!
//! P2-F01: adapter enumeration and capability negotiation ([`adapter`]).
//! P2-F02: session-owned device/queue context lifecycle ([`context`]).
//! P2-F03: buffer/texture staging upload/readback ([`resource`]).
//! P2-F04: compute shader/pipeline key/cache ([`pipeline`]).
//! P2-F05: GPU errors and diagnostic events ([`diagnostics`]).
//! Public `wgpu` types do not leave this crate's implementation modules.

#![forbid(unsafe_code)]

pub mod adapter;
pub mod context;
pub mod diagnostics;
mod enumerate;
pub mod pipeline;
pub mod resource;

pub use adapter::{
    AdapterCapabilities, AdapterClass, AdapterIdentity, AdapterLimits, AdapterSelection,
    BackendKind, CapabilityId, CapabilityRequest, F16Plan, FeatureBits, FieldAvailability,
    FloatAtomicPlan, LimitFloor, NegotiatedPlan, ResolvedLimit, ResolvedLimits, SubgroupPlan,
    TimestampPlan, is_eligible, negotiate, select_auto,
};
pub use context::{
    ContextGeneration, ContextId, ContextStatus, DeviceHandle, DeviceLimitsSnapshot,
    DeviceOpenReport, GpuContext, LostNotice, LostSource, OpenError, OpenFailureReport,
    OpenFailureStage, diagnose_device_open, format_device_report,
};
pub use diagnostics::{
    DEFAULT_DIAGNOSTIC_CAPACITY, DiagnosticKind, DiagnosticStats, GpuDiagnosticEvent, GpuOperation,
    ScopeClass, ScopeErrorInject,
};
pub use enumerate::{WGPU_VERSION, diagnose_exit_code, enumerate_host_adapters};
pub use pipeline::{
    AFFINE_U32_BIND_LAYOUT_VERSION, AFFINE_U32_BIND_LAYOUT_VERSION_V2, AFFINE_U32_ENTRY,
    AFFINE_U32_NAME, AFFINE_U32_PARAMS_BYTES, AFFINE_U32_VERSION, AFFINE_U32_WGSL,
    AFFINE_U32_WORKGROUP_SIZE, AffineU32Params, ComputePipelineDesc, ComputePipelineHandle,
    PipelineCacheStats, PipelineKey, ShaderHeader, SpecConstants, affine_bind_layout_supported,
    cpu_affine_u32, parse_shader_header,
};
pub use resource::{
    BufferDesc, BufferHandle, BufferUsage, COPY_BUFFER_ALIGNMENT, COPY_BYTES_PER_ROW_ALIGNMENT,
    PendingResourceCounts, RESOURCE_ACCOUNT_SOURCE, ReadbackTicket, ResourceAccountingSnapshot,
    ResourceLabel, SubmissionHandle, Texture2dDesc, TextureFormat, TextureHandle, TextureUsage,
    rgba8_bytes_per_row, rgba8_logical_bytes, rgba8_padded_bytes,
};
