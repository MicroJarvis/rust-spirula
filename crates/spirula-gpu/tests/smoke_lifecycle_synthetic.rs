//! P2-F06 synthetic smoke: pass + validation/lost/timeout fail probes.

use spirula_core::EXIT_SUCCESS;
use spirula_gpu::{
    AdapterCapabilities, AdapterClass, AdapterIdentity, AdapterLimits, BackendKind,
    CapabilityRequest, ContextStatus, FeatureBits, FieldAvailability, GpuContext, SmokeStatus,
    format_smoke_report, probe_smoke_lost_fail, probe_smoke_timeout_fail,
    probe_smoke_validation_fail, run_compute_smoke_on_context, run_surface_smoke_gated,
    smoke_exit_code,
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

fn open_ctx() -> GpuContext {
    GpuContext::open_synthetic(
        &CapabilityRequest::diagnose_default(),
        sample_adapter(),
        false,
    )
    .expect("synthetic open")
}

#[test]
fn synthetic_compute_smoke_pass_reclaims() {
    let mut ctx = open_ctx();
    let report = run_compute_smoke_on_context(&mut ctx).expect("pass");
    assert_eq!(report.status, SmokeStatus::Pass);
    assert_eq!(report.match_cpu, Some(true));
    assert_eq!(report.after_drop_current_bytes, Some(0));
    assert_eq!(report.retained_submissions, Some(0));
    assert_eq!(report.backend.as_deref(), Some("metal"));
    assert!(format_smoke_report(&report).contains("status=pass"));
    assert_eq!(smoke_exit_code(&report), EXIT_SUCCESS);
}

#[test]
fn synthetic_compute_smoke_repeat_stable() {
    let mut ctx = open_ctx();
    let a = run_compute_smoke_on_context(&mut ctx).expect("pass1");
    let b = run_compute_smoke_on_context(&mut ctx).expect("pass2");
    assert_eq!(a.status, SmokeStatus::Pass);
    assert_eq!(b.status, SmokeStatus::Pass);
    assert_eq!(a.after_drop_current_bytes, Some(0));
    assert_eq!(b.after_drop_current_bytes, Some(0));
}

#[test]
fn synthetic_smoke_fail_probes_validation_timeout_lost() {
    let mut ctx = open_ctx();
    let v = probe_smoke_validation_fail(&mut ctx);
    assert_eq!(v.status, SmokeStatus::Fail);
    assert_eq!(v.error_code.as_deref(), Some("SP-INP-003"));
    // Resource-scope validation is Warning (SP-INP-003), not Error kind.
    assert!(
        !ctx.poll_diagnostics(0).is_empty(),
        "validation probe must emit a diagnostic"
    );

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
fn synthetic_surface_smoke_unsupported_no_forged_window() {
    let off = run_surface_smoke_gated(&CapabilityRequest::diagnose_default(), false);
    assert_eq!(off.status, SmokeStatus::Unsupported);
    assert!(
        off.reason
            .as_deref()
            .is_some_and(|r| r.contains("will not forge a window"))
    );
}
