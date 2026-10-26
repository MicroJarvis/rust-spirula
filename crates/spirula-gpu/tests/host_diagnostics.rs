//! P2-F05 host probes: validation scope + destroy/lost → typed error + events.
//!
//! Real hardware OOM is **unsupported** here — do not claim host OOM reproduction.
//! Synthetic `ScopeErrorInject::OutOfMemory` covers the typed SP-GPU-004 path.

use spirula_core::{ContextKey, ErrorCode};
use spirula_gpu::{
    BufferDesc, BufferUsage, CapabilityRequest, ContextStatus, DiagnosticKind, GpuContext,
    LostSource, ResourceLabel,
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
