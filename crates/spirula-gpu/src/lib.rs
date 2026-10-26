//! Spirula GPU boundary (`spirula-gpu`).
//!
//! P2-F01: adapter enumeration and capability negotiation ([`adapter`]).
//! P2-F02: session-owned device/queue context lifecycle ([`context`]).
//! P2-F03: buffer/texture staging upload/readback ([`resource`]).
//! Pipelines/compute remain P2-F04.
//! Public `wgpu` types do not leave this crate's implementation modules.

#![forbid(unsafe_code)]

pub mod adapter;
pub mod context;
mod enumerate;
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
pub use enumerate::{WGPU_VERSION, diagnose_exit_code, enumerate_host_adapters};
pub use resource::{
    BufferDesc, BufferHandle, BufferUsage, COPY_BYTES_PER_ROW_ALIGNMENT, RESOURCE_ACCOUNT_SOURCE,
    ReadbackTicket, ResourceAccountingSnapshot, ResourceLabel, SubmissionHandle, Texture2dDesc,
    TextureFormat, TextureHandle, TextureUsage, rgba8_bytes_per_row, rgba8_logical_bytes,
    rgba8_padded_bytes,
};
