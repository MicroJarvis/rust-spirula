//! Pure adapter identity, capability requests, negotiation, and selection.
//!
//! No `wgpu` types here — synthetic tests drive the same ranking TrainPlan /
//! P2-F02 will consume.

use spirula_core::{ContextKey, DomainError, ErrorCode, UserAction};

/// wgpu backend family as a portable value (not a wgpu re-export).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BackendKind {
    Vulkan,
    Metal,
    Dx12,
    Gl,
    BrowserWebGpu,
    Noop,
    Empty,
}

impl BackendKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Vulkan => "vulkan",
            Self::Metal => "metal",
            Self::Dx12 => "dx12",
            Self::Gl => "gl",
            Self::BrowserWebGpu => "browser-webgpu",
            Self::Noop => "noop",
            Self::Empty => "empty",
        }
    }

    /// Higher is preferred when class and VRAM ties.
    pub const fn rank(self) -> u8 {
        match self {
            Self::Metal => 6,
            Self::Vulkan => 5,
            Self::Dx12 => 4,
            Self::Gl => 3,
            Self::BrowserWebGpu => 2,
            Self::Noop => 1,
            Self::Empty => 0,
        }
    }
}

/// Adapter device class for selection scoring.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AdapterClass {
    DiscreteGpu,
    IntegratedGpu,
    Cpu,
    Other,
}

impl AdapterClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DiscreteGpu => "discrete-gpu",
            Self::IntegratedGpu => "integrated-gpu",
            Self::Cpu => "cpu",
            Self::Other => "other",
        }
    }

    /// C++-aligned preference: discrete > integrated > other > cpu.
    pub const fn rank(self) -> u8 {
        match self {
            Self::DiscreteGpu => 3,
            Self::IntegratedGpu => 2,
            Self::Other => 1,
            Self::Cpu => 0,
        }
    }
}

/// Measured value or an honest unavailable reason (never invent VRAM/uuid).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldAvailability<T> {
    Available(T),
    Unavailable { reason: &'static str },
}

impl<T> FieldAvailability<T> {
    pub const fn is_available(&self) -> bool {
        matches!(self, Self::Available(_))
    }

    pub fn as_available(&self) -> Option<&T> {
        match self {
            Self::Available(v) => Some(v),
            Self::Unavailable { .. } => None,
        }
    }
}

/// Stable identity fields for one enumerated adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterIdentity {
    pub index: usize,
    pub name: String,
    pub backend: BackendKind,
    pub class: AdapterClass,
    pub vendor: u32,
    pub device: u32,
    pub driver: String,
    pub driver_info: String,
    pub dedicated_vram_bytes: FieldAvailability<u64>,
    pub uuid: FieldAvailability<String>,
}

/// Capabilities Spirula negotiates in P2-F01 (feature bit ≠ kernel shipped).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CapabilityId {
    Subgroup,
    ShaderF16,
    TimestampQuery,
    Float32AtomicAdd,
}

impl CapabilityId {
    pub const ALL: [CapabilityId; 4] = [
        Self::Subgroup,
        Self::ShaderF16,
        Self::TimestampQuery,
        Self::Float32AtomicAdd,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Subgroup => "subgroup",
            Self::ShaderF16 => "shader-f16",
            Self::TimestampQuery => "timestamp-query",
            Self::Float32AtomicAdd => "float32-atomic-add",
        }
    }
}

/// Adapter-reported presence of the four negotiated bits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeatureBits {
    pub subgroup: bool,
    pub shader_f16: bool,
    pub timestamp_query: bool,
    pub float32_atomic_add: bool,
}

impl FeatureBits {
    pub fn supports(self, id: CapabilityId) -> bool {
        match id {
            CapabilityId::Subgroup => self.subgroup,
            CapabilityId::ShaderF16 => self.shader_f16,
            CapabilityId::TimestampQuery => self.timestamp_query,
            CapabilityId::Float32AtomicAdd => self.float32_atomic_add,
        }
    }
}

/// Narrow limit floors (required min or optional desire). Same three fields —
/// not a full wgpu `Limits` mirror.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LimitFloor {
    pub max_buffer_size: Option<u64>,
    pub max_storage_buffers_per_shader_stage: Option<u32>,
    pub max_compute_workgroup_storage_size: Option<u32>,
}

/// Observed limits on one adapter (subset).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdapterLimits {
    pub max_buffer_size: u64,
    pub max_storage_buffers_per_shader_stage: u32,
    pub max_compute_workgroup_storage_size: u32,
}

impl AdapterLimits {
    pub fn meets(self, floor: LimitFloor) -> Result<(), &'static str> {
        if let Some(need) = floor.max_buffer_size
            && self.max_buffer_size < need
        {
            return Err("max_buffer_size");
        }
        if let Some(need) = floor.max_storage_buffers_per_shader_stage
            && self.max_storage_buffers_per_shader_stage < need
        {
            return Err("max_storage_buffers_per_shader_stage");
        }
        if let Some(need) = floor.max_compute_workgroup_storage_size
            && self.max_compute_workgroup_storage_size < need
        {
            return Err("max_compute_workgroup_storage_size");
        }
        Ok(())
    }
}

/// Resolved value for one negotiated limit field.
///
/// Invariants after a successful negotiate (required floors already passed):
/// - `effective <= adapter`
/// - if `required` was set, `effective >= required`
/// - `desired_met` is honest about the optional desire only (adapter ≥ desire),
///   and never implies `effective == desire` when desire sits below required.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedLimit<T: Copy + Ord> {
    /// Optional desire from the request (`None` if not optionally requested).
    pub desired: Option<T>,
    /// Usable value for callers: ≥ required floor (if any) and ≤ adapter.
    pub effective: T,
    /// `true` iff no optional desire, or adapter capacity ≥ desire.
    pub desired_met: bool,
}

impl<T: Copy + Ord> ResolvedLimit<T> {
    /// `adapter` must already satisfy `required` when `required` is `Some`.
    fn resolve(adapter: T, required: Option<T>, desired: Option<T>) -> Self {
        debug_assert!(required.is_none_or(|r| adapter >= r));
        let desired_met = desired.is_none_or(|d| adapter >= d);
        // Optional desire may raise the usable value up to adapter, but must
        // never press below the required floor on a successful negotiation.
        let uncapped = match desired {
            None => adapter,
            Some(want) => want.min(adapter),
        };
        let effective = match required {
            None => uncapped,
            Some(floor) => uncapped.max(floor),
        };
        Self {
            desired,
            effective,
            desired_met,
        }
    }
}

/// Verifiable resolved limits after required floors pass and optional desires
/// are applied without violating those floors (same three fields as [`LimitFloor`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedLimits {
    pub max_buffer_size: ResolvedLimit<u64>,
    pub max_storage_buffers_per_shader_stage: ResolvedLimit<u32>,
    pub max_compute_workgroup_storage_size: ResolvedLimit<u32>,
}

impl ResolvedLimits {
    fn negotiate(adapter: AdapterLimits, required: LimitFloor, optional: LimitFloor) -> Self {
        Self {
            max_buffer_size: ResolvedLimit::resolve(
                adapter.max_buffer_size,
                required.max_buffer_size,
                optional.max_buffer_size,
            ),
            max_storage_buffers_per_shader_stage: ResolvedLimit::resolve(
                adapter.max_storage_buffers_per_shader_stage,
                required.max_storage_buffers_per_shader_stage,
                optional.max_storage_buffers_per_shader_stage,
            ),
            max_compute_workgroup_storage_size: ResolvedLimit::resolve(
                adapter.max_compute_workgroup_storage_size,
                required.max_compute_workgroup_storage_size,
                optional.max_compute_workgroup_storage_size,
            ),
        }
    }
}

/// One adapter's report for selection / TrainPlan inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterCapabilities {
    pub identity: AdapterIdentity,
    pub features: FeatureBits,
    pub limits: AdapterLimits,
}

/// Request: required rejects; optional features/limits negotiate fallbacks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CapabilityRequest {
    pub required_features: Vec<CapabilityId>,
    pub optional_features: Vec<CapabilityId>,
    /// Hard floor — miss rejects the adapter (`SP-GPU-001`).
    pub required_min_limits: LimitFloor,
    /// Soft desire — miss clamps to device capacity and records `desired_met=false`.
    /// Must not reject an adapter that already meets [`Self::required_min_limits`].
    pub optional_desired_limits: LimitFloor,
    /// When float32 atomic is optional and missing, prefer staged reduce.
    pub prefer_staged_reduce_fallback: bool,
}

impl CapabilityRequest {
    /// Diagnose default: no required extras; negotiate all four features as
    /// optional; no optional limit desires.
    pub fn diagnose_default() -> Self {
        Self {
            required_features: Vec::new(),
            optional_features: CapabilityId::ALL.to_vec(),
            required_min_limits: LimitFloor::default(),
            optional_desired_limits: LimitFloor::default(),
            prefer_staged_reduce_fallback: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubgroupPlan {
    /// Feature bit present — kernels not claimed implemented here.
    AvailableDeclared,
    FallbackNonSubgroup,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum F16Plan {
    AvailableDeclared,
    FallbackF32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimestampPlan {
    AvailableDeclared,
    FallbackHostTimer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatAtomicPlan {
    AvailableDeclared,
    FallbackCas,
    FallbackStagedReduce,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NegotiatedPlan {
    pub subgroup: SubgroupPlan,
    pub shader_f16: F16Plan,
    pub timestamp: TimestampPlan,
    pub float_atomic: FloatAtomicPlan,
    /// Resolved limits after required floors; optional desires never invent
    /// capacity or claim unmet requests as satisfied.
    pub limits: ResolvedLimits,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterSelection {
    pub adapter: AdapterCapabilities,
    pub negotiated: NegotiatedPlan,
}

fn missing_required(adapter: &AdapterCapabilities, request: &CapabilityRequest) -> Vec<String> {
    let mut missing = Vec::new();
    for id in &request.required_features {
        if !adapter.features.supports(*id) {
            missing.push(id.as_str().to_owned());
        }
    }
    if let Err(field) = adapter.limits.meets(request.required_min_limits) {
        missing.push(field.to_owned());
    }
    missing
}

/// Whether an adapter satisfies every required feature/limit.
pub fn is_eligible(adapter: &AdapterCapabilities, request: &CapabilityRequest) -> bool {
    missing_required(adapter, request).is_empty()
}

/// Negotiate plans for an **eligible** adapter. Required gaps → `SP-GPU-001`.
pub fn negotiate(
    adapter: &AdapterCapabilities,
    request: &CapabilityRequest,
) -> Result<NegotiatedPlan, DomainError> {
    let missing = missing_required(adapter, request);
    if !missing.is_empty() {
        return Err(no_adapter_error(
            format!(
                "adapter `{}` missing required capability/limit: {}",
                adapter.identity.name,
                missing.join(",")
            ),
            &adapter.identity.name,
            &missing.join(","),
        ));
    }

    let want = |id: CapabilityId| {
        request.required_features.contains(&id) || request.optional_features.contains(&id)
    };

    let subgroup = if adapter.features.subgroup && want(CapabilityId::Subgroup) {
        SubgroupPlan::AvailableDeclared
    } else if request.required_features.contains(&CapabilityId::Subgroup) {
        unreachable!("required checked above");
    } else {
        SubgroupPlan::FallbackNonSubgroup
    };

    let shader_f16 = if adapter.features.shader_f16 && want(CapabilityId::ShaderF16) {
        F16Plan::AvailableDeclared
    } else {
        F16Plan::FallbackF32
    };

    let timestamp = if adapter.features.timestamp_query && want(CapabilityId::TimestampQuery) {
        TimestampPlan::AvailableDeclared
    } else {
        TimestampPlan::FallbackHostTimer
    };

    let float_atomic =
        if adapter.features.float32_atomic_add && want(CapabilityId::Float32AtomicAdd) {
            FloatAtomicPlan::AvailableDeclared
        } else if request.prefer_staged_reduce_fallback {
            FloatAtomicPlan::FallbackStagedReduce
        } else {
            FloatAtomicPlan::FallbackCas
        };

    Ok(NegotiatedPlan {
        subgroup,
        shader_f16,
        timestamp,
        float_atomic,
        limits: ResolvedLimits::negotiate(
            adapter.limits,
            request.required_min_limits,
            request.optional_desired_limits,
        ),
    })
}

fn selection_key(adapter: &AdapterCapabilities) -> (u8, u64, u8, &str, usize) {
    let vram = adapter
        .identity
        .dedicated_vram_bytes
        .as_available()
        .copied()
        .unwrap_or(0);
    (
        adapter.identity.class.rank(),
        vram,
        adapter.identity.backend.rank(),
        adapter.identity.name.as_str(),
        adapter.identity.index,
    )
}

/// Auto-select: filter required, then deterministic rank.
///
/// VRAM participates only when at least one eligible adapter reports
/// `FieldAvailability::Available`; otherwise the VRAM key is zero for all and
/// ranking falls through to backend/name/index without fabricating sizes.
pub fn select_auto(
    adapters: &[AdapterCapabilities],
    request: &CapabilityRequest,
) -> Result<AdapterSelection, DomainError> {
    if adapters.is_empty() {
        return Err(no_adapter_error(
            "no GPU adapters were enumerated",
            "",
            "adapters=0",
        ));
    }

    let eligible: Vec<&AdapterCapabilities> = adapters
        .iter()
        .filter(|a| is_eligible(a, request))
        .collect();
    if eligible.is_empty() {
        let detail = adapters
            .iter()
            .map(|a| {
                let miss = missing_required(a, request);
                format!("{}:missing={}", a.identity.name, miss.join("+"))
            })
            .collect::<Vec<_>>()
            .join(";");
        return Err(no_adapter_error(
            format!("no adapter satisfies required features/limits ({})", detail),
            "",
            &detail,
        ));
    }

    let any_vram = eligible
        .iter()
        .any(|a| a.identity.dedicated_vram_bytes.is_available());
    let best = eligible
        .into_iter()
        .max_by(|a, b| {
            let mut ka = selection_key(a);
            let mut kb = selection_key(b);
            if !any_vram {
                ka.1 = 0;
                kb.1 = 0;
            }
            // max_by: higher class/vram/backend first; for equal, lower name/index
            // via reverse compare on name/index — use explicit cmp.
            ka.0.cmp(&kb.0)
                .then(ka.1.cmp(&kb.1))
                .then(ka.2.cmp(&kb.2))
                .then(kb.3.cmp(ka.3)) // lexicographically smaller name wins on tie
                .then(kb.4.cmp(&ka.4)) // smaller index wins
        })
        .expect("eligible non-empty");

    let negotiated = negotiate(best, request)?;
    Ok(AdapterSelection {
        adapter: best.clone(),
        negotiated,
    })
}

fn no_adapter_error(message: impl Into<String>, device: &str, actual: &str) -> DomainError {
    let mut err = DomainError::try_new(ErrorCode::NoAdapter, message)
        .expect("non-empty message")
        .with_user_action(UserAction::CorrectInput);
    if !device.is_empty() {
        err = err
            .with_context(ContextKey::Device, device.to_owned())
            .expect("device context once");
    }
    err.with_context(ContextKey::Field, "required_capabilities")
        .expect("field context once")
        .with_context(ContextKey::Actual, actual.to_owned())
        .expect("actual context once")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(
        index: usize,
        name: &str,
        class: AdapterClass,
        backend: BackendKind,
        features: FeatureBits,
        limits: AdapterLimits,
        vram: FieldAvailability<u64>,
    ) -> AdapterCapabilities {
        AdapterCapabilities {
            identity: AdapterIdentity {
                index,
                name: name.to_owned(),
                backend,
                class,
                vendor: 0,
                device: 0,
                driver: "test".into(),
                driver_info: String::new(),
                dedicated_vram_bytes: vram,
                uuid: FieldAvailability::Unavailable {
                    reason: "wgpu provides no portable adapter UUID",
                },
            },
            features,
            limits,
        }
    }

    fn basic_limits() -> AdapterLimits {
        AdapterLimits {
            max_buffer_size: 1 << 30,
            max_storage_buffers_per_shader_stage: 8,
            max_compute_workgroup_storage_size: 16_384,
        }
    }

    #[test]
    fn empty_adapters_yield_sp_gpu_001() {
        let err = select_auto(&[], &CapabilityRequest::diagnose_default()).unwrap_err();
        assert_eq!(err.code(), ErrorCode::NoAdapter);
        assert_eq!(err.code().as_str(), "SP-GPU-001");
    }

    #[test]
    fn required_feature_miss_rejects_without_silent_downgrade() {
        let adapters = vec![sample(
            0,
            "cpu",
            AdapterClass::Cpu,
            BackendKind::Vulkan,
            FeatureBits::default(),
            basic_limits(),
            FieldAvailability::Unavailable {
                reason: "synthetic",
            },
        )];
        let mut req = CapabilityRequest::diagnose_default();
        req.required_features = vec![CapabilityId::Subgroup];
        let err = select_auto(&adapters, &req).unwrap_err();
        assert_eq!(err.code(), ErrorCode::NoAdapter);
        assert!(
            err.message().contains("subgroup") || err.context_value(ContextKey::Actual).is_some()
        );
    }

    #[test]
    fn optional_miss_keeps_adapter_with_fallback() {
        let adapters = vec![sample(
            0,
            "igpu",
            AdapterClass::IntegratedGpu,
            BackendKind::Metal,
            FeatureBits::default(),
            basic_limits(),
            FieldAvailability::Unavailable {
                reason: "synthetic",
            },
        )];
        let sel = select_auto(&adapters, &CapabilityRequest::diagnose_default()).unwrap();
        assert_eq!(sel.negotiated.subgroup, SubgroupPlan::FallbackNonSubgroup);
        assert_eq!(sel.negotiated.shader_f16, F16Plan::FallbackF32);
        assert_eq!(sel.negotiated.timestamp, TimestampPlan::FallbackHostTimer);
        assert_eq!(sel.negotiated.float_atomic, FloatAtomicPlan::FallbackCas);
        assert!(sel.negotiated.limits.max_buffer_size.desired.is_none());
        assert!(sel.negotiated.limits.max_buffer_size.desired_met);
        assert_eq!(
            sel.negotiated.limits.max_buffer_size.effective,
            basic_limits().max_buffer_size
        );
    }

    #[test]
    fn limit_floor_rejects_undersized_adapter() {
        let adapters = vec![sample(
            0,
            "tiny",
            AdapterClass::DiscreteGpu,
            BackendKind::Vulkan,
            FeatureBits {
                subgroup: true,
                shader_f16: true,
                timestamp_query: true,
                float32_atomic_add: true,
            },
            AdapterLimits {
                max_buffer_size: 1024,
                max_storage_buffers_per_shader_stage: 1,
                max_compute_workgroup_storage_size: 16,
            },
            FieldAvailability::Unavailable {
                reason: "synthetic",
            },
        )];
        let mut req = CapabilityRequest::default();
        req.required_min_limits.max_buffer_size = Some(1 << 20);
        let err = select_auto(&adapters, &req).unwrap_err();
        assert_eq!(err.code(), ErrorCode::NoAdapter);
    }

    #[test]
    fn ranking_prefers_discrete_then_backend_then_name() {
        let limits = basic_limits();
        let feats = FeatureBits::default();
        let unavailable = FieldAvailability::Unavailable {
            reason: "synthetic no vram",
        };
        let adapters = vec![
            sample(
                0,
                "zeta",
                AdapterClass::IntegratedGpu,
                BackendKind::Metal,
                feats,
                limits,
                unavailable.clone(),
            ),
            sample(
                1,
                "alpha",
                AdapterClass::DiscreteGpu,
                BackendKind::Vulkan,
                feats,
                limits,
                unavailable.clone(),
            ),
            sample(
                2,
                "beta",
                AdapterClass::DiscreteGpu,
                BackendKind::Metal,
                feats,
                limits,
                unavailable,
            ),
        ];
        let sel = select_auto(&adapters, &CapabilityRequest::diagnose_default()).unwrap();
        // Discrete Metal "beta" beats discrete Vulkan "alpha".
        assert_eq!(sel.adapter.identity.name, "beta");
        assert_eq!(sel.adapter.identity.backend, BackendKind::Metal);
    }

    #[test]
    fn vram_tie_break_only_when_available() {
        let limits = basic_limits();
        let feats = FeatureBits::default();
        let adapters = vec![
            sample(
                0,
                "a",
                AdapterClass::DiscreteGpu,
                BackendKind::Vulkan,
                feats,
                limits,
                FieldAvailability::Available(1_000),
            ),
            sample(
                1,
                "b",
                AdapterClass::DiscreteGpu,
                BackendKind::Vulkan,
                feats,
                limits,
                FieldAvailability::Available(8_000),
            ),
        ];
        let sel = select_auto(&adapters, &CapabilityRequest::diagnose_default()).unwrap();
        assert_eq!(sel.adapter.identity.name, "b");
    }

    #[test]
    fn staged_reduce_fallback_when_requested() {
        let adapters = vec![sample(
            0,
            "igpu",
            AdapterClass::IntegratedGpu,
            BackendKind::Metal,
            FeatureBits::default(),
            basic_limits(),
            FieldAvailability::Unavailable {
                reason: "synthetic",
            },
        )];
        let mut req = CapabilityRequest::diagnose_default();
        req.prefer_staged_reduce_fallback = true;
        let sel = select_auto(&adapters, &req).unwrap();
        assert_eq!(
            sel.negotiated.float_atomic,
            FloatAtomicPlan::FallbackStagedReduce
        );
    }

    #[test]
    fn optional_limit_desire_over_device_clamps_without_reject() {
        let device = AdapterLimits {
            max_buffer_size: 4 << 20,
            max_storage_buffers_per_shader_stage: 4,
            max_compute_workgroup_storage_size: 8_192,
        };
        let adapters = vec![sample(
            0,
            "mid",
            AdapterClass::DiscreteGpu,
            BackendKind::Vulkan,
            FeatureBits::default(),
            device,
            FieldAvailability::Unavailable {
                reason: "synthetic",
            },
        )];
        let mut req = CapabilityRequest::diagnose_default();
        req.optional_desired_limits.max_buffer_size = Some(64 << 20);
        req.optional_desired_limits
            .max_storage_buffers_per_shader_stage = Some(16);
        let sel = select_auto(&adapters, &req).unwrap();
        let lim = sel.negotiated.limits;
        assert!(!lim.max_buffer_size.desired_met);
        assert_eq!(lim.max_buffer_size.desired, Some(64 << 20));
        assert_eq!(lim.max_buffer_size.effective, device.max_buffer_size);
        assert!(!lim.max_storage_buffers_per_shader_stage.desired_met);
        assert_eq!(
            lim.max_storage_buffers_per_shader_stage.effective,
            device.max_storage_buffers_per_shader_stage
        );
        // Unrequested field stays honest device capacity.
        assert!(lim.max_compute_workgroup_storage_size.desired_met);
        assert_eq!(
            lim.max_compute_workgroup_storage_size.effective,
            device.max_compute_workgroup_storage_size
        );
    }

    #[test]
    fn optional_limit_met_reports_desired_as_effective() {
        let device = basic_limits();
        let adapters = vec![sample(
            0,
            "fat",
            AdapterClass::DiscreteGpu,
            BackendKind::Metal,
            FeatureBits::default(),
            device,
            FieldAvailability::Unavailable {
                reason: "synthetic",
            },
        )];
        let mut req = CapabilityRequest::diagnose_default();
        req.optional_desired_limits.max_buffer_size = Some(1 << 20);
        let sel = select_auto(&adapters, &req).unwrap();
        let buf = sel.negotiated.limits.max_buffer_size;
        assert!(buf.desired_met);
        assert_eq!(buf.desired, Some(1 << 20));
        assert_eq!(buf.effective, 1 << 20);
        assert!(buf.effective <= device.max_buffer_size);
    }

    #[test]
    fn required_limit_outranks_optional_desire() {
        // Device meets optional desire but misses required floor → reject.
        let adapters = vec![sample(
            0,
            "almost",
            AdapterClass::DiscreteGpu,
            BackendKind::Vulkan,
            FeatureBits::default(),
            AdapterLimits {
                max_buffer_size: 8 << 20,
                max_storage_buffers_per_shader_stage: 8,
                max_compute_workgroup_storage_size: 16_384,
            },
            FieldAvailability::Unavailable {
                reason: "synthetic",
            },
        )];
        let mut req = CapabilityRequest::diagnose_default();
        req.required_min_limits.max_buffer_size = Some(32 << 20);
        req.optional_desired_limits.max_buffer_size = Some(4 << 20);
        let err = select_auto(&adapters, &req).unwrap_err();
        assert_eq!(err.code(), ErrorCode::NoAdapter);
        assert!(
            err.message().contains("max_buffer_size")
                || err
                    .context_value(ContextKey::Actual)
                    .is_some_and(|a| a.contains("max_buffer_size"))
        );
    }

    #[test]
    fn required_floor_met_optional_unmet_keeps_adapter() {
        let device = AdapterLimits {
            max_buffer_size: 8 << 20,
            max_storage_buffers_per_shader_stage: 8,
            max_compute_workgroup_storage_size: 16_384,
        };
        let adapters = vec![sample(
            0,
            "ok-req",
            AdapterClass::IntegratedGpu,
            BackendKind::Metal,
            FeatureBits::default(),
            device,
            FieldAvailability::Unavailable {
                reason: "synthetic",
            },
        )];
        let mut req = CapabilityRequest::diagnose_default();
        req.required_min_limits.max_buffer_size = Some(4 << 20);
        req.optional_desired_limits.max_buffer_size = Some(64 << 20);
        let sel = select_auto(&adapters, &req).unwrap();
        assert!(sel.adapter.limits.max_buffer_size >= 4 << 20);
        assert!(!sel.negotiated.limits.max_buffer_size.desired_met);
        assert_eq!(
            sel.negotiated.limits.max_buffer_size.effective,
            device.max_buffer_size
        );
        assert!(sel.negotiated.limits.max_buffer_size.effective >= 4 << 20);
    }

    #[test]
    fn eligible_optional_below_required_does_not_lower_effective() {
        // Independent probe: cap > required > optional desire on all three fields.
        let device = AdapterLimits {
            max_buffer_size: 64 << 20,
            max_storage_buffers_per_shader_stage: 8,
            max_compute_workgroup_storage_size: 16_384,
        };
        let adapters = vec![sample(
            0,
            "overlap",
            AdapterClass::DiscreteGpu,
            BackendKind::Vulkan,
            FeatureBits::default(),
            device,
            FieldAvailability::Unavailable {
                reason: "synthetic",
            },
        )];
        let mut req = CapabilityRequest::diagnose_default();
        req.required_min_limits = LimitFloor {
            max_buffer_size: Some(32 << 20),
            max_storage_buffers_per_shader_stage: Some(6),
            max_compute_workgroup_storage_size: Some(8_192),
        };
        req.optional_desired_limits = LimitFloor {
            max_buffer_size: Some(4 << 20),
            max_storage_buffers_per_shader_stage: Some(2),
            max_compute_workgroup_storage_size: Some(4_096),
        };
        assert!(is_eligible(&adapters[0], &req));
        let sel = select_auto(&adapters, &req).unwrap();
        let lim = sel.negotiated.limits;

        assert!(lim.max_buffer_size.desired_met); // adapter ≥ optional desire
        assert_eq!(lim.max_buffer_size.desired, Some(4 << 20));
        assert_eq!(lim.max_buffer_size.effective, 32 << 20); // raised to required
        assert!(lim.max_buffer_size.effective <= device.max_buffer_size);

        assert!(lim.max_storage_buffers_per_shader_stage.desired_met);
        assert_eq!(lim.max_storage_buffers_per_shader_stage.desired, Some(2));
        assert_eq!(lim.max_storage_buffers_per_shader_stage.effective, 6);
        assert!(lim.max_storage_buffers_per_shader_stage.effective <= 8);

        assert!(lim.max_compute_workgroup_storage_size.desired_met);
        assert_eq!(lim.max_compute_workgroup_storage_size.desired, Some(4_096));
        assert_eq!(lim.max_compute_workgroup_storage_size.effective, 8_192);
        assert!(lim.max_compute_workgroup_storage_size.effective <= 16_384);
    }

    #[test]
    fn resolve_equal_required_and_optional_uses_that_value() {
        let device = basic_limits();
        let adapters = vec![sample(
            0,
            "eq",
            AdapterClass::DiscreteGpu,
            BackendKind::Metal,
            FeatureBits::default(),
            device,
            FieldAvailability::Unavailable {
                reason: "synthetic",
            },
        )];
        let mut req = CapabilityRequest::diagnose_default();
        req.required_min_limits.max_buffer_size = Some(1 << 20);
        req.optional_desired_limits.max_buffer_size = Some(1 << 20);
        let sel = select_auto(&adapters, &req).unwrap();
        let buf = sel.negotiated.limits.max_buffer_size;
        assert!(buf.desired_met);
        assert_eq!(buf.effective, 1 << 20);
    }
}
