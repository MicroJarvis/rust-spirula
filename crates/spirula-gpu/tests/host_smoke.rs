//! P2-F06 host smoke: compute pass/skip + fail probes; surface unsupported.

use spirula_core::EXIT_SUCCESS;
use spirula_gpu::{
    CapabilityRequest, ContextStatus, GpuContext, SmokeStatus, format_smoke_report,
    probe_smoke_lost_fail, probe_smoke_timeout_fail, probe_smoke_validation_fail,
    run_compute_smoke, run_compute_smoke_on_context, run_compute_smoke_repeat,
    run_surface_smoke_gated, smoke_exit_code,
};

#[test]
fn host_compute_smoke_pass_or_precise_skip() {
    let request = CapabilityRequest::diagnose_default();
    let report = run_compute_smoke(&request);
    eprintln!("{}", format_smoke_report(&report));
    match report.status {
        SmokeStatus::Pass => {
            assert_eq!(report.match_cpu, Some(true));
            assert_eq!(report.after_drop_current_bytes, Some(0));
            assert_eq!(report.retained_submissions, Some(0));
            assert!(report.device_name.is_some());
            assert!(report.backend.is_some());
            assert!(report.features.is_some());
            assert!(report.device_limits.is_some());
            assert_eq!(report.context_status, Some(ContextStatus::Live));
        }
        SmokeStatus::Skip => {
            assert!(report.open_stage.is_some());
            assert!(report.reason.is_some());
            assert!(report.error_code.is_some());
            eprintln!(
                "host_compute_smoke skipped stage={:?} code={:?}",
                report.open_stage, report.error_code
            );
        }
        other => panic!("unexpected compute smoke status={other:?}"),
    }
    assert_eq!(smoke_exit_code(&report), EXIT_SUCCESS);
}

#[test]
fn host_compute_smoke_repeat_or_skip() {
    let request = CapabilityRequest::diagnose_default();
    let a = run_compute_smoke_repeat(&request, 0);
    let b = run_compute_smoke_repeat(&request, 1);
    eprintln!(
        "repeat0 status={:?} repeat1 status={:?}",
        a.status, b.status
    );
    assert_eq!(a.status, b.status);
    assert_eq!(smoke_exit_code(&a), EXIT_SUCCESS);
    assert_eq!(smoke_exit_code(&b), EXIT_SUCCESS);
    if a.status == SmokeStatus::Pass {
        assert_eq!(a.after_drop_current_bytes, Some(0));
        assert_eq!(b.after_drop_current_bytes, Some(0));
    }
}

#[test]
fn host_smoke_fail_probes_or_unavailable() {
    let request = CapabilityRequest::diagnose_default();
    match GpuContext::open(&request) {
        Ok(mut ctx) => {
            let v = probe_smoke_validation_fail(&mut ctx);
            eprintln!("validation probe: {}", format_smoke_report(&v));
            assert_eq!(v.status, SmokeStatus::Fail);
            assert_eq!(v.error_code.as_deref(), Some("SP-INP-003"));

            let t = probe_smoke_timeout_fail(&mut ctx);
            eprintln!("timeout probe: {}", format_smoke_report(&t));
            assert_eq!(t.status, SmokeStatus::Fail);
            assert_eq!(t.error_code.as_deref(), Some("SP-GPU-003"));
            assert_eq!(ctx.retained_submission_count(), 0);
            assert_eq!(ctx.resource_accounting().current_bytes, 0);

            let lost = probe_smoke_lost_fail(&mut ctx);
            eprintln!("lost probe: {}", format_smoke_report(&lost));
            assert_eq!(lost.status, SmokeStatus::Fail);
            assert_eq!(lost.error_code.as_deref(), Some("SP-GPU-002"));
            assert_eq!(ctx.status(), ContextStatus::Lost);
            ctx.close();
        }
        Err(failure) => {
            eprintln!(
                "host_smoke_fail_probes unavailable stage={:?} {}",
                failure.stage,
                failure.error.message()
            );
        }
    }
}

#[test]
fn host_submit_inject_fail_reclaims_then_retry_or_unavailable() {
    let request = CapabilityRequest::diagnose_default();
    match GpuContext::open(&request) {
        Ok(mut ctx) => {
            ctx.inject_next_submit_error();
            let fail =
                run_compute_smoke_on_context(&mut ctx).expect_err("injected submit must Fail");
            eprintln!("submit inject fail: {}", format_smoke_report(&fail));
            assert_eq!(fail.status, SmokeStatus::Fail);
            assert_eq!(fail.error_code.as_deref(), Some("SP-GPU-003"));
            assert_eq!(fail.after_drop_current_bytes, Some(0));
            assert_eq!(fail.retained_submissions, Some(0));
            assert_eq!(
                ctx.resource_accounting().current_bytes,
                0,
                "stranded pending must not remain after smoke Fail"
            );
            assert_eq!(ctx.retained_submission_count(), 0);
            assert_eq!(ctx.pending_resource_counts().stagings, 0);
            assert_eq!(ctx.pending_resource_counts().commands, 0);

            let pass = run_compute_smoke_on_context(&mut ctx).expect("retry pass");
            eprintln!("retry pass: {}", format_smoke_report(&pass));
            assert_eq!(pass.status, SmokeStatus::Pass);
            assert_eq!(pass.after_drop_current_bytes, Some(0));
            assert_eq!(ctx.resource_accounting().current_bytes, 0);
            ctx.close();
        }
        Err(failure) => {
            eprintln!(
                "host_submit_inject unavailable stage={:?} {}",
                failure.stage,
                failure.error.message()
            );
        }
    }
}

#[test]
fn host_surface_smoke_unsupported_no_forged_window() {
    let report = run_surface_smoke_gated(&CapabilityRequest::diagnose_default(), false);
    eprintln!("{}", format_smoke_report(&report));
    assert_eq!(report.status, SmokeStatus::Unsupported);
    assert!(
        report
            .reason
            .as_deref()
            .is_some_and(|r| r.contains("will not forge a window"))
    );
    assert_eq!(smoke_exit_code(&report), EXIT_SUCCESS);

    let opted = run_surface_smoke_gated(&CapabilityRequest::diagnose_default(), true);
    eprintln!("opt-in surface: {}", format_smoke_report(&opted));
    assert!(
        matches!(opted.status, SmokeStatus::Unsupported | SmokeStatus::Skip),
        "opt-in must not forge a pass: {:?}",
        opted.status
    );
    assert_eq!(smoke_exit_code(&opted), EXIT_SUCCESS);
}
