//! P2-F05 host probes: validation scope + destroy/lost → typed error + events.
//!
//! Real hardware OOM is **unsupported** here — do not claim host OOM reproduction.
//! Synthetic `ScopeErrorInject::OutOfMemory` covers the typed SP-GPU-004 path.

use spirula_core::{ContextKey, ErrorCode};
use spirula_gpu::{
    BufferDesc, BufferUsage, CapabilityRequest, ContextStatus, DiagnosticKind, GpuContext,
    LostSource, ResourceLabel, ScopeErrorInject,
};

#[test]
fn host_validation_scope_typed_error_and_diagnostic_or_unavailable() {
    let request = CapabilityRequest::diagnose_default();
    match GpuContext::open(&request) {
        Ok(mut ctx) => {
            let err = ctx.host_probe_validation_scope().unwrap_err();
            eprintln!(
                "host_validation_scope code={} cmd={:?} msg={}",
                err.code().as_str(),
                err.context_value(ContextKey::Command),
                err.message()
            );
            // Resource-class scoped validation → SP-INP-003 (ValueOutOfRange).
            assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
            assert_eq!(err.context_value(ContextKey::Command), Some("create"));
            let events = ctx.poll_diagnostics(0);
            assert!(
                !events.is_empty(),
                "validation failure must emit a diagnostic"
            );
            assert_eq!(events[0].kind, DiagnosticKind::Warning);
            assert_eq!(events[0].code.as_deref(), Some("SP-INP-003"));
            ctx.close();
        }
        Err(failure) => {
            eprintln!(
                "host_validation_unavailable stage={:?} {}",
                failure.stage,
                failure.error.message()
            );
        }
    }
}

#[test]
fn host_destroy_lost_emits_device_lost_diagnostic_or_unavailable() {
    let request = CapabilityRequest::diagnose_default();
    match GpuContext::open(&request) {
        Ok(mut ctx) => {
            let buf = ctx
                .create_buffer(BufferDesc {
                    label: ResourceLabel::parse("spirula:gpu:buffer:diag-loss").unwrap(),
                    size: 64,
                    usage: BufferUsage::COPY_SRC_DST,
                })
                .unwrap();
            ctx.upload_buffer(&buf, 0, &[1u8; 64]).unwrap();
            let sub = ctx.submit().unwrap();
            ctx.destroy_host_device_without_poll();
            let result = ctx.wait(sub, None);
            let status = ctx.status();
            let notices = ctx.take_lost_notices();
            let lost_events = ctx.filter_diagnostics(0, Some(DiagnosticKind::DeviceLost), None);
            eprintln!(
                "host_destroy_lost result={:?} status={status:?} notices={} lost_events={}",
                result.as_ref().map_err(|e| e.code().as_str()),
                notices.len(),
                lost_events.len()
            );
            assert!(result.is_err());
            assert_eq!(result.unwrap_err().code(), ErrorCode::DeviceLost);
            assert_eq!(status, ContextStatus::Lost);
            assert_eq!(notices.len(), 1);
            assert_eq!(notices[0].source, LostSource::HostCallback);
            assert_eq!(
                lost_events.len(),
                1,
                "exactly one device-lost diagnostic for one callback delivery"
            );
            assert_eq!(lost_events[0].code.as_deref(), Some("SP-GPU-002"));
            drop(buf);
        }
        Err(failure) => {
            eprintln!(
                "host_destroy_lost_unavailable stage={:?} {}",
                failure.stage,
                failure.error.message()
            );
        }
    }
}

#[test]
fn host_inject_submit_on_live_device_or_unavailable() {
    let request = CapabilityRequest::diagnose_default();
    match GpuContext::open(&request) {
        Ok(mut ctx) => {
            ctx.inject_next_submit_error();
            let err = ctx.submit().unwrap_err();
            assert_eq!(err.code().as_str(), "SP-GPU-003");
            let events = ctx.poll_diagnostics(0);
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].code.as_deref(), Some("SP-GPU-003"));
            ctx.close();
        }
        Err(failure) => {
            eprintln!(
                "host_inject_submit_unavailable stage={:?} {}",
                failure.stage,
                failure.error.message()
            );
        }
    }
}

#[test]
fn host_upload_budget_metadata_and_event_or_unavailable() {
    let request = CapabilityRequest::diagnose_default();
    match GpuContext::open(&request) {
        Ok(mut ctx) => {
            let label = ResourceLabel::parse("spirula:gpu:buffer:probe").unwrap();
            let buf = ctx
                .create_buffer(BufferDesc {
                    label: label.clone(),
                    size: 64,
                    usage: BufferUsage::COPY_SRC_DST,
                })
                .unwrap();
            ctx.set_resource_budget(Some(64));
            let Err(err) = ctx.upload_buffer(&buf, 0, &[3u8; 64]) else {
                panic!("expected upload budget SP-GPU-004");
            };
            eprintln!(
                "host_upload_budget code={} command={:?} resource={:?} events={}",
                err.code().as_str(),
                err.context_value(ContextKey::Command),
                err.context_value(ContextKey::Resource),
                ctx.poll_diagnostics(0).len()
            );
            assert_eq!(err.code().as_str(), "SP-GPU-004");
            assert_eq!(err.context_value(ContextKey::Command), Some("upload"));
            assert_eq!(
                err.context_value(ContextKey::Resource),
                Some("spirula:gpu:buffer:probe")
            );
            assert!(!ctx.poll_diagnostics(0).is_empty());
            assert_eq!(ctx.resource_accounting().current_bytes, 64);
            drop(buf);
            assert_eq!(ctx.resource_accounting().current_bytes, 0);
            ctx.close();
        }
        Err(failure) => {
            eprintln!(
                "host_upload_budget_unavailable stage={:?} {}",
                failure.stage,
                failure.error.message()
            );
        }
    }
}

#[test]
fn host_post_submit_scope_waitable_reclaim_or_unavailable() {
    use std::time::Duration;
    let request = CapabilityRequest::diagnose_default();
    match GpuContext::open(&request) {
        Ok(mut ctx) => {
            let buf = ctx
                .create_buffer(BufferDesc {
                    label: ResourceLabel::parse("spirula:gpu:buffer:probe").unwrap(),
                    size: 64,
                    usage: BufferUsage::COPY_SRC_DST,
                })
                .unwrap();
            ctx.upload_buffer(&buf, 0, &[3u8; 64]).unwrap();
            ctx.inject_next_post_submit_scope_error(ScopeErrorInject::Validation);
            let handle = ctx
                .submit()
                .expect("post-submit scope must return waitable handle");
            assert_eq!(ctx.retained_submission_count(), 1);
            assert_eq!(
                ctx.poll_diagnostics(0)[0].code.as_deref(),
                Some("SP-GPU-003")
            );
            // Capacity-1 ring + unrelated submit error must not erase latched failure.
            ctx.set_diagnostic_capacity(1);
            ctx.inject_next_submit_error();
            let _ = ctx.submit().unwrap_err();
            assert!(ctx.diagnostic_stats().dropped_oldest >= 1);
            drop(buf);
            let empty = ctx.submit().unwrap();
            ctx.wait(empty, Some(Duration::from_secs(5))).unwrap();
            // Prior scoped submission still held until its handle is waited.
            assert_eq!(ctx.retained_submission_count(), 1);
            let err = ctx
                .wait(handle, Some(Duration::from_secs(5)))
                .expect_err("post-submit Validation must surface on wait");
            assert_eq!(err.code().as_str(), "SP-GPU-003");
            assert_eq!(err.context_value(ContextKey::Command), Some("submit"));
            eprintln!(
                "host_post_submit_scope afterwait records={} current={} status={:?} code={}",
                ctx.retained_submission_count(),
                ctx.resource_accounting().current_bytes,
                ctx.status(),
                err.code().as_str()
            );
            assert_eq!(ctx.retained_submission_count(), 0);
            assert_eq!(ctx.resource_accounting().current_bytes, 0);
            assert_eq!(ctx.status(), ContextStatus::Live);
            ctx.close();
        }
        Err(failure) => {
            eprintln!(
                "host_post_submit_unavailable stage={:?} {}",
                failure.stage,
                failure.error.message()
            );
        }
    }
}

#[test]
fn host_post_submit_oom_wait_typed_failure_or_unavailable() {
    use std::time::Duration;
    let request = CapabilityRequest::diagnose_default();
    match GpuContext::open(&request) {
        Ok(mut ctx) => {
            let buf = ctx
                .create_buffer(BufferDesc {
                    label: ResourceLabel::parse("spirula:gpu:buffer:post-oom").unwrap(),
                    size: 64,
                    usage: BufferUsage::COPY_SRC_DST,
                })
                .unwrap();
            ctx.upload_buffer(&buf, 0, &[5u8; 64]).unwrap();
            ctx.inject_next_post_submit_scope_error(ScopeErrorInject::OutOfMemory {
                requested_bytes: Some(64),
                current_bytes: Some(64),
                budget_bytes: Some(1 << 20),
            });
            let handle = ctx
                .submit()
                .expect("post-submit OOM must return waitable handle");
            assert_eq!(ctx.retained_submission_count(), 1);
            let err = ctx
                .wait(handle, Some(Duration::from_secs(5)))
                .expect_err("post-submit OOM must surface on wait");
            assert_eq!(err.code().as_str(), "SP-GPU-004");
            assert_eq!(err.context_value(ContextKey::Command), Some("submit"));
            drop(buf);
            assert_eq!(ctx.retained_submission_count(), 0);
            assert_eq!(ctx.resource_accounting().current_bytes, 0);
            ctx.close();
        }
        Err(failure) => {
            eprintln!(
                "host_post_submit_oom_unavailable stage={:?} {}",
                failure.stage,
                failure.error.message()
            );
        }
    }
}
