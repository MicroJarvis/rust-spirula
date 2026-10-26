//! Synthetic P2-F02 lifecycle: create failure, lost/close, handles, owner reopen.
//! No host GPU required.

use spirula_core::ErrorCode;
use spirula_gpu::{
    AdapterCapabilities, AdapterClass, AdapterIdentity, AdapterLimits, BackendKind, CapabilityId,
    CapabilityRequest, ContextStatus, FeatureBits, FieldAvailability, GpuContext, LostSource,
    OpenFailureStage,
};

fn sample_adapter() -> AdapterCapabilities {
    AdapterCapabilities {
        identity: AdapterIdentity {
            index: 0,
            name: "synthetic-discrete".into(),
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
fn synthetic_create_failure_is_sp_gpu_004() {
    let req = CapabilityRequest::diagnose_default();
    let Err(err) = GpuContext::open_synthetic(&req, sample_adapter(), true) else {
        panic!("expected device create failure");
    };
    assert_eq!(err.stage, OpenFailureStage::DeviceCreate);
    assert_eq!(err.error.code(), ErrorCode::AllocationFailed);
    assert_eq!(err.error.code().as_str(), "SP-GPU-004");
    assert!(
        err.negotiated.is_some(),
        "negotiation must run before create"
    );
}

#[test]
fn synthetic_open_close_idempotent_and_reset_scene_keeps_device() {
    let req = CapabilityRequest::diagnose_default();
    let mut ctx = GpuContext::open_synthetic(&req, sample_adapter(), false).unwrap();
    assert_eq!(ctx.status(), ContextStatus::Live);
    let handle = ctx.device_handle().unwrap();
    ctx.validate_handle(handle).unwrap();

    ctx.reset_scene();
    assert_eq!(ctx.status(), ContextStatus::Live);
    ctx.validate_handle(handle).unwrap();

    ctx.close();
    assert_eq!(ctx.status(), ContextStatus::Closed);
    assert_eq!(
        ctx.device_handle().unwrap_err().code(),
        ErrorCode::DeviceLost
    );
    assert_eq!(
        ctx.validate_handle(handle).unwrap_err().code(),
        ErrorCode::DeviceLost
    );

    ctx.close(); // idempotent
    assert_eq!(ctx.status(), ContextStatus::Closed);
}

#[test]
fn synthetic_inject_lost_records_notice_and_rejects_handle() {
    let req = CapabilityRequest::diagnose_default();
    let mut ctx = GpuContext::open_synthetic(&req, sample_adapter(), false).unwrap();
    let handle = ctx.device_handle().unwrap();
    ctx.inject_lost("synthetic lost for test");
    assert_eq!(ctx.status(), ContextStatus::Lost);
    let notices = ctx.take_lost_notices();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].source, LostSource::SyntheticInject);
    assert_eq!(notices[0].reason, "synthetic lost for test");
    assert_eq!(
        ctx.validate_handle(handle).unwrap_err().code(),
        ErrorCode::DeviceLost
    );
    assert_eq!(
        ctx.validate_handle(handle).unwrap_err().code().as_str(),
        "SP-GPU-002"
    );
}

#[test]
fn synthetic_cross_context_handle_rejected_sp_inp_001() {
    let req = CapabilityRequest::diagnose_default();
    let mut a = GpuContext::open_synthetic(&req, sample_adapter(), false).unwrap();
    let mut b = GpuContext::open_synthetic(&req, sample_adapter(), false).unwrap();
    let handle_a = a.device_handle().unwrap();
    let err = b.validate_handle(handle_a).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidArgument);
    assert_eq!(err.code().as_str(), "SP-INP-001");
}

#[test]
fn synthetic_owner_reopen_bumps_generation_old_handle_dead() {
    let req = CapabilityRequest::diagnose_default();
    let mut ctx = GpuContext::open_synthetic(&req, sample_adapter(), false).unwrap();
    let id = ctx.id();
    let old = ctx.device_handle().unwrap();
    assert_eq!(old.generation().get(), 1);

    ctx.reopen_synthetic(&req, sample_adapter(), false).unwrap();
    assert_eq!(ctx.id(), id);
    assert_eq!(ctx.generation().get(), 2);
    assert_eq!(ctx.status(), ContextStatus::Live);

    assert_eq!(
        ctx.validate_handle(old).unwrap_err().code(),
        ErrorCode::DeviceLost
    );
    let fresh = ctx.device_handle().unwrap();
    assert_eq!(fresh.generation().get(), 2);
    ctx.validate_handle(fresh).unwrap();
}

#[test]
fn synthetic_reopen_device_create_failure_syncs_report() {
    let req = CapabilityRequest::diagnose_default();
    let mut ctx = GpuContext::open_synthetic(&req, sample_adapter(), false).unwrap();
    let old = ctx.device_handle().unwrap();
    let err = ctx
        .reopen_synthetic(&req, sample_adapter(), true)
        .unwrap_err();
    assert_eq!(err.stage, OpenFailureStage::DeviceCreate);
    assert_eq!(ctx.status(), ContextStatus::CreateFailed);
    let report = ctx.report().expect("report retained after failed reopen");
    assert_eq!(report.status, ContextStatus::CreateFailed);
    assert!(!report.queue_present);
    assert_eq!(
        ctx.validate_handle(old).unwrap_err().code(),
        ErrorCode::DeviceLost
    );
    assert!(ctx.device_handle().is_err());
}

#[test]
fn synthetic_reopen_select_failure_syncs_report_unlike_stale_live() {
    // Reproduce review defect: Select Err must not leave report.status=Live
    // with queue_present=true while context is CreateFailed.
    let open_req = CapabilityRequest::diagnose_default();
    let mut weak = sample_adapter();
    weak.features.subgroup = false;
    let mut ctx = GpuContext::open_synthetic(&open_req, weak.clone(), false).unwrap();
    let old = ctx.device_handle().unwrap();
    assert_eq!(ctx.report().unwrap().status, ContextStatus::Live);

    let mut need_subgroup = CapabilityRequest::diagnose_default();
    need_subgroup.required_features = vec![CapabilityId::Subgroup];
    need_subgroup.optional_features.clear();

    let err = ctx
        .reopen_synthetic(&need_subgroup, weak, false)
        .expect_err("required Subgroup missing on adapter");
    assert_eq!(err.stage, OpenFailureStage::Select);
    assert_eq!(err.error.code(), ErrorCode::NoAdapter);
    assert_eq!(ctx.status(), ContextStatus::CreateFailed);

    let report = ctx.report().expect("public report kept for observation");
    assert_eq!(
        report.status,
        ContextStatus::CreateFailed,
        "report.status must match context after Select reopen failure"
    );
    assert!(
        !report.queue_present,
        "no backend survived failed reopen; queue must be false"
    );
    assert_eq!(
        ctx.validate_handle(old).unwrap_err().code(),
        ErrorCode::DeviceLost
    );
    assert!(ctx.device_handle().is_err());
}

#[test]
fn synthetic_negotiated_features_appear_on_report() {
    let req = CapabilityRequest::diagnose_default();
    let ctx = GpuContext::open_synthetic(&req, sample_adapter(), false).unwrap();
    let report = ctx.report().unwrap();
    assert!(report.requested_features.subgroup);
    assert!(report.requested_features.shader_f16);
    assert!(!report.requested_features.timestamp_query);
    assert!(report.requested_features.float32_atomic_add);
    assert_eq!(report.requested_limits.max_buffer_size.effective, 1 << 30);
    assert!(report.queue_present);
}
