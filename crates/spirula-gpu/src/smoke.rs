//! P2-F06: headless GPU smoke outcomes (compute + honest surface gate).
//!
//! Reuses P2-F04 affine compute and P2-F05 diagnostics without extending the
//! general resource/pipeline APIs. Surface smoke never forges a window.

use std::env;
use std::time::Duration;

use spirula_core::{DomainError, EXIT_FAILURE, EXIT_SUCCESS, ErrorCode};

use crate::WGPU_VERSION;
use crate::adapter::{BackendKind, CapabilityRequest, FeatureBits};
use crate::context::{
    ContextStatus, DeviceLimitsSnapshot, DeviceOpenReport, GpuContext, OpenError, OpenFailureStage,
};
use crate::diagnostics::DiagnosticKind;
use crate::pipeline::{AffineU32Params, cpu_affine_u32};
use crate::resource::{
    BufferDesc, BufferHandle, BufferUsage, RESOURCE_ACCOUNT_SOURCE, ReadbackTicket, ResourceLabel,
    SubmissionHandle,
};

/// Which smoke path was requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmokeKind {
    Compute,
    Surface,
}

/// Coarse outcome for scripts and CI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmokeStatus {
    /// Path completed successfully.
    Pass,
    /// Adapter/device unavailable — not a false pass.
    Skip,
    /// Typed failure observed (validation / lost / timeout / mismatch).
    Fail,
    /// Path not safely supported (e.g. surface without a real present target).
    Unsupported,
}

/// Structured smoke report (machine-readable lines via [`format_smoke_report`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmokeReport {
    pub kind: SmokeKind,
    pub status: SmokeStatus,
    pub wgpu_version: &'static str,
    pub reason: Option<String>,
    pub open_stage: Option<OpenFailureStage>,
    pub error_code: Option<String>,
    pub device_name: Option<String>,
    pub backend: Option<String>,
    pub adapter_class: Option<String>,
    pub features: Option<FeatureBits>,
    pub device_limits: Option<DeviceLimitsSnapshot>,
    pub match_cpu: Option<bool>,
    pub accounted_source: Option<&'static str>,
    pub after_drop_current_bytes: Option<u64>,
    pub retained_submissions: Option<usize>,
    pub diagnostic_error_count: Option<usize>,
    pub context_status: Option<ContextStatus>,
    pub repeat_index: u32,
}

impl SmokeReport {
    fn base(kind: SmokeKind, status: SmokeStatus) -> Self {
        Self {
            kind,
            status,
            wgpu_version: WGPU_VERSION,
            reason: None,
            open_stage: None,
            error_code: None,
            device_name: None,
            backend: None,
            adapter_class: None,
            features: None,
            device_limits: None,
            match_cpu: None,
            accounted_source: None,
            after_drop_current_bytes: None,
            retained_submissions: None,
            diagnostic_error_count: None,
            context_status: None,
            repeat_index: 0,
        }
    }

    fn with_device(mut self, report: &DeviceOpenReport) -> Self {
        self.device_name = Some(report.adapter.identity.name.clone());
        self.backend = Some(report.backend.as_str().to_owned());
        self.adapter_class = Some(report.adapter.identity.class.as_str().to_owned());
        self.features = Some(report.enabled_features);
        self.device_limits = Some(report.device_limits);
        self
    }
}

/// Process exit code for a smoke report.
///
/// `Pass`, `Skip`, and `Unsupported` → 0 (skip/unsupported are explicit, not
/// silent success). `Fail` → 1.
pub fn smoke_exit_code(report: &SmokeReport) -> u8 {
    match report.status {
        SmokeStatus::Pass | SmokeStatus::Skip | SmokeStatus::Unsupported => EXIT_SUCCESS,
        SmokeStatus::Fail => EXIT_FAILURE,
    }
}

/// Format a stable multi-line smoke report for stdout/stderr.
pub fn format_smoke_report(report: &SmokeReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "spirula-gpu-smoke kind={} status={} wgpu_version={} repeat={}\n",
        smoke_kind_str(report.kind),
        smoke_status_str(report.status),
        report.wgpu_version,
        report.repeat_index
    ));
    if let Some(name) = &report.device_name {
        out.push_str(&format!(
            "device name={name:?} backend={} class={}\n",
            report.backend.as_deref().unwrap_or("unknown"),
            report.adapter_class.as_deref().unwrap_or("unknown")
        ));
    }
    if let Some(f) = report.features {
        out.push_str(&format!(
            "features subgroup={} f16={} timestamp={} f32_atomic={}\n",
            f.subgroup, f.shader_f16, f.timestamp_query, f.float32_atomic_add
        ));
    }
    if let Some(lim) = report.device_limits {
        out.push_str(&format!(
            "device_limits max_buffer_size={} max_storage_buffers_per_shader_stage={} \
             max_compute_workgroup_storage_size={} max_compute_workgroups_per_dimension={} \
             max_texture_dimension_2d={}\n",
            lim.max_buffer_size,
            lim.max_storage_buffers_per_shader_stage,
            lim.max_compute_workgroup_storage_size,
            lim.max_compute_workgroups_per_dimension,
            lim.max_texture_dimension_2d
        ));
    }
    if let Some(stage) = report.open_stage {
        out.push_str(&format!("open_failure_stage={stage:?}\n"));
    }
    if let Some(code) = &report.error_code {
        out.push_str(&format!("error_code={code}\n"));
    }
    if let Some(reason) = &report.reason {
        out.push_str(&format!("reason={reason}\n"));
    }
    if let Some(m) = report.match_cpu {
        out.push_str(&format!(
            "match={}\n",
            if m { "cpu_exact" } else { "mismatch" }
        ));
    }
    if let Some(src) = report.accounted_source {
        out.push_str(&format!("accounted source={src}\n"));
    }
    if let Some(n) = report.after_drop_current_bytes {
        out.push_str(&format!("after_drop current={n}\n"));
    }
    if let Some(n) = report.retained_submissions {
        out.push_str(&format!("retained_submissions={n}\n"));
    }
    if let Some(n) = report.diagnostic_error_count {
        out.push_str(&format!("diagnostic_error_count={n}\n"));
    }
    if let Some(st) = report.context_status {
        out.push_str(&format!("context_status={st:?}\n"));
    }
    out.push_str("note: smoke only; not application/train/UI readiness; not P2-F07 packaging\n");
    out
}

fn smoke_kind_str(k: SmokeKind) -> &'static str {
    match k {
        SmokeKind::Compute => "compute",
        SmokeKind::Surface => "surface",
    }
}

fn smoke_status_str(s: SmokeStatus) -> &'static str {
    match s {
        SmokeStatus::Pass => "pass",
        SmokeStatus::Skip => "skip",
        SmokeStatus::Fail => "fail",
        SmokeStatus::Unsupported => "unsupported",
    }
}

fn label(s: &str) -> ResourceLabel {
    ResourceLabel::parse(s).expect("smoke label")
}

fn words_to_bytes(words: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(words.len() * 4);
    for w in words {
        out.extend_from_slice(&w.to_le_bytes());
    }
    out
}

fn bytes_to_words(bytes: &[u8]) -> Vec<u32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect()
}

fn skip_from_open(kind: SmokeKind, failure: OpenError) -> SmokeReport {
    let mut report = SmokeReport::base(kind, SmokeStatus::Skip);
    report.open_stage = Some(failure.stage);
    report.error_code = Some(failure.error.code().as_str().to_owned());
    report.reason = Some(format!(
        "adapter/device unavailable at {:?}: {}",
        failure.stage,
        failure.error.message()
    ));
    if let Some(adapter) = &failure.adapter {
        report.device_name = Some(adapter.identity.name.clone());
        report.backend = Some(adapter.identity.backend.as_str().to_owned());
        report.adapter_class = Some(adapter.identity.class.as_str().to_owned());
        report.features = Some(adapter.features);
    }
    report
}

fn fail_report(
    kind: SmokeKind,
    err: &DomainError,
    reason: impl Into<String>,
    ctx: Option<&mut GpuContext>,
) -> SmokeReport {
    let mut report = SmokeReport::base(kind, SmokeStatus::Fail);
    report.error_code = Some(err.code().as_str().to_owned());
    report.reason = Some(reason.into());
    if let Some(ctx) = ctx {
        if let Some(dev) = ctx.report() {
            report = report.with_device(dev);
        }
        report.context_status = Some(ctx.status());
        report.after_drop_current_bytes = Some(ctx.resource_accounting().current_bytes);
        report.retained_submissions = Some(ctx.retained_submission_count());
        report.diagnostic_error_count = Some(
            ctx.filter_diagnostics(0, Some(DiagnosticKind::Error), None)
                .len()
                + ctx
                    .filter_diagnostics(0, Some(DiagnosticKind::DeviceLost), None)
                    .len(),
        );
        report.accounted_source = Some(RESOURCE_ACCOUNT_SOURCE);
    }
    report
}

/// Headless compute smoke: affine upload→dispatch→readback vs CPU, then drop
/// to `current=0`. Unavailable open → precise [`SmokeStatus::Skip`].
pub fn run_compute_smoke(request: &CapabilityRequest) -> SmokeReport {
    run_compute_smoke_repeat(request, 0)
}

/// Same as [`run_compute_smoke`] with an explicit repeat index for logs.
pub fn run_compute_smoke_repeat(request: &CapabilityRequest, repeat_index: u32) -> SmokeReport {
    let mut ctx = match GpuContext::open(request) {
        Ok(c) => c,
        Err(failure) => {
            let mut r = skip_from_open(SmokeKind::Compute, failure);
            r.repeat_index = repeat_index;
            return r;
        }
    };
    match run_compute_smoke_on_context(&mut ctx) {
        Ok(mut report) => {
            report.repeat_index = repeat_index;
            report
        }
        Err(mut report) => {
            report.repeat_index = repeat_index;
            report
        }
    }
}

/// Locals held across the compute smoke path so Fail exits can reclaim.
struct SmokeLocals {
    params: Option<BufferHandle>,
    in_buf: Option<BufferHandle>,
    out_buf: Option<BufferHandle>,
    ticket: Option<ReadbackTicket>,
    submission: Option<SubmissionHandle>,
}

impl SmokeLocals {
    fn new() -> Self {
        Self {
            params: None,
            in_buf: None,
            out_buf: None,
            ticket: None,
            submission: None,
        }
    }

    /// Drop ticket (pending readback / abandon-on-complete), abandon
    /// **unsubmitted** pending encodes, wait any accepted submission, then drop
    /// handles. Never frees in-flight GPU work without wait.
    fn reclaim(&mut self, ctx: &mut GpuContext) {
        self.ticket = None;
        ctx.abandon_unsubmitted_pending();
        if let Some(h) = self.submission.take() {
            let _ = ctx.wait(h, Some(Duration::from_secs(5)));
        }
        self.params = None;
        self.in_buf = None;
        self.out_buf = None;
        // Holds cleared → destroy_pending buffers reclaim; clear leftover encodes.
        ctx.abandon_unsubmitted_pending();
    }

    fn fail(
        &mut self,
        ctx: &mut GpuContext,
        err: &DomainError,
        reason: impl Into<String>,
    ) -> SmokeReport {
        self.reclaim(ctx);
        fail_report(SmokeKind::Compute, err, reason, Some(ctx))
    }
}

/// Compute smoke against an already-open context (host or synthetic).
///
/// On every Fail path, unsubmitted pending is abandoned and accepted work is
/// waited so the context is reusable (`current=0`, `retained=0`) when still Live.
#[allow(clippy::result_large_err)]
pub fn run_compute_smoke_on_context(ctx: &mut GpuContext) -> Result<SmokeReport, SmokeReport> {
    let device = ctx.report().cloned().ok_or_else(|| {
        fail_report(
            SmokeKind::Compute,
            &DomainError::try_new(ErrorCode::DeviceLost, "smoke: context has no device report")
                .expect("non-empty"),
            "missing device report",
            Some(ctx),
        )
    })?;

    let count = 100u32;
    let input: Vec<u32> = (0..count)
        .map(|i| i.wrapping_mul(3).wrapping_add(1))
        .collect();
    let scale = 7u32;
    let bias = 11u32;
    let mode = 2u32;

    let mut locals = SmokeLocals::new();

    match ctx.create_buffer(BufferDesc {
        label: label("spirula:gpu:buffer:smoke-params"),
        size: 16,
        usage: BufferUsage::UNIFORM_UPLOAD,
    }) {
        Ok(b) => locals.params = Some(b),
        Err(e) => return Err(locals.fail(ctx, &e, "create params failed")),
    }
    match ctx.create_buffer(BufferDesc {
        label: label("spirula:gpu:buffer:smoke-in"),
        size: u64::from(count) * 4,
        usage: BufferUsage::STORAGE_COPY,
    }) {
        Ok(b) => locals.in_buf = Some(b),
        Err(e) => return Err(locals.fail(ctx, &e, "create input failed")),
    }
    match ctx.create_buffer(BufferDesc {
        label: label("spirula:gpu:buffer:smoke-out"),
        size: u64::from(count) * 4,
        usage: BufferUsage::STORAGE_COPY,
    }) {
        Ok(b) => locals.out_buf = Some(b),
        Err(e) => return Err(locals.fail(ctx, &e, "create output failed")),
    }

    if let Err(e) = ctx.upload_buffer(
        locals.params.as_ref().expect("params"),
        0,
        &AffineU32Params::new(scale, bias, count).as_bytes(),
    ) {
        return Err(locals.fail(ctx, &e, "upload params failed"));
    }
    if let Err(e) = ctx.upload_buffer(
        locals.in_buf.as_ref().expect("in"),
        0,
        &words_to_bytes(&input),
    ) {
        return Err(locals.fail(ctx, &e, "upload input failed"));
    }
    match ctx.submit() {
        Ok(h) => locals.submission = Some(h),
        Err(e) => return Err(locals.fail(ctx, &e, "upload submit failed")),
    }
    let h = locals.submission.take().expect("submission");
    if let Err(e) = ctx.wait(h, Some(Duration::from_secs(5))) {
        locals.submission = Some(h);
        return Err(locals.fail(ctx, &e, "upload wait failed"));
    }

    let pipe = match ctx.obtain_affine_u32_pipeline(mode) {
        Ok(p) => p,
        Err(e) => return Err(locals.fail(ctx, &e, "pipeline obtain failed")),
    };
    if let Err(e) = ctx.dispatch_affine_u32(
        &pipe,
        locals.params.as_ref().expect("params"),
        locals.in_buf.as_ref().expect("in"),
        locals.out_buf.as_ref().expect("out"),
        count,
    ) {
        return Err(locals.fail(ctx, &e, "dispatch failed"));
    }
    match ctx.submit() {
        Ok(h) => locals.submission = Some(h),
        Err(e) => return Err(locals.fail(ctx, &e, "dispatch submit failed")),
    }
    let h = locals.submission.take().expect("submission");
    if let Err(e) = ctx.wait(h, Some(Duration::from_secs(5))) {
        locals.submission = Some(h);
        return Err(locals.fail(ctx, &e, "dispatch wait failed"));
    }

    match ctx.readback_buffer(
        locals.out_buf.as_ref().expect("out"),
        0,
        u64::from(count) * 4,
    ) {
        Ok(t) => locals.ticket = Some(t),
        Err(e) => return Err(locals.fail(ctx, &e, "readback encode failed")),
    }
    match ctx.submit() {
        Ok(h) => locals.submission = Some(h),
        Err(e) => return Err(locals.fail(ctx, &e, "readback submit failed")),
    }
    let h = locals.submission.take().expect("submission");
    if let Err(e) = ctx.wait(h, Some(Duration::from_secs(5))) {
        locals.submission = Some(h);
        return Err(locals.fail(ctx, &e, "readback wait failed"));
    }
    let got = match ctx.map_readback(locals.ticket.as_ref().expect("ticket")) {
        Ok(bytes) => bytes_to_words(&bytes),
        Err(e) => return Err(locals.fail(ctx, &e, "map failed")),
    };
    let expect = cpu_affine_u32(&input, scale, bias, mode);
    let match_cpu = got == expect;

    // Success path: drop locals (map already consumed ticket staging).
    locals.ticket = None;
    locals.params = None;
    locals.in_buf = None;
    locals.out_buf = None;

    let after = ctx.resource_accounting().current_bytes;
    let retained = ctx.retained_submission_count();
    let diag_errs = ctx
        .filter_diagnostics(0, Some(DiagnosticKind::Error), None)
        .len();
    let status = ctx.status();

    if !match_cpu {
        return Err(locals.fail(
            ctx,
            &DomainError::try_new(ErrorCode::Internal, "affine smoke CPU mismatch").expect("msg"),
            "affine output did not match cpu_affine_u32",
        ));
    }
    if after != 0 || retained != 0 {
        return Err(locals.fail(
            ctx,
            &DomainError::try_new(ErrorCode::Internal, "smoke accounting leak after drop")
                .expect("msg"),
            format!("after_drop current={after} retained_submissions={retained}"),
        ));
    }
    if status != ContextStatus::Live {
        return Err(locals.fail(
            ctx,
            &DomainError::try_new(ErrorCode::DeviceLost, "smoke context not live").expect("msg"),
            format!("context_status={status:?}"),
        ));
    }

    let mut report = SmokeReport::base(SmokeKind::Compute, SmokeStatus::Pass).with_device(&device);
    report.match_cpu = Some(true);
    report.accounted_source = Some(RESOURCE_ACCOUNT_SOURCE);
    report.after_drop_current_bytes = Some(after);
    report.retained_submissions = Some(retained);
    report.diagnostic_error_count = Some(diag_errs);
    report.context_status = Some(status);
    report.reason = Some("affine upload/dispatch/readback cpu_exact; resources reclaimed".into());
    Ok(report)
}

/// Intentional validation fail probe (resource-scope → `SP-INP-003` on host;
/// synthetic inject → typed scope error). Leaves context reclaimable.
pub fn probe_smoke_validation_fail(ctx: &mut GpuContext) -> SmokeReport {
    let err = if ctx.backend_is_host() {
        match ctx.host_probe_validation_scope() {
            Err(e) => e,
            Ok(()) => {
                return fail_report(
                    SmokeKind::Compute,
                    &DomainError::try_new(
                        ErrorCode::Internal,
                        "validation probe unexpectedly succeeded",
                    )
                    .expect("msg"),
                    "host validation probe returned Ok",
                    Some(ctx),
                );
            }
        }
    } else {
        ctx.inject_next_scope_error(crate::diagnostics::ScopeErrorInject::Validation);
        match ctx.create_buffer(BufferDesc {
            label: label("spirula:gpu:buffer:smoke-validation"),
            size: 16,
            usage: BufferUsage::COPY_SRC_DST,
        }) {
            Err(e) => e,
            Ok(buf) => {
                drop(buf);
                return fail_report(
                    SmokeKind::Compute,
                    &DomainError::try_new(
                        ErrorCode::Internal,
                        "synthetic validation inject unexpectedly succeeded",
                    )
                    .expect("msg"),
                    "synthetic validation create returned Ok",
                    Some(ctx),
                );
            }
        }
    };
    let mut report = fail_report(
        SmokeKind::Compute,
        &err,
        "validation fail probe observed typed error",
        Some(ctx),
    );
    // Expected fail probe: surface as Fail with the typed code (success of the probe).
    report.reason = Some(format!(
        "validation probe ok: code={} cmd={:?}",
        err.code().as_str(),
        err.context_value(spirula_core::ContextKey::Command)
    ));
    report
}

/// Intentional device-lost fail probe → `SP-GPU-002`.
pub fn probe_smoke_lost_fail(ctx: &mut GpuContext) -> SmokeReport {
    ctx.inject_lost("p2-f06 smoke lost probe");
    let err = match ctx.device_handle() {
        Err(e) => e,
        Ok(_) => {
            return fail_report(
                SmokeKind::Compute,
                &DomainError::try_new(ErrorCode::Internal, "lost probe still live").expect("msg"),
                "inject_lost did not reject device handle",
                Some(ctx),
            );
        }
    };
    let mut report = fail_report(
        SmokeKind::Compute,
        &err,
        "lost fail probe observed typed error",
        Some(ctx),
    );
    report.reason = Some(format!(
        "lost probe ok: code={} status={:?}",
        err.code().as_str(),
        ctx.status()
    ));
    report
}

/// Intentional wait-timeout fail probe → typed wait error; submission retained.
pub fn probe_smoke_timeout_fail(ctx: &mut GpuContext) -> SmokeReport {
    let buf = match ctx.create_buffer(BufferDesc {
        label: label("spirula:gpu:buffer:smoke-timeout"),
        size: 16,
        usage: BufferUsage::COPY_SRC_DST,
    }) {
        Ok(b) => b,
        Err(e) => {
            return fail_report(
                SmokeKind::Compute,
                &e,
                "timeout probe create failed",
                Some(ctx),
            );
        }
    };
    if let Err(e) = ctx.upload_buffer(&buf, 0, &[0u8; 16]) {
        return fail_report(
            SmokeKind::Compute,
            &e,
            "timeout probe upload failed",
            Some(ctx),
        );
    }
    let handle = match ctx.submit() {
        Ok(h) => h,
        Err(e) => {
            return fail_report(
                SmokeKind::Compute,
                &e,
                "timeout probe submit failed",
                Some(ctx),
            );
        }
    };
    ctx.inject_next_wait_timeout();
    let err = match ctx.wait(handle, Some(Duration::from_millis(1))) {
        Err(e) => e,
        Ok(()) => {
            drop(buf);
            return fail_report(
                SmokeKind::Compute,
                &DomainError::try_new(ErrorCode::Internal, "timeout probe wait Ok").expect("msg"),
                "inject_next_wait_timeout did not fail wait",
                Some(ctx),
            );
        }
    };
    let retained = ctx.retained_submission_count();
    // Finish the submission so resources can reclaim.
    let _ = ctx.wait(handle, Some(Duration::from_secs(5)));
    drop(buf);
    let mut report = fail_report(
        SmokeKind::Compute,
        &err,
        "timeout fail probe observed typed error",
        Some(ctx),
    );
    report.reason = Some(format!(
        "timeout probe ok: code={} retained_before_retry={retained}",
        err.code().as_str()
    ));
    report.retained_submissions = Some(ctx.retained_submission_count());
    report.after_drop_current_bytes = Some(ctx.resource_accounting().current_bytes);
    report
}

/// Env key that opts into attempting a real surface path.
pub const SURFACE_SMOKE_ENV: &str = "SPIRULA_GPU_SURFACE";

fn surface_env_enabled() -> bool {
    env::var_os(SURFACE_SMOKE_ENV).is_some_and(|v| v == "1" || v == "true")
}

/// Minimal surface smoke. Does **not** forge a window.
///
/// Without [`SURFACE_SMOKE_ENV`]=1 → [`SmokeStatus::Unsupported`].
/// With the env set but no safe present target wired in this crate → still
/// [`SmokeStatus::Unsupported`] (honest; present path stays P6-F07 / later).
/// Adapter/device open failure → [`SmokeStatus::Skip`].
pub fn run_surface_smoke(request: &CapabilityRequest) -> SmokeReport {
    run_surface_smoke_gated(request, surface_env_enabled())
}

/// Testable surface gate: `opt_in` mirrors [`SURFACE_SMOKE_ENV`].
pub fn run_surface_smoke_gated(request: &CapabilityRequest, opt_in: bool) -> SmokeReport {
    if !opt_in {
        let mut report = SmokeReport::base(SmokeKind::Surface, SmokeStatus::Unsupported);
        report.reason = Some(format!(
            "surface smoke disabled; set {SURFACE_SMOKE_ENV}=1 only with a safe host present \
             target (P2-F06 will not forge a window)"
        ));
        return report;
    }

    // Opening a device proves compute-capable GPU, but this slice still has no
    // display-backed present target without forging a window.
    match GpuContext::open(request) {
        Err(failure) => skip_from_open(SmokeKind::Surface, failure),
        Ok(mut ctx) => {
            let mut report = SmokeReport::base(SmokeKind::Surface, SmokeStatus::Unsupported);
            if let Some(dev) = ctx.report() {
                report = report.with_device(dev);
            }
            report.reason = Some(
                "SPIRULA_GPU_SURFACE enabled but no safe present target is wired; \
                 refusing to forge a window (surface present remains out of this slice)"
                    .into(),
            );
            report.context_status = Some(ctx.status());
            ctx.close();
            report
        }
    }
}

impl GpuContext {
    /// True when the live backend is a real wgpu device (not synthetic).
    pub fn backend_is_host(&self) -> bool {
        matches!(self.backend, Some(crate::context::Backend::Wgpu { .. }))
    }
}

/// Backend kind helper for reports (re-export convenience).
pub fn backend_kind_str(backend: BackendKind) -> &'static str {
    backend.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{
        AdapterCapabilities, AdapterClass, AdapterIdentity, AdapterLimits, FeatureBits,
        FieldAvailability,
    };

    fn sample_adapter() -> AdapterCapabilities {
        AdapterCapabilities {
            identity: AdapterIdentity {
                index: 0,
                name: "synthetic-smoke".into(),
                backend: BackendKind::Metal,
                class: AdapterClass::DiscreteGpu,
                vendor: 1,
                device: 2,
                driver: "test".into(),
                driver_info: String::new(),
                dedicated_vram_bytes: FieldAvailability::Unavailable {
                    reason: "synthetic",
                },
                uuid: FieldAvailability::Unavailable {
                    reason: "synthetic",
                },
            },
            features: FeatureBits {
                subgroup: true,
                shader_f16: true,
                timestamp_query: false,
                float32_atomic_add: true,
            },
            limits: AdapterLimits {
                max_buffer_size: 1 << 30,
                max_storage_buffers_per_shader_stage: 16,
                max_compute_workgroup_storage_size: 32_768,
            },
        }
    }

    #[test]
    fn synthetic_compute_smoke_passes_and_reclaims() {
        let mut ctx = GpuContext::open_synthetic(
            &CapabilityRequest::diagnose_default(),
            sample_adapter(),
            false,
        )
        .expect("synthetic open");
        let report = run_compute_smoke_on_context(&mut ctx).expect("pass");
        assert_eq!(report.status, SmokeStatus::Pass);
        assert_eq!(report.match_cpu, Some(true));
        assert_eq!(report.after_drop_current_bytes, Some(0));
        assert_eq!(report.retained_submissions, Some(0));
        assert_eq!(smoke_exit_code(&report), EXIT_SUCCESS);
    }

    #[test]
    fn submit_inject_fail_matches_context_current_zero_then_retry() {
        let mut ctx = GpuContext::open_synthetic(
            &CapabilityRequest::diagnose_default(),
            sample_adapter(),
            false,
        )
        .expect("open");
        ctx.inject_next_submit_error();
        let fail = run_compute_smoke_on_context(&mut ctx).expect_err("fail");
        assert_eq!(fail.status, SmokeStatus::Fail);
        assert_eq!(fail.after_drop_current_bytes, Some(0));
        assert_eq!(ctx.resource_accounting().current_bytes, 0);
        assert_eq!(ctx.retained_submission_count(), 0);
        let pass = run_compute_smoke_on_context(&mut ctx).expect("retry");
        assert_eq!(pass.status, SmokeStatus::Pass);
    }

    #[test]
    fn synthetic_validation_lost_timeout_probes_are_typed_fails() {
        let mut ctx = GpuContext::open_synthetic(
            &CapabilityRequest::diagnose_default(),
            sample_adapter(),
            false,
        )
        .expect("open");
        let v = probe_smoke_validation_fail(&mut ctx);
        assert_eq!(v.status, SmokeStatus::Fail);
        assert_eq!(v.error_code.as_deref(), Some("SP-INP-003"));

        let t = probe_smoke_timeout_fail(&mut ctx);
        assert_eq!(t.status, SmokeStatus::Fail);
        assert_eq!(t.error_code.as_deref(), Some("SP-GPU-003"));
        assert_eq!(ctx.retained_submission_count(), 0);
        assert_eq!(ctx.resource_accounting().current_bytes, 0);

        let lost = probe_smoke_lost_fail(&mut ctx);
        assert_eq!(lost.status, SmokeStatus::Fail);
        assert_eq!(lost.error_code.as_deref(), Some("SP-GPU-002"));
        assert_eq!(ctx.status(), ContextStatus::Lost);
    }

    #[test]
    fn surface_smoke_unsupported_without_opt_in() {
        let report = run_surface_smoke_gated(&CapabilityRequest::diagnose_default(), false);
        assert_eq!(report.status, SmokeStatus::Unsupported);
        assert!(
            report
                .reason
                .as_deref()
                .is_some_and(|r| r.contains("will not forge a window"))
        );
        assert_eq!(smoke_exit_code(&report), EXIT_SUCCESS);
    }

    #[test]
    fn surface_smoke_opt_in_still_unsupported_without_present_target() {
        let report = run_surface_smoke_gated(&CapabilityRequest::diagnose_default(), true);
        // Host may Skip (no adapter) or Unsupported (device opens but no surface).
        assert!(
            matches!(report.status, SmokeStatus::Unsupported | SmokeStatus::Skip),
            "status={:?} reason={:?}",
            report.status,
            report.reason
        );
        assert_eq!(smoke_exit_code(&report), EXIT_SUCCESS);
    }
}
