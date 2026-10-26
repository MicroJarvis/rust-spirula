//! P2-F05 synthetic probes: inject submit/scope/lost + diagnostic ring bounds.

use spirula_core::{ContextKey, ErrorCode};
use spirula_gpu::{
    AdapterCapabilities, AdapterClass, AdapterIdentity, AdapterLimits, BackendKind, BufferDesc,
    BufferUsage, CapabilityRequest, ContextStatus, DiagnosticKind, FeatureBits, FieldAvailability,
    GpuContext, GpuOperation, LostSource, ResourceLabel, ScopeErrorInject,
};

fn sample_adapter() -> AdapterCapabilities {
    AdapterCapabilities {
        identity: AdapterIdentity {
            index: 0,
            name: "synthetic-diag".into(),
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
fn synthetic_inject_submit_emits_sp_gpu_003_with_operation() {
    let mut ctx = open_ctx();
    ctx.inject_next_submit_error();
    let err = ctx.submit().unwrap_err();
    assert_eq!(err.code(), ErrorCode::SubmitFailed);
    assert_eq!(err.code().as_str(), "SP-GPU-003");
    assert_eq!(err.context_value(ContextKey::Command), Some("submit"));
    let events = ctx.poll_diagnostics(0);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, DiagnosticKind::Error);
    assert_eq!(events[0].operation, GpuOperation::Submit);
    assert_eq!(events[0].code.as_deref(), Some("SP-GPU-003"));
    assert_eq!(events[0].generation, ctx.generation());
}

#[test]
fn synthetic_inject_scope_oom_on_create_carries_budget_fields() {
    let mut ctx = open_ctx();
    ctx.set_resource_budget(Some(1 << 20));
    ctx.inject_next_scope_error(ScopeErrorInject::OutOfMemory {
        requested_bytes: Some(64),
        current_bytes: Some(0),
        budget_bytes: Some(1 << 20),
    });
    let label = ResourceLabel::parse("spirula:gpu:buffer:oom-probe").unwrap();
    let Err(err) = ctx.create_buffer(BufferDesc {
        label: label.clone(),
        size: 64,
        usage: BufferUsage::COPY_SRC_DST,
    }) else {
        panic!("expected injected OOM scope failure");
    };
    assert_eq!(err.code().as_str(), "SP-GPU-004");
    assert_eq!(err.context_value(ContextKey::Command), Some("create"));
    assert_eq!(
        err.context_value(ContextKey::Resource),
        Some("spirula:gpu:buffer:oom-probe")
    );
    let filtered = ctx.filter_diagnostics(0, Some(DiagnosticKind::Error), Some(label.as_str()));
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].requested_bytes, Some(64));
    assert_eq!(filtered[0].budget_bytes, Some(1 << 20));
}

#[test]
fn synthetic_inject_lost_one_notice_rejects_handle_and_emits_event() {
    let mut ctx = open_ctx();
    let handle = ctx.device_handle().unwrap();
    let buf = ctx
        .create_buffer(BufferDesc {
            label: ResourceLabel::parse("spirula:gpu:buffer:pre-lost").unwrap(),
            size: 16,
            usage: BufferUsage::COPY_DST,
        })
        .unwrap();
    ctx.inject_lost("synthetic lost diag");
    assert_eq!(ctx.status(), ContextStatus::Lost);
    let notices = ctx.take_lost_notices();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].source, LostSource::SyntheticInject);
    assert_eq!(
        ctx.validate_handle(handle).unwrap_err().code().as_str(),
        "SP-GPU-002"
    );
    let lost_events = ctx.filter_diagnostics(0, Some(DiagnosticKind::DeviceLost), None);
    assert_eq!(lost_events.len(), 1);
    assert_eq!(lost_events[0].code.as_deref(), Some("SP-GPU-002"));
    // Old resource handle cannot be used after loss.
    let err = ctx.upload_buffer(&buf, 0, &[0u8; 16]).unwrap_err();
    assert_eq!(err.code(), ErrorCode::DeviceLost);
}

#[test]
fn synthetic_diagnostic_ring_drops_oldest_and_isolates_generations() {
    let mut ctx = open_ctx();
    let gen1 = ctx.generation();
    ctx.set_diagnostic_capacity(2);
    ctx.inject_next_submit_error();
    let _ = ctx.submit().unwrap_err();
    ctx.inject_next_submit_error();
    let _ = ctx.submit().unwrap_err();
    ctx.inject_next_submit_error();
    let _ = ctx.submit().unwrap_err();
    let stats = ctx.diagnostic_stats();
    assert_eq!(stats.capacity, 2);
    assert_eq!(stats.len, 2);
    assert_eq!(stats.dropped_oldest, 1);
    let retained = ctx.poll_diagnostics(0);
    assert_eq!(retained.len(), 2);
    assert!(retained[0].seq < retained[1].seq);
    assert_eq!(retained[0].generation, gen1);

    ctx.reopen_synthetic(
        &CapabilityRequest::diagnose_default(),
        sample_adapter(),
        false,
    )
    .unwrap();
    assert_eq!(ctx.generation().get(), gen1.get() + 1);
    assert!(ctx.poll_diagnostics(0).is_empty());
    assert_eq!(ctx.diagnostic_stats().dropped_oldest, 0);
    assert_eq!(ctx.diagnostic_stats().next_seq, 1);

    ctx.inject_next_submit_error();
    let _ = ctx.submit().unwrap_err();
    let after = ctx.poll_diagnostics(0);
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].generation, ctx.generation());
    assert_ne!(after[0].generation, gen1);
}

#[test]
fn synthetic_budget_preflight_emits_labeled_allocation_event() {
    let mut ctx = open_ctx();
    ctx.set_resource_budget(Some(8));
    let label = ResourceLabel::parse("spirula:gpu:buffer:big").unwrap();
    let Err(err) = ctx.create_buffer(BufferDesc {
        label: label.clone(),
        size: 64,
        usage: BufferUsage::COPY_DST,
    }) else {
        panic!("expected budget preflight failure");
    };
    assert_eq!(err.code(), ErrorCode::AllocationFailed);
    // P1-F08 preflight keeps Resource=accounted_work_buffer; the spirula label
    // is attached on the diagnostic event and via Command=create.
    assert_eq!(err.context_value(ContextKey::Command), Some("create"));
    let events = ctx.filter_diagnostics(0, None, Some(label.as_str()));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].code.as_deref(), Some("SP-GPU-004"));
    assert_eq!(events[0].operation, GpuOperation::Create);
    assert_eq!(events[0].label.as_deref(), Some("spirula:gpu:buffer:big"));
}

#[test]
fn synthetic_close_clears_diagnostics() {
    let mut ctx = open_ctx();
    ctx.inject_next_submit_error();
    let _ = ctx.submit().unwrap_err();
    assert!(!ctx.poll_diagnostics(0).is_empty());
    ctx.close();
    assert!(ctx.poll_diagnostics(0).is_empty());
}

#[test]
fn synthetic_upload_budget_carries_label_operation_and_event() {
    let mut ctx = open_ctx();
    let label = ResourceLabel::parse("spirula:gpu:buffer:upload-budget").unwrap();
    let buf = ctx
        .create_buffer(BufferDesc {
            label: label.clone(),
            size: 64,
            usage: BufferUsage::COPY_SRC_DST,
        })
        .unwrap();
    ctx.set_resource_budget(Some(64));
    let Err(err) = ctx.upload_buffer(&buf, 0, &[3u8; 64]) else {
        panic!("expected upload budget failure");
    };
    assert_eq!(err.code().as_str(), "SP-GPU-004");
    assert_eq!(err.context_value(ContextKey::Command), Some("upload"));
    assert_eq!(
        err.context_value(ContextKey::Resource),
        Some("spirula:gpu:buffer:upload-budget")
    );
    let events = ctx.filter_diagnostics(0, None, Some(label.as_str()));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].operation, GpuOperation::Upload);
    assert_eq!(events[0].code.as_deref(), Some("SP-GPU-004"));
    assert_eq!(ctx.resource_accounting().current_bytes, 64);
    drop(buf);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);
}

#[test]
fn synthetic_readback_budget_carries_label_operation_and_event() {
    let mut ctx = open_ctx();
    let label = ResourceLabel::parse("spirula:gpu:buffer:readback-budget").unwrap();
    let buf = ctx
        .create_buffer(BufferDesc {
            label: label.clone(),
            size: 64,
            usage: BufferUsage::COPY_SRC_DST,
        })
        .unwrap();
    ctx.set_resource_budget(Some(64));
    let Err(err) = ctx.readback_buffer(&buf, 0, 64) else {
        panic!("expected readback budget failure");
    };
    assert_eq!(err.code().as_str(), "SP-GPU-004");
    assert_eq!(err.context_value(ContextKey::Command), Some("readback"));
    assert_eq!(
        err.context_value(ContextKey::Resource),
        Some("spirula:gpu:buffer:readback-budget")
    );
    let events = ctx.filter_diagnostics(0, None, Some(label.as_str()));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].operation, GpuOperation::Readback);
    drop(buf);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);
}

#[test]
fn synthetic_post_submit_scope_returns_waitable_handle() {
    let mut ctx = open_ctx();
    let buf = ctx
        .create_buffer(BufferDesc {
            label: ResourceLabel::parse("spirula:gpu:buffer:post-submit").unwrap(),
            size: 64,
            usage: BufferUsage::COPY_SRC_DST,
        })
        .unwrap();
    ctx.upload_buffer(&buf, 0, &[3u8; 64]).unwrap();
    ctx.inject_next_post_submit_scope_error(ScopeErrorInject::Validation);
    let handle = ctx
        .submit()
        .expect("post-submit scope must still return handle");
    assert_eq!(ctx.retained_submission_count(), 1);
    let events = ctx.poll_diagnostics(0);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].code.as_deref(), Some("SP-GPU-003"));
    assert_eq!(events[0].operation, GpuOperation::Submit);
    drop(buf);
    ctx.wait(handle, None).unwrap();
    assert_eq!(ctx.retained_submission_count(), 0);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);
    assert_eq!(ctx.status(), ContextStatus::Live);
}

#[test]
fn synthetic_dispatch_oom_keeps_sp_gpu_004() {
    use spirula_gpu::{AFFINE_U32_PARAMS_BYTES, BufferUsage as BU};
    let mut ctx = open_ctx();
    let pipe = ctx.obtain_affine_u32_pipeline(0).unwrap();
    let params = ctx
        .create_buffer(BufferDesc {
            label: ResourceLabel::parse("spirula:gpu:buffer:params").unwrap(),
            size: AFFINE_U32_PARAMS_BYTES,
            usage: BU::UNIFORM_UPLOAD,
        })
        .unwrap();
    let input = ctx
        .create_buffer(BufferDesc {
            label: ResourceLabel::parse("spirula:gpu:buffer:input").unwrap(),
            size: 4,
            usage: BU::STORAGE_COPY,
        })
        .unwrap();
    let output = ctx
        .create_buffer(BufferDesc {
            label: ResourceLabel::parse("spirula:gpu:buffer:output").unwrap(),
            size: 4,
            usage: BU::STORAGE_COPY,
        })
        .unwrap();
    ctx.inject_next_scope_error(ScopeErrorInject::OutOfMemory {
        requested_bytes: Some(64),
        current_bytes: Some(24),
        budget_bytes: Some(1 << 20),
    });
    let err = ctx
        .dispatch_affine_u32(&pipe, &params, &input, &output, 1)
        .unwrap_err();
    assert_eq!(err.code().as_str(), "SP-GPU-004");
    assert_eq!(err.context_value(ContextKey::Command), Some("dispatch"));
    let events = ctx.poll_diagnostics(0);
    assert_eq!(events[0].code.as_deref(), Some("SP-GPU-004"));
    assert_eq!(events[0].operation, GpuOperation::Dispatch);
}
