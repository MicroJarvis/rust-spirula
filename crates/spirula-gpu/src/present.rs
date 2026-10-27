//! P6-F07: GPU → host RGBA probe for application presentation (readback path).
//!
//! Binding: `docs/phase-6/presentation-path.md`. No Slint types here.

use std::time::Instant;

use spirula_core::{
    CancelToken, ContextKey, DomainError, ErrorCode, FrameCopyReport, PresentationPathKind,
    PresentedFrame, cpu_probe_rgba8, rgba8_matches_cpu_probe,
};

use crate::context::{ContextStatus, GpuContext};
use crate::resource::{
    ResourceLabel, Texture2dDesc, TextureFormat, TextureUsage, rgba8_padded_bytes,
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
        None => format!(
            "wgpu={} status={:?}",
            crate::enumerate::WGPU_VERSION,
            // report missing still Live in synthetic paths
            "unknown"
        ),
    }
}

/// Produce one CPU-exact RGBA8 frame via texture upload → readback → map.
///
/// Path is always [`PresentationPathKind::ReadbackToHost`]. Does **not** use a
/// window or claim wgpu surface present.
pub fn produce_probe_rgba_frame(
    ctx: &mut GpuContext,
    width: u32,
    height: u32,
    frame_id: u64,
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
    let tex = ctx.create_texture_2d(Texture2dDesc {
        label: label("spirula:present:texture:probe-rgba"),
        width,
        height,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsage {
            copy_src: true,
            copy_dst: true,
        },
    })?;
    if let Some(c) = cancel
        && c.is_cancelled()
    {
        drop(tex);
        return Err(canceled());
    }
    ctx.upload_texture_rgba8(&tex, &pattern)?;
    let ticket = ctx.readback_texture_rgba8(&tex)?;
    let submission = ctx.submit()?;
    let mut sync = vec!["submit"];
    ctx.wait(submission, None)?;
    sync.push("wait");
    if let Some(c) = cancel
        && c.is_cancelled()
    {
        drop(ticket);
        drop(tex);
        return Err(canceled());
    }
    let mapped = ctx.map_readback(&ticket)?;
    sync.push("map");
    drop(ticket);
    drop(tex);

    if !rgba8_matches_cpu_probe(width, height, &mapped) {
        return Err(DomainError::try_new(
            ErrorCode::Internal,
            "presentation readback mismatch vs cpu_probe_rgba8",
        )
        .expect("msg"));
    }

    let staging = rgba8_padded_bytes(width, height).ok();

    Ok(PresentedFrame {
        frame_id,
        width,
        height,
        rgba8: mapped,
        path: PresentationPathKind::ReadbackToHost,
        copies: FrameCopyReport {
            logical_rgba_bytes: (width as u64) * (height as u64) * 4,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::CapabilityRequest;
    use crate::context::GpuContext;

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
        let frame = produce_probe_rgba_frame(&mut ctx, 8, 4, 1, None).expect("frame");
        assert_eq!(frame.width, 8);
        assert_eq!(frame.height, 4);
        assert!(rgba8_matches_cpu_probe(8, 4, &frame.rgba8));
        assert_eq!(frame.path, PresentationPathKind::ReadbackToHost);
        assert!(frame.copies.sync_points.contains(&"wait"));
        drop(frame);
        assert_eq!(ctx.status(), ContextStatus::Live);
        let snap = ctx.resource_accounting();
        assert_eq!(snap.current_bytes, 0, "resources must drop to current=0");
        ctx.close();
    }

    #[test]
    fn rejects_zero_size() {
        let request = CapabilityRequest::diagnose_default();
        let Ok(mut ctx) = GpuContext::open(&request) else {
            return;
        };
        let err = produce_probe_rgba_frame(&mut ctx, 0, 8, 1, None).expect_err("size");
        assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
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
        let err = produce_probe_rgba_frame(&mut ctx, 4, 4, 1, Some(&token)).expect_err("cancel");
        assert_eq!(err.code(), ErrorCode::Canceled);
        ctx.close();
    }
}
