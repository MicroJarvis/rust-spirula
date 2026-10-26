//! Session-owned GPU context: instance → adapter → device/queue → close.
//!
//! Ownership is explicit. Lost notices are typed and never mutate application
//! session state. Rebuild requires an owner call to [`GpuContext::reopen`] or
//! a fresh [`GpuContext::open`] — GPU never auto-recreates the device.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

use spirula_core::{ContextKey, DomainError, ErrorCode, UserAction};

use crate::adapter::{
    AdapterCapabilities, CapabilityRequest, F16Plan, FeatureBits, FloatAtomicPlan, NegotiatedPlan,
    ResolvedLimits, SubgroupPlan, TimestampPlan, select_auto,
};
use crate::enumerate::{
    WGPU_VERSION, enumerate_on_instance, map_adapter, new_instance, paired_live_adapter,
};

/// Opaque context identity; unique per [`GpuContext`] value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContextId(u64);

impl ContextId {
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Generation bound to a live device; bumps on successful [`GpuContext::reopen`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContextGeneration(u64);

impl ContextGeneration {
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Opaque handle for device-gated APIs. Carries identity + generation only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceHandle {
    context_id: ContextId,
    generation: ContextGeneration,
}

impl DeviceHandle {
    pub fn context_id(self) -> ContextId {
        self.context_id
    }

    pub fn generation(self) -> ContextGeneration {
        self.generation
    }
}

/// Observable lifecycle status for diagnose / tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextStatus {
    Live,
    Closed,
    Lost,
    CreateFailed,
}

/// Typed lost notice — GPU-local only; never writes application session state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LostNotice {
    pub context_id: ContextId,
    pub generation: ContextGeneration,
    pub reason: String,
    pub source: LostSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LostSource {
    /// Host/driver callback observed on a real device.
    HostCallback,
    /// Test / diagnose injection (not a host callback).
    SyntheticInject,
}

/// Snapshot after a successful open (device/queue present until close/lost).
#[derive(Debug, Clone)]
pub struct DeviceOpenReport {
    pub context_id: ContextId,
    pub generation: ContextGeneration,
    pub status: ContextStatus,
    pub wgpu_version: &'static str,
    pub adapter: AdapterCapabilities,
    pub negotiated: NegotiatedPlan,
    pub requested_features: FeatureBits,
    pub enabled_features: FeatureBits,
    pub requested_limits: ResolvedLimits,
    pub device_limits: DeviceLimitsSnapshot,
    pub queue_present: bool,
    pub backend: crate::adapter::BackendKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceLimitsSnapshot {
    pub max_buffer_size: u64,
    pub max_storage_buffers_per_shader_stage: u32,
    pub max_compute_workgroup_storage_size: u32,
}

/// Failure detail when open does not reach Live.
#[derive(Debug)]
pub struct OpenFailureReport {
    pub error: DomainError,
    pub stage: OpenFailureStage,
    pub adapter: Option<AdapterCapabilities>,
    pub negotiated: Option<NegotiatedPlan>,
}

/// Boxed because [`OpenFailureReport`] is large for `Result` error variants.
pub type OpenError = Box<OpenFailureReport>;

fn open_err(
    error: DomainError,
    stage: OpenFailureStage,
    adapter: Option<AdapterCapabilities>,
    negotiated: Option<NegotiatedPlan>,
) -> OpenError {
    Box::new(OpenFailureReport {
        error,
        stage,
        adapter,
        negotiated,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenFailureStage {
    EnumerateEmpty,
    Select,
    LiveMatch,
    DeviceCreate,
}

static NEXT_CONTEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Host device-lost latch. Triggered state and reason share one mutex so a
/// consumer cannot observe "lost" with an empty reason (the prior flag-then-
/// message write order could lose the real callback string forever).
pub(crate) struct LostWatch {
    notice: Arc<Mutex<Option<String>>>,
}

impl LostWatch {
    fn new() -> Self {
        Self {
            notice: Arc::new(Mutex::new(None)),
        }
    }

    /// Record a lost reason once. Presence of `Some(reason)` is the trigger
    /// (no separate flag). Used by the wgpu callback and unit tests.
    fn deliver(&self, reason: String) {
        if let Ok(mut slot) = self.notice.lock()
            && slot.is_none()
        {
            *slot = Some(reason);
        }
    }

    fn arm_callback(&self, device: &wgpu::Device) {
        let watch = LostWatch {
            notice: Arc::clone(&self.notice),
        };
        device.set_device_lost_callback(move |_reason, msg| {
            watch.deliver(msg);
        });
    }

    /// Take the pending reason exactly once. `None` if not yet lost / already taken.
    fn take_triggered(&self) -> Option<String> {
        self.notice.lock().ok().and_then(|mut g| g.take())
    }
}

pub(crate) enum Backend {
    Wgpu {
        _instance: wgpu::Instance,
        device: wgpu::Device,
        queue: wgpu::Queue,
        lost: LostWatch,
    },
    /// Synthetic backend for fault-injection tests (no wgpu device).
    Synthetic,
}

/// Session-owned GPU context. Not a process-global singleton.
pub struct GpuContext {
    id: ContextId,
    generation: ContextGeneration,
    status: ContextStatus,
    pub(crate) backend: Option<Backend>,
    report: Option<DeviceOpenReport>,
    lost_notices: Vec<LostNotice>,
    closed: bool,
    pub(crate) resource_ledger: std::sync::Arc<std::sync::Mutex<crate::resource::ResourceLedger>>,
}

impl GpuContext {
    /// Cloneable wgpu device/queue handles when on the real backend.
    pub(crate) fn backend_device_queue(&self) -> Option<(wgpu::Device, wgpu::Queue)> {
        match &self.backend {
            Some(Backend::Wgpu { device, queue, .. }) => Some((device.clone(), queue.clone())),
            _ => None,
        }
    }

    fn fresh_ledger(
        id: ContextId,
        generation: ContextGeneration,
    ) -> std::sync::Arc<std::sync::Mutex<crate::resource::ResourceLedger>> {
        std::sync::Arc::new(std::sync::Mutex::new(crate::resource::ResourceLedger::new(
            id, generation,
        )))
    }

    fn reset_resources_for_generation(&mut self) {
        if let Ok(mut led) = self.resource_ledger.lock() {
            led.invalidate_all();
        }
        self.resource_ledger = Self::fresh_ledger(self.id, self.generation);
    }
}

impl GpuContext {
    /// Open a real host context on a fresh instance: enumerate, select, negotiate,
    /// request device with negotiated features/limits.
    pub fn open(request: &CapabilityRequest) -> Result<Self, OpenError> {
        let id = ContextId(NEXT_CONTEXT_ID.fetch_add(1, Ordering::SeqCst));
        match create_live_backend(request) {
            Ok((backend, report)) => Ok(Self {
                id,
                generation: ContextGeneration(1),
                status: ContextStatus::Live,
                backend: Some(backend),
                report: Some(with_ids(report, id, ContextGeneration(1))),
                lost_notices: Vec::new(),
                closed: false,
                resource_ledger: Self::fresh_ledger(id, ContextGeneration(1)),
            }),
            Err(failure) => Err(failure),
        }
    }

    /// Synthetic open for fault injection (no host GPU). Not used by host diagnose.
    pub fn open_synthetic(
        request: &CapabilityRequest,
        adapter: AdapterCapabilities,
        fail_device_create: bool,
    ) -> Result<Self, OpenError> {
        let selection = match select_auto(std::slice::from_ref(&adapter), request) {
            Ok(sel) => sel,
            Err(error) => {
                return Err(open_err(
                    error,
                    OpenFailureStage::Select,
                    Some(adapter),
                    None,
                ));
            }
        };

        if fail_device_create {
            return Err(open_err(
                device_create_error("synthetic device create failure"),
                OpenFailureStage::DeviceCreate,
                Some(selection.adapter),
                Some(selection.negotiated),
            ));
        }

        let id = ContextId(NEXT_CONTEXT_ID.fetch_add(1, Ordering::SeqCst));
        let generation = ContextGeneration(1);
        let requested_features = features_from_plan(&selection.negotiated);
        let report = DeviceOpenReport {
            context_id: id,
            generation,
            status: ContextStatus::Live,
            wgpu_version: WGPU_VERSION,
            backend: selection.adapter.identity.backend,
            requested_features,
            enabled_features: requested_features,
            requested_limits: selection.negotiated.limits,
            device_limits: DeviceLimitsSnapshot {
                max_buffer_size: selection.negotiated.limits.max_buffer_size.effective,
                max_storage_buffers_per_shader_stage: selection
                    .negotiated
                    .limits
                    .max_storage_buffers_per_shader_stage
                    .effective,
                max_compute_workgroup_storage_size: selection
                    .negotiated
                    .limits
                    .max_compute_workgroup_storage_size
                    .effective,
            },
            queue_present: true,
            adapter: selection.adapter,
            negotiated: selection.negotiated,
        };

        Ok(Self {
            id,
            generation,
            status: ContextStatus::Live,
            backend: Some(Backend::Synthetic),
            report: Some(report),
            lost_notices: Vec::new(),
            closed: false,
            resource_ledger: Self::fresh_ledger(id, generation),
        })
    }

    pub fn id(&self) -> ContextId {
        self.id
    }

    pub fn generation(&self) -> ContextGeneration {
        self.generation
    }

    pub fn status(&mut self) -> ContextStatus {
        self.poll_host_lost();
        self.status
    }

    pub fn report(&self) -> Option<&DeviceOpenReport> {
        self.report.as_ref()
    }

    pub fn lost_notices(&self) -> &[LostNotice] {
        &self.lost_notices
    }

    /// Drain recorded lost notices (GPU-local; owner decides SessionState).
    pub fn take_lost_notices(&mut self) -> Vec<LostNotice> {
        self.poll_host_lost();
        std::mem::take(&mut self.lost_notices)
    }

    /// Issue a device handle for the current Live generation.
    pub fn device_handle(&mut self) -> Result<DeviceHandle, DomainError> {
        self.poll_host_lost();
        if self.status == ContextStatus::Live && !self.closed {
            return Ok(DeviceHandle {
                context_id: self.id,
                generation: self.generation,
            });
        }
        Err(handle_unavailable_error(self.status))
    }

    /// Validate a handle against this context (cross-context / stale / closed / lost).
    pub fn validate_handle(&mut self, handle: DeviceHandle) -> Result<(), DomainError> {
        self.poll_host_lost();
        if handle.context_id != self.id {
            return Err(DomainError::try_new(
                ErrorCode::InvalidArgument,
                "device handle belongs to a different context",
            )
            .expect("non-empty")
            .with_user_action(UserAction::CorrectInput)
            .with_context(ContextKey::Field, "device_handle")
            .expect("field once"));
        }
        if handle.generation != self.generation {
            return Err(DomainError::try_new(
                ErrorCode::DeviceLost,
                "device handle generation is stale",
            )
            .expect("non-empty")
            .with_user_action(UserAction::RecreateDevice));
        }
        match self.status {
            ContextStatus::Live if !self.closed => Ok(()),
            ContextStatus::Lost => Err(DomainError::try_new(
                ErrorCode::DeviceLost,
                "device lost; handle rejected",
            )
            .expect("non-empty")
            .with_user_action(UserAction::RecreateDevice)),
            _ => Err(DomainError::try_new(
                ErrorCode::DeviceLost,
                "context not live; handle rejected",
            )
            .expect("non-empty")
            .with_user_action(UserAction::RecreateDevice)),
        }
    }

    /// Explicit close. Idempotent. Drops device/queue; does not reopen.
    pub fn close(&mut self) {
        if self.closed && self.backend.is_none() {
            return;
        }
        self.closed = true;
        if self.status == ContextStatus::Live {
            self.status = ContextStatus::Closed;
        }
        if let Ok(mut led) = self.resource_ledger.lock() {
            led.invalidate_all();
        }
        self.backend = None;
        if let Some(report) = self.report.as_mut() {
            report.status = self.status;
            report.queue_present = false;
        }
    }

    /// Scene/engine reset must not close the device — no-op on GPU ownership
    /// until P2-F03 owns scene resources (C++ EngineState::reset vs destroy).
    pub fn reset_scene(&mut self) {
        // Intentionally empty in P2-F02.
    }

    /// Inject a synthetic lost notice. Distinct from [`LostSource::HostCallback`].
    pub fn inject_lost(&mut self, reason: impl Into<String>) {
        if self.closed || self.status != ContextStatus::Live {
            return;
        }
        self.lost_notices.push(LostNotice {
            context_id: self.id,
            generation: self.generation,
            reason: reason.into(),
            source: LostSource::SyntheticInject,
        });
        self.status = ContextStatus::Lost;
        if let Some(report) = self.report.as_mut() {
            report.status = ContextStatus::Lost;
            report.queue_present = false;
        }
        if let Ok(mut led) = self.resource_ledger.lock() {
            led.invalidate_all();
        }
        self.backend = None;
    }

    /// Poll host lost flag into a typed notice (no application SessionState mutation).
    pub fn poll_host_lost(&mut self) {
        let triggered = match &self.backend {
            Some(Backend::Wgpu { lost, .. }) => lost.take_triggered(),
            _ => None,
        };
        if let Some(reason) = triggered
            && self.status == ContextStatus::Live
        {
            self.lost_notices.push(LostNotice {
                context_id: self.id,
                generation: self.generation,
                reason,
                source: LostSource::HostCallback,
            });
            self.status = ContextStatus::Lost;
            if let Some(report) = self.report.as_mut() {
                report.status = ContextStatus::Lost;
                report.queue_present = false;
            }
            if let Ok(mut led) = self.resource_ledger.lock() {
                led.invalidate_all();
            }
            self.backend = None;
        }
    }
}

fn create_live_backend(
    request: &CapabilityRequest,
) -> Result<(Backend, DeviceOpenReport), OpenError> {
    let instance = new_instance();
    let (reports, live) = enumerate_on_instance(&instance);
    if reports.is_empty() {
        return Err(open_err(
            DomainError::try_new(ErrorCode::NoAdapter, "no adapters enumerated on host")
                .expect("non-empty")
                .with_user_action(UserAction::CorrectInput)
                .with_context(ContextKey::Actual, "adapters=0")
                .expect("actual once"),
            OpenFailureStage::EnumerateEmpty,
            None,
            None,
        ));
    }

    let selection = match select_auto(&reports, request) {
        Ok(sel) => sel,
        Err(error) => {
            return Err(open_err(error, OpenFailureStage::Select, None, None));
        }
    };

    let live_adapter = match paired_live_adapter(&reports, &live, &selection.adapter) {
        Some(a) => a,
        None => {
            return Err(open_err(
                DomainError::try_new(
                    ErrorCode::NoAdapter,
                    "selected adapter not paired in this enumerate snapshot (external/stale index rejected)",
                )
                .expect("non-empty")
                .with_user_action(UserAction::CorrectInput),
                OpenFailureStage::LiveMatch,
                Some(selection.adapter),
                Some(selection.negotiated),
            ));
        }
    };

    let adapter_caps = map_adapter(selection.adapter.identity.index, live_adapter);
    let plan = selection.negotiated;
    let requested_features = features_from_plan(&plan);
    let wgpu_features = features_to_wgpu(&requested_features);

    let mut limits = live_adapter.limits();
    limits.max_buffer_size = plan.limits.max_buffer_size.effective;
    limits.max_storage_buffers_per_shader_stage =
        plan.limits.max_storage_buffers_per_shader_stage.effective;
    limits.max_compute_workgroup_storage_size =
        plan.limits.max_compute_workgroup_storage_size.effective;

    let descriptor = wgpu::DeviceDescriptor {
        label: Some("spirula:gpu:device:session"),
        required_features: wgpu_features,
        required_limits: limits,
        experimental_features: wgpu::ExperimentalFeatures::default(),
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::Off,
    };

    let (device, queue) = match pollster::block_on(live_adapter.request_device(&descriptor)) {
        Ok(pair) => pair,
        Err(err) => {
            return Err(open_err(
                device_create_error(format!("device request failed: {err}")),
                OpenFailureStage::DeviceCreate,
                Some(adapter_caps),
                Some(plan),
            ));
        }
    };

    let lost = LostWatch::new();
    lost.arm_callback(&device);

    let enabled_features = features_from_wgpu(device.features());
    let device_limits = device.limits();
    let report = DeviceOpenReport {
        context_id: ContextId(0),
        generation: ContextGeneration(0),
        status: ContextStatus::Live,
        wgpu_version: WGPU_VERSION,
        backend: adapter_caps.identity.backend,
        requested_features,
        enabled_features,
        requested_limits: plan.limits,
        device_limits: DeviceLimitsSnapshot {
            max_buffer_size: device_limits.max_buffer_size,
            max_storage_buffers_per_shader_stage: device_limits
                .max_storage_buffers_per_shader_stage,
            max_compute_workgroup_storage_size: device_limits.max_compute_workgroup_storage_size,
        },
        queue_present: true,
        adapter: adapter_caps,
        negotiated: plan,
    };

    Ok((
        Backend::Wgpu {
            _instance: instance,
            device,
            queue,
            lost,
        },
        report,
    ))
}

fn with_ids(
    mut report: DeviceOpenReport,
    id: ContextId,
    generation: ContextGeneration,
) -> DeviceOpenReport {
    report.context_id = id;
    report.generation = generation;
    report
}

fn device_create_error(message: impl Into<String>) -> DomainError {
    DomainError::try_new(ErrorCode::AllocationFailed, message)
        .expect("non-empty")
        .with_user_action(UserAction::Retry)
}

fn handle_unavailable_error(status: ContextStatus) -> DomainError {
    let msg = match status {
        ContextStatus::Lost => "device lost; handle unavailable",
        ContextStatus::Closed => "context closed; handle unavailable",
        ContextStatus::CreateFailed => "create failed; handle unavailable",
        ContextStatus::Live => "context closed; handle unavailable",
    };
    DomainError::try_new(ErrorCode::DeviceLost, msg)
        .expect("non-empty")
        .with_user_action(UserAction::RecreateDevice)
}

fn features_from_plan(plan: &NegotiatedPlan) -> FeatureBits {
    FeatureBits {
        subgroup: matches!(plan.subgroup, SubgroupPlan::AvailableDeclared),
        shader_f16: matches!(plan.shader_f16, F16Plan::AvailableDeclared),
        timestamp_query: matches!(plan.timestamp, TimestampPlan::AvailableDeclared),
        float32_atomic_add: matches!(plan.float_atomic, FloatAtomicPlan::AvailableDeclared),
    }
}

fn features_to_wgpu(bits: &FeatureBits) -> wgpu::Features {
    let mut f = wgpu::Features::empty();
    if bits.subgroup {
        f |= wgpu::Features::SUBGROUP;
    }
    if bits.shader_f16 {
        f |= wgpu::Features::SHADER_F16;
    }
    if bits.timestamp_query {
        f |= wgpu::Features::TIMESTAMP_QUERY;
    }
    if bits.float32_atomic_add {
        f |= wgpu::Features::SHADER_FLOAT32_ATOMIC;
    }
    f
}

fn features_from_wgpu(features: wgpu::Features) -> FeatureBits {
    FeatureBits {
        subgroup: features.contains(wgpu::Features::SUBGROUP),
        shader_f16: features.contains(wgpu::Features::SHADER_F16),
        timestamp_query: features.contains(wgpu::Features::TIMESTAMP_QUERY),
        float32_atomic_add: features.contains(wgpu::Features::SHADER_FLOAT32_ATOMIC),
    }
}

/// Format a device open report for diagnose stdout.
pub fn format_device_report(report: &DeviceOpenReport) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(out, "device_context: status={:?}", report.status);
    let _ = writeln!(
        out,
        "  context_id={} generation={} wgpu={}",
        report.context_id.get(),
        report.generation.get(),
        report.wgpu_version
    );
    let _ = writeln!(
        out,
        "  adapter[{}] {} {} vendor={} device={}",
        report.adapter.identity.index,
        report.adapter.identity.backend.as_str(),
        report.adapter.identity.class.as_str(),
        report.adapter.identity.vendor,
        report.adapter.identity.device
    );
    let _ = writeln!(out, "  name={}", report.adapter.identity.name);
    let _ = writeln!(
        out,
        "  requested_features: subgroup={} shader_f16={} timestamp_query={} float32_atomic_add={}",
        report.requested_features.subgroup,
        report.requested_features.shader_f16,
        report.requested_features.timestamp_query,
        report.requested_features.float32_atomic_add
    );
    let _ = writeln!(
        out,
        "  enabled_features: subgroup={} shader_f16={} timestamp_query={} float32_atomic_add={}",
        report.enabled_features.subgroup,
        report.enabled_features.shader_f16,
        report.enabled_features.timestamp_query,
        report.enabled_features.float32_atomic_add
    );
    let lim = &report.requested_limits;
    let _ = writeln!(
        out,
        "  requested_limits: max_buffer_size={} max_storage_buffers_per_shader_stage={} max_compute_workgroup_storage_size={}",
        lim.max_buffer_size.effective,
        lim.max_storage_buffers_per_shader_stage.effective,
        lim.max_compute_workgroup_storage_size.effective
    );
    let _ = writeln!(
        out,
        "  device_limits: max_buffer_size={} max_storage_buffers_per_shader_stage={} max_compute_workgroup_storage_size={}",
        report.device_limits.max_buffer_size,
        report.device_limits.max_storage_buffers_per_shader_stage,
        report.device_limits.max_compute_workgroup_storage_size
    );
    let _ = writeln!(out, "  queue_present={}", report.queue_present);
    let _ = writeln!(
        out,
        "  negotiated subgroup={:?} shader_f16={:?} timestamp={:?} float_atomic={:?}",
        report.negotiated.subgroup,
        report.negotiated.shader_f16,
        report.negotiated.timestamp,
        report.negotiated.float_atomic
    );
    out
}

/// Headless diagnose: open → print report → close.
pub fn diagnose_device_open(request: &CapabilityRequest) -> Result<DeviceOpenReport, OpenError> {
    let mut ctx = GpuContext::open(request)?;
    let report = ctx
        .report()
        .cloned()
        .expect("live context always has report");
    ctx.close();
    Ok(report)
}

impl Drop for GpuContext {
    fn drop(&mut self) {
        self.close();
    }
}

impl GpuContext {
    /// Align public report with a failed reopen/create: status + no queue.
    /// Keeps the previous report payload for diagnosis; does not delete it.
    fn mark_create_failed(&mut self) {
        self.status = ContextStatus::CreateFailed;
        self.closed = true;
        self.backend = None;
        if let Ok(mut led) = self.resource_ledger.lock() {
            led.invalidate_all();
        }
        if let Some(report) = self.report.as_mut() {
            report.status = ContextStatus::CreateFailed;
            report.queue_present = false;
        }
    }

    /// Owner-driven recreate on the same [`ContextId`]: bumps generation on
    /// success. Old handles remain invalid. Never invoked from lost callbacks.
    pub fn reopen(&mut self, request: &CapabilityRequest) -> Result<(), OpenError> {
        let prev_gen = self.generation;
        self.backend = None;
        self.closed = false;
        match create_live_backend(request) {
            Ok((backend, report)) => {
                let next_gen = ContextGeneration(prev_gen.get() + 1);
                self.generation = next_gen;
                self.status = ContextStatus::Live;
                self.backend = Some(backend);
                self.report = Some(with_ids(report, self.id, next_gen));
                self.reset_resources_for_generation();
                Ok(())
            }
            Err(failure) => {
                self.mark_create_failed();
                Err(failure)
            }
        }
    }

    /// Synthetic reopen (tests): bumps generation without host GPU.
    pub fn reopen_synthetic(
        &mut self,
        request: &CapabilityRequest,
        adapter: AdapterCapabilities,
        fail_device_create: bool,
    ) -> Result<(), OpenError> {
        let prev_gen = self.generation;
        self.backend = None;
        self.closed = false;
        if fail_device_create {
            self.mark_create_failed();
            return Err(open_err(
                device_create_error("synthetic device recreate failure"),
                OpenFailureStage::DeviceCreate,
                Some(adapter),
                None,
            ));
        }
        let selection = match select_auto(std::slice::from_ref(&adapter), request) {
            Ok(sel) => sel,
            Err(error) => {
                self.mark_create_failed();
                return Err(open_err(
                    error,
                    OpenFailureStage::Select,
                    Some(adapter),
                    None,
                ));
            }
        };
        let next_gen = ContextGeneration(prev_gen.get() + 1);
        let requested_features = features_from_plan(&selection.negotiated);
        self.generation = next_gen;
        self.status = ContextStatus::Live;
        self.backend = Some(Backend::Synthetic);
        self.report = Some(DeviceOpenReport {
            context_id: self.id,
            generation: next_gen,
            status: ContextStatus::Live,
            wgpu_version: WGPU_VERSION,
            backend: selection.adapter.identity.backend,
            requested_features,
            enabled_features: requested_features,
            requested_limits: selection.negotiated.limits,
            device_limits: DeviceLimitsSnapshot {
                max_buffer_size: selection.negotiated.limits.max_buffer_size.effective,
                max_storage_buffers_per_shader_stage: selection
                    .negotiated
                    .limits
                    .max_storage_buffers_per_shader_stage
                    .effective,
                max_compute_workgroup_storage_size: selection
                    .negotiated
                    .limits
                    .max_compute_workgroup_storage_size
                    .effective,
            },
            queue_present: true,
            adapter: selection.adapter,
            negotiated: selection.negotiated,
        });
        self.reset_resources_for_generation();
        Ok(())
    }
}

#[cfg(test)]
mod lost_watch_tests {
    use super::LostWatch;

    #[test]
    fn take_triggered_returns_real_reason_exactly_once() {
        let watch = LostWatch::new();
        watch.deliver("driver reset: unit-test".into());
        assert_eq!(
            watch.take_triggered().as_deref(),
            Some("driver reset: unit-test")
        );
        assert_eq!(
            watch.take_triggered(),
            None,
            "second take must not invent a fallback reason"
        );
    }

    #[test]
    fn deliver_under_lock_cannot_lose_reason_to_empty_take() {
        // Deterministic stand-in for the old flag.store-then-message.lock race:
        // a single Option under one mutex is both the trigger and the payload.
        let watch = LostWatch::new();
        watch.deliver("exact host reason".into());
        let first = watch.take_triggered();
        assert_eq!(first.as_deref(), Some("exact host reason"));
        assert!(watch.take_triggered().is_none());
    }

    #[test]
    fn new_watch_unaffected_by_prior_generation_deliver() {
        let previous = LostWatch::new();
        let next = LostWatch::new();
        previous.deliver("lost on generation N".into());
        assert!(
            next.take_triggered().is_none(),
            "reopen installs a fresh LostWatch; old callback state must not leak"
        );
        assert_eq!(
            previous.take_triggered().as_deref(),
            Some("lost on generation N")
        );
    }

    #[test]
    fn second_deliver_keeps_first_reason() {
        let watch = LostWatch::new();
        watch.deliver("first".into());
        watch.deliver("second".into());
        assert_eq!(watch.take_triggered().as_deref(), Some("first"));
    }
}
