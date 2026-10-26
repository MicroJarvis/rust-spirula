//! Spirula GPU boundary (`spirula-gpu`).
//!
//! P2-F01: adapter enumeration and capability negotiation ([`adapter`]).
//! P2-F02: session-owned device/queue context lifecycle ([`context`]).
//! Buffer/texture/readback remain P2-F03; pipelines/compute remain P2-F04.
//! Public `wgpu` types do not leave this crate's implementation modules.

#![forbid(unsafe_code)]

pub mod adapter;
pub mod context;
mod enumerate;

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
