//! P6-F07: GPU → host RGBA probe for application presentation (readback path).
//!
//! Binding: `docs/phase-6/presentation-path.md`. No Slint types here.
//!
//! Fail paths reuse P2-F06 reclaim semantics: abandon unsubmitted pending, wait
//! any accepted submission, drop handles → `current_bytes==0` and
//! `retained_submission_count==0` while Live (retryable).

use std::time::{Duration, Instant};

use spirula_core::{
    CancelToken, ContextKey, DomainError, ErrorCode, FrameCopyReport, PresentationPathKind,
    PresentedFrame, cpu_probe_rgba8, rgba8_matches_cpu_probe,
};

use crate::context::{ContextStatus, GpuContext};
use crate::resource::{
    ReadbackTicket, ResourceLabel, SubmissionHandle, Texture2dDesc, TextureFormat, TextureHandle,
    TextureUsage, rgba8_bytes_per_row, rgba8_padded_bytes,
};

fn label(s: &str) -> ResourceLabel {
    ResourceLabel::parse(s).expect("present label")
}

fn canceled() -> DomainError {
    DomainError::try_new(ErrorCode::Canceled, "presentation probe canceled")
        .expect("msg")
        .with_context(ContextKey::Actual, "cancel")
        .expect("ctx")
}

fn bad_size(msg: impl Into<String>) -> DomainError {
    DomainError::try_new(ErrorCode::ValueOutOfRange, msg)
        .expect("msg")
        .with_context(ContextKey::Field, "size")
        .expect("ctx")
}

fn device_label(ctx: &GpuContext) -> String {
    match ctx.report() {
        Some(r) => format!(
            "wgpu={} backend={} device={}",
            crate::enumerate::WGPU_VERSION,
            r.backend.as_str(),
            r.adapter.identity.name
        ),
        None => format!("wgpu={} status=unknown", crate::enumerate::WGPU_VERSION),
    }
}

fn is_wait_timeout(err: &DomainError) -> bool {
    err.code() == ErrorCode::SubmitFailed && err.message().contains("timed out")
}

/// Locals held across produce so Fail exits reclaim like P2-F06 smoke.
struct PresentLocals {
    tex: Option<TextureHandle>,
    ticket: Option<ReadbackTicket>,
    submission: Option<SubmissionHandle>,
}

impl PresentLocals {
    fn new() -> Self {
        Self {
            tex: None,
            ticket: None,
            submission: None,
        }
    }

    /// Drop ticket, abandon unsubmitted. Only wait a submission that is still
    /// accepted/in-flight. Already-reaped terminal wait errors leave
    /// `submission=None` (or yield "unknown submission handle") — treat as
    /// clean, preserve the caller's typed error. True in-flight timeout that
    /// cannot finish → close context and report (do not drop the only handle).
    fn reclaim(&mut self, ctx: &mut GpuContext) -> Result<(), DomainError> {
        self.ticket = None;
        ctx.abandon_unsubmitted_pending();
        if let Some(h) = self.submission {
            match wait_submission_deadline(ctx, h, RECLAIM_WAIT_DEADLINE, None) {
                Ok(()) => {
                    self.submission = None;
                }
                Err(e) if is_unknown_submission(&e) => {
                    // Prior wait already reaped (e.g. post-submit Validation/OOM).
                    self.submission = None;
                }
                Err(e) if is_wait_timeout(&e) || e.code() == ErrorCode::Canceled => {
                    // Still in-flight: cannot discard the only handle.
                    self.tex = None;
                    ctx.abandon_unsubmitted_pending();
                    if ctx.status() == ContextStatus::Live {
                        ctx.close();
                    }
                    self.submission = None;
                    return Err(DomainError::try_new(
                        ErrorCode::SubmitFailed,
                        format!(
                            "presentation reclaim wait failed; context closed to avoid \
                             dropping the only accepted submission handle ({})",
                            e.message()
                        ),
                    )
                    .expect("msg"));
                }
                Err(e) => {
                    // Other wait failures after a still-listed submission: try
                    // one more interpretation — if retained is already 0, treat
                    // as reaped; else close.
                    if ctx.retained_submission_count() == 0 {
                        self.submission = None;
                    } else {
                        self.tex = None;
                        ctx.abandon_unsubmitted_pending();
                        if ctx.status() == ContextStatus::Live {
                            ctx.close();
                        }
                        self.submission = None;
                        return Err(DomainError::try_new(
                            ErrorCode::SubmitFailed,
                            format!(
                                "presentation reclaim wait failed; context closed ({})",
                                e.message()
                            ),
                        )
                        .expect("msg"));
                    }
                }
            }
        }
        self.tex = None;
        ctx.abandon_unsubmitted_pending();
        Ok(())
    }

    fn fail(&mut self, ctx: &mut GpuContext, err: DomainError) -> DomainError {
        // Always prefer the original typed produce error when reclaim is clean.
        match self.reclaim(ctx) {
            Ok(()) => err,
            Err(reclaim_err) => reclaim_err,
        }
    }
}

fn is_unknown_submission(err: &DomainError) -> bool {
    err.message().contains("unknown submission handle")
}

const WAIT_SLICE: Duration = Duration::from_millis(50);
const PRODUCE_WAIT_DEADLINE: Duration = Duration::from_secs(30);
const RECLAIM_WAIT_DEADLINE: Duration = Duration::from_secs(5);

/// Wait an accepted submission with short slices, a **total deadline**, and
/// optional cancel. Timeout Err retains the submission for the next slice —
/// the handle must not be dropped while retained.
///
/// `wait(..., None)` is not used: an uncancellable host wait would ignore Stop,
/// and cancel=None must still bound wall time.
fn wait_submission_deadline(
    ctx: &mut GpuContext,
    submission: SubmissionHandle,
    deadline: Duration,
    cancel: Option<&CancelToken>,
) -> Result<(), DomainError> {
    let start = Instant::now();
    loop {
        if let Some(c) = cancel
            && c.is_cancelled()
        {
            return Err(canceled());
        }
        if start.elapsed() >= deadline {
            return Err(DomainError::try_new(
                ErrorCode::SubmitFailed,
                format!(
                    "submission wait timed out (caller deadline {:?}; in-flight staging retained)",
                    deadline
                ),
            )
            .expect("msg"));
        }
        let slice = WAIT_SLICE.min(deadline.saturating_sub(start.elapsed()));
        if slice.is_zero() {
            continue;
        }
        match ctx.wait(submission, Some(slice)) {
            Ok(()) => return Ok(()),
            Err(e) if is_wait_timeout(&e) => continue,
            Err(e) => return Err(e),
        }
    }
}

fn wait_cancelable(
    ctx: &mut GpuContext,
    submission: SubmissionHandle,
    cancel: Option<&CancelToken>,
) -> Result<(), DomainError> {
    wait_submission_deadline(ctx, submission, PRODUCE_WAIT_DEADLINE, cancel)
}

/// Produce one CPU-exact RGBA8 frame via texture upload → readback → map.
///
/// Path is always [`PresentationPathKind::ReadbackToHost`]. Does **not** use a
/// window or claim wgpu surface present. On Err, reclaim attempts
/// `current_bytes==0` / `retained==0` while Live; if reclaim wait cannot finish,
/// the context is **closed** and the error reports that — never claim cleanup
/// after dropping the only accepted handle on a timeout.
pub fn produce_probe_rgba_frame(
    ctx: &mut GpuContext,
    width: u32,
    height: u32,
    frame_id: u64,
    size_generation: u64,
    cancel: Option<&CancelToken>,
) -> Result<PresentedFrame, DomainError> {
    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        return Err(bad_size(format!(
            "presentation size {width}x{height} out of allowed 1..=4096"
        )));
    }
    if let Some(c) = cancel {
        c.check().map_err(|_| canceled())?;
    }
    if ctx.status() != ContextStatus::Live {
        return Err(DomainError::try_new(
            ErrorCode::DeviceLost,
            "presentation requires a live GpuContext",
        )
        .expect("msg"));
    }

    let started = Instant::now();
    let pattern = cpu_probe_rgba8(width, height);
    let mut locals = PresentLocals::new();

    match ctx.create_texture_2d(Texture2dDesc {
        label: label("spirula:present:texture:probe-rgba"),
        width,
        height,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsage {
            copy_src: true,
            copy_dst: true,
        },
    }) {
        Ok(t) => locals.tex = Some(t),
        Err(e) => return Err(locals.fail(ctx, e)),
    }

    if let Some(c) = cancel
        && c.is_cancelled()
    {
        return Err(locals.fail(ctx, canceled()));
    }

    let tex = locals.tex.as_ref().expect("tex");
    if let Err(e) = ctx.upload_texture_rgba8(tex, &pattern) {
        return Err(locals.fail(ctx, e));
    }
    match ctx.readback_texture_rgba8(tex) {
        Ok(t) => locals.ticket = Some(t),
        Err(e) => return Err(locals.fail(ctx, e)),
    }

    match ctx.submit() {
        Ok(h) => locals.submission = Some(h),
        Err(e) => return Err(locals.fail(ctx, e)),
    }

    let submission = *locals.submission.as_ref().expect("submission");
    if let Err(e) = wait_cancelable(ctx, submission, cancel) {
        // Post-submit Validation/OOM: wait already reaped → clear handle.
        // Timeout / cancel mid-wait: submission still accepted → reclaim waits.
        if !is_wait_timeout(&e) && e.code() != ErrorCode::Canceled {
            locals.submission = None;
        }
        return Err(locals.fail(ctx, e));
    }
    // Successful wait reaped the submission record.
    locals.submission = None;

    let mut sync = vec!["submit", "wait"];
    if let Some(c) = cancel
        && c.is_cancelled()
    {
        return Err(locals.fail(ctx, canceled()));
    }

    let ticket = locals.ticket.as_ref().expect("ticket");
    let mapped = match ctx.map_readback(ticket) {
        Ok(m) => m,
        Err(e) => return Err(locals.fail(ctx, e)),
    };
    sync.push("map");
    locals.ticket = None;
    locals.tex = None;

    if !rgba8_matches_cpu_probe(width, height, &mapped) {
        return Err(locals.fail(
            ctx,
            DomainError::try_new(
                ErrorCode::Internal,
                "presentation readback mismatch vs cpu_probe_rgba8",
            )
            .expect("msg"),
        ));
    }

    let bpr = rgba8_bytes_per_row(width).ok();
    let staging = rgba8_padded_bytes(width, height).ok();

    Ok(PresentedFrame {
        frame_id,
        size_generation,
        width,
        height,
        rgba8: mapped,
        path: PresentationPathKind::ReadbackToHost,
        copies: FrameCopyReport {
            logical_rgba_bytes: (width as u64) * (height as u64) * 4,
            readback_bytes_per_row: bpr,
            readback_staging_bytes: staging,
            ui_pixel_upload_bytes: None,
            sync_points: sync,
            produce_duration: Some(started.elapsed()),
            unavailable: vec![
                "compositor_present_latency",
                "shared_texture_zero_copy",
                "wgpu_surface_present",
            ],
        },
        device_label: device_label(ctx),
    })
}

/// Exact cleanup snapshot after a successful produce (handles already dropped).
pub fn assert_presentation_cleanup(ctx: &GpuContext) {
    let snap = ctx.resource_accounting();
    let pending = ctx.pending_resource_counts();
    let retained = ctx.retained_submission_count();
    assert_eq!(snap.current_bytes, 0, "current_bytes must be exactly 0");
    assert_eq!(pending.commands, 0, "pending commands must be 0");
    assert_eq!(pending.stagings, 0, "pending stagings must be 0");
    assert_eq!(pending.buffers, 0, "pending buffers must be 0");
    assert_eq!(pending.textures, 0, "pending textures must be 0");
    assert_eq!(pending.current_bytes, 0, "pending current_bytes must be 0");
    assert_eq!(retained, 0, "retained submissions must be 0");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::CapabilityRequest;
    use crate::context::GpuContext;
    use crate::resource::{COPY_BYTES_PER_ROW_ALIGNMENT, rgba8_bytes_per_row, rgba8_padded_bytes};
    use spirula_core::{cpu_probe_corners, rgba8_corners};

    #[test]
    fn host_probe_frame_cpu_exact_or_skip() {
        let request = CapabilityRequest::diagnose_default();
        let mut ctx = match GpuContext::open(&request) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("skip: no GPU ({})", e.error);
                return;
            }
        };
        let frame = produce_probe_rgba_frame(&mut ctx, 8, 4, 1, 0, None).expect("frame");
        assert_eq!(frame.width, 8);
        assert_eq!(frame.height, 4);
        assert!(rgba8_matches_cpu_probe(8, 4, &frame.rgba8));
        assert_eq!(frame.path, PresentationPathKind::ReadbackToHost);
        assert!(frame.copies.sync_points.contains(&"wait"));
        drop(frame);
        assert_eq!(ctx.status(), ContextStatus::Live);
        assert_presentation_cleanup(&ctx);
        ctx.close();
    }

    #[test]
    fn pitch_and_corners_for_odd_sizes_or_skip() {
        let request = CapabilityRequest::diagnose_default();
        let mut ctx = match GpuContext::open(&request) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("skip: no GPU ({})", e.error);
                return;
            }
        };
        for (w, h) in [(1u32, 1u32), (63, 2), (65, 3)] {
            let frame = produce_probe_rgba_frame(&mut ctx, w, h, 1, 0, None).expect("frame");
            let bpr = rgba8_bytes_per_row(w).unwrap();
            let padded = rgba8_padded_bytes(w, h).unwrap();
            assert_eq!(frame.copies.readback_bytes_per_row, Some(bpr));
            assert_eq!(frame.copies.readback_staging_bytes, Some(padded));
            assert_eq!(bpr % COPY_BYTES_PER_ROW_ALIGNMENT, 0);
            assert_eq!(
                frame.copies.logical_rgba_bytes,
                u64::from(w) * u64::from(h) * 4
            );
            assert_eq!(frame.rgba8.len() as u64, frame.copies.logical_rgba_bytes);
            let expected = cpu_probe_corners(w, h).unwrap();
            assert_eq!(rgba8_corners(w, h, &frame.rgba8), Some(expected));
            drop(frame);
            assert_presentation_cleanup(&ctx);
        }
        ctx.close();
    }

    #[test]
    fn rejects_zero_and_oversize() {
        let request = CapabilityRequest::diagnose_default();
        let Ok(mut ctx) = GpuContext::open(&request) else {
            return;
        };
        for (w, h) in [(0u32, 8u32), (8, 0), (4097, 1), (1, 4097)] {
            let err = produce_probe_rgba_frame(&mut ctx, w, h, 1, 0, None).expect_err("size");
            assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
        }
        assert_presentation_cleanup(&ctx);
        ctx.close();
    }

    #[test]
    fn cancel_before_produce() {
        let request = CapabilityRequest::diagnose_default();
        let Ok(mut ctx) = GpuContext::open(&request) else {
            return;
        };
        let token = CancelToken::new();
        token.request(spirula_core::CancelReason::User);
        let err = produce_probe_rgba_frame(&mut ctx, 4, 4, 1, 0, Some(&token)).expect_err("cancel");
        assert_eq!(err.code(), ErrorCode::Canceled);
        assert_presentation_cleanup(&ctx);
        ctx.close();
    }

    #[test]
    fn submit_inject_reclaims_and_retries_or_skip() {
        let request = CapabilityRequest::diagnose_default();
        let mut ctx = match GpuContext::open(&request) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("skip: no GPU ({})", e.error);
                return;
            }
        };
        ctx.inject_next_submit_error();
        let err = produce_probe_rgba_frame(&mut ctx, 65, 3, 1, 0, None).expect_err("submit");
        assert_eq!(err.code(), ErrorCode::SubmitFailed);
        assert_presentation_cleanup(&ctx);
        let frame = produce_probe_rgba_frame(&mut ctx, 65, 3, 2, 0, None).expect("retry");
        assert_eq!((frame.width, frame.height), (65, 3));
        drop(frame);
        assert_presentation_cleanup(&ctx);
        ctx.close();
    }

    #[test]
    fn wait_timeout_inject_is_retried_then_cleanup_or_skip() {
        // Cancelable wait slices retry after inject timeout (handle retained).
        // Must not drop the only SubmissionHandle on the first timeout Err.
        let request = CapabilityRequest::diagnose_default();
        let mut ctx = match GpuContext::open(&request) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("skip: no GPU ({})", e.error);
                return;
            }
        };
        ctx.inject_next_wait_timeout();
        let frame = produce_probe_rgba_frame(&mut ctx, 65, 3, 1, 0, None).expect("retry wait");
        assert_eq!((frame.width, frame.height), (65, 3));
        drop(frame);
        assert_presentation_cleanup(&ctx);
        ctx.close();
    }

    #[test]
    fn post_submit_validation_preserves_typed_err_live_retry_or_skip() {
        use crate::diagnostics::ScopeErrorInject;
        let request = CapabilityRequest::diagnose_default();
        let mut ctx = match GpuContext::open(&request) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("skip: no GPU ({})", e.error);
                return;
            }
        };
        ctx.inject_next_post_submit_scope_error(ScopeErrorInject::Validation);
        let err = produce_probe_rgba_frame(&mut ctx, 65, 3, 1, 0, None).expect_err("validation");
        assert_eq!(err.code(), ErrorCode::SubmitFailed, "{err}");
        assert!(
            err.message().contains("validation") || err.message().contains("injected"),
            "{}",
            err.message()
        );
        assert!(
            !err.message().contains("reclaim wait failed"),
            "must not overwrite typed err: {}",
            err.message()
        );
        assert_eq!(ctx.status(), ContextStatus::Live);
        assert_presentation_cleanup(&ctx);
        let frame = produce_probe_rgba_frame(&mut ctx, 8, 4, 2, 0, None).expect("retry");
        drop(frame);
        assert_presentation_cleanup(&ctx);
        ctx.close();
    }

    #[test]
    fn post_submit_oom_preserves_sp_gpu_004_live_retry_or_skip() {
        use crate::diagnostics::ScopeErrorInject;
        let request = CapabilityRequest::diagnose_default();
        let mut ctx = match GpuContext::open(&request) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("skip: no GPU ({})", e.error);
                return;
            }
        };
        ctx.inject_next_post_submit_scope_error(ScopeErrorInject::OutOfMemory {
            requested_bytes: Some(1536),
            current_bytes: Some(4608),
            budget_bytes: Some(8192),
        });
        let err = produce_probe_rgba_frame(&mut ctx, 65, 3, 1, 0, None).expect_err("oom");
        assert_eq!(err.code(), ErrorCode::AllocationFailed, "{err}");
        assert!(
            !err.message().contains("reclaim wait failed"),
            "must preserve SP-GPU-004: {}",
            err.message()
        );
        assert_eq!(ctx.status(), ContextStatus::Live);
        assert_presentation_cleanup(&ctx);
        let frame = produce_probe_rgba_frame(&mut ctx, 8, 4, 2, 0, None).expect("retry");
        drop(frame);
        assert_presentation_cleanup(&ctx);
        ctx.close();
    }

    #[test]
    fn wait_timeout_manual_reclaim_keeps_handle_or_skip() {
        use crate::resource::{BufferDesc, BufferUsage};
        let request = CapabilityRequest::diagnose_default();
        let mut ctx = match GpuContext::open(&request) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("skip: no GPU ({})", e.error);
                return;
            }
        };
        let buf = ctx
            .create_buffer(BufferDesc {
                label: label("spirula:present:buffer:timeout-probe"),
                size: 16,
                usage: BufferUsage::COPY_DST,
            })
            .expect("buf");
        ctx.upload_buffer(&buf, 0, &[0u8; 16]).expect("upload");
        let handle = ctx.submit().expect("submit");
        ctx.inject_next_wait_timeout();
        let err = ctx
            .wait(handle, Some(Duration::from_millis(1)))
            .expect_err("timeout");
        assert!(is_wait_timeout(&err), "{err}");
        assert_eq!(ctx.retained_submission_count(), 1);
        // Reclaim: same handle, longer wait (F06 semantics).
        ctx.wait(handle, Some(Duration::from_secs(5)))
            .expect("reclaim wait");
        drop(buf);
        assert_presentation_cleanup(&ctx);
        ctx.close();
    }
}
