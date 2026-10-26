//! Host headless device open → observe → close (P2-F02).
//!
//! Always runs. Empty/no-adapter hosts must fail with precise SP-GPU-001, not
//! a false pass. Device create ≠ compute readiness.

use spirula_gpu::{
    CapabilityRequest, ContextStatus, GpuContext, OpenFailureStage, WGPU_VERSION,
    diagnose_device_open,
};

#[test]
fn host_open_observe_close_or_precise_unavailable() {
    let request = CapabilityRequest::diagnose_default();
    match GpuContext::open(&request) {
        Ok(mut ctx) => {
            assert_eq!(ctx.status(), ContextStatus::Live);
            let report = ctx.report().expect("live report").clone();
            eprintln!("wgpu_version={}", report.wgpu_version);
            assert_eq!(report.wgpu_version, WGPU_VERSION);
            eprintln!(
                "host_device context_id={} generation={} backend={} name={:?} queue={}",
                report.context_id.get(),
                report.generation.get(),
                report.backend.as_str(),
                report.adapter.identity.name,
                report.queue_present
            );
            eprintln!(
                "host_requested_features subgroup={} f16={} timestamp={} f32_atomic={}",
                report.requested_features.subgroup,
                report.requested_features.shader_f16,
                report.requested_features.timestamp_query,
                report.requested_features.float32_atomic_add
            );
            eprintln!(
                "host_enabled_features subgroup={} f16={} timestamp={} f32_atomic={}",
                report.enabled_features.subgroup,
                report.enabled_features.shader_f16,
                report.enabled_features.timestamp_query,
                report.enabled_features.float32_atomic_add
            );
            eprintln!(
                "host_requested_limits max_buffer_size={} storage_buffers={} workgroup_storage={}",
                report.requested_limits.max_buffer_size.effective,
                report
                    .requested_limits
                    .max_storage_buffers_per_shader_stage
                    .effective,
                report
                    .requested_limits
                    .max_compute_workgroup_storage_size
                    .effective
            );
            eprintln!(
                "host_device_limits max_buffer_size={} storage_buffers={} workgroup_storage={}",
                report.device_limits.max_buffer_size,
                report.device_limits.max_storage_buffers_per_shader_stage,
                report.device_limits.max_compute_workgroup_storage_size
            );
            assert!(report.queue_present);
            assert!(!report.adapter.identity.name.trim().is_empty());

            let handle = ctx.device_handle().expect("live handle");
            ctx.validate_handle(handle)
                .expect("handle valid while live");

            // Scene reset must not close the device.
            ctx.reset_scene();
            assert_eq!(ctx.status(), ContextStatus::Live);
            ctx.validate_handle(handle)
                .expect("handle after reset_scene");

            ctx.close();
            assert_eq!(ctx.status(), ContextStatus::Closed);
            assert!(ctx.device_handle().is_err());
            assert!(ctx.validate_handle(handle).is_err());
            ctx.close(); // idempotent
            assert_eq!(ctx.status(), ContextStatus::Closed);

            // Owner-driven reopen on a fresh context (diagnose path).
            match diagnose_device_open(&request) {
                Ok(second) => {
                    eprintln!(
                        "host_diagnose_reopen_ok context_id={} generation={}",
                        second.context_id.get(),
                        second.generation.get()
                    );
                    // Distinct open → distinct context id; old handle stays dead on closed ctx.
                    assert_ne!(second.context_id, report.context_id);
                }
                Err(failure) => {
                    panic!(
                        "second diagnose open failed after first success: {:?} {}",
                        failure.stage,
                        failure.error.message()
                    );
                }
            }
        }
        Err(failure) => {
            eprintln!(
                "host_device_unavailable stage={:?} code={} message={}",
                failure.stage,
                failure.error.code().as_str(),
                failure.error.message()
            );
            assert!(
                matches!(
                    failure.stage,
                    OpenFailureStage::EnumerateEmpty
                        | OpenFailureStage::Select
                        | OpenFailureStage::DeviceCreate
                ),
                "unexpected stage {:?}",
                failure.stage
            );
            match failure.stage {
                OpenFailureStage::EnumerateEmpty | OpenFailureStage::Select => {
                    assert_eq!(failure.error.code().as_str(), "SP-GPU-001");
                }
                OpenFailureStage::DeviceCreate => {
                    assert_eq!(failure.error.code().as_str(), "SP-GPU-004");
                }
                OpenFailureStage::LiveMatch => {
                    assert_eq!(failure.error.code().as_str(), "SP-GPU-001");
                }
            }
        }
    }
}

#[test]
fn host_real_lost_path_is_distinct_from_synthetic_inject_api() {
    // Documentation/guard test: LostSource variants exist and host open does not
    // fabricate SyntheticInject notices. Real host lost callbacks are not forced
    // here (would require driver fault); synthetic coverage lives in
    // context_lifecycle_synthetic.rs.
    let request = CapabilityRequest::diagnose_default();
    if let Ok(mut ctx) = GpuContext::open(&request) {
        assert!(
            ctx.lost_notices().is_empty(),
            "fresh host open must not invent lost notices"
        );
        // Synthetic inject is available for tests but is explicitly not HostCallback.
        ctx.inject_lost("test-only inject on host context");
        let notices = ctx.take_lost_notices();
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].source, spirula_gpu::LostSource::SyntheticInject);
        assert_ne!(notices[0].source, spirula_gpu::LostSource::HostCallback);
        ctx.close();
    }
}
