//! Spirula GPU boundary (`spirula-gpu`).
//!
//! P2-F01: adapter enumeration, required/optional capability negotiation, and
//! a diagnose binary. Device/queue lifecycle is P2-F02. Public `wgpu` types do
//! not leave this crate's implementation modules; consumers use the narrow
//! value types in [`adapter`].

#![forbid(unsafe_code)]

pub mod adapter;
mod enumerate;

pub use adapter::{
    AdapterCapabilities, AdapterClass, AdapterIdentity, AdapterLimits, AdapterSelection,
    BackendKind, CapabilityId, CapabilityRequest, F16Plan, FeatureBits, FieldAvailability,
    FloatAtomicPlan, LimitFloor, NegotiatedPlan, ResolvedLimit, ResolvedLimits, SubgroupPlan,
    TimestampPlan, is_eligible, negotiate, select_auto,
};
pub use enumerate::{WGPU_VERSION, diagnose_exit_code, enumerate_host_adapters};
