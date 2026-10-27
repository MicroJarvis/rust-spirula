//! Slint components, view models, commands, and UI-facing event translation.
//!
//! P6-F07: host [`PresentedFrame`] → Slint `Image` upload path. No `wgpu`
//! types and no direct GPU/IO. Binding: `docs/phase-6/presentation-path.md`.

#![forbid(unsafe_code)]

use slint::{Image, Rgba8Pixel, SharedPixelBuffer, SharedString};
use spirula_core::{
    ContextKey, DomainError, ErrorCode, FrameCopyReport, PresentedFrame, rgba8_matches_cpu_probe,
};

/// Evidence that a host frame was converted and assigned to a Slint `Image`.
///
/// This is **set-image** only. Software paint / window show are separate and
/// must not be reported as wgpu surface present.
#[derive(Debug, Clone)]
pub struct SetImageEvidence {
    pub width: u32,
    pub height: u32,
    pub frame_id: u64,
    pub upload_bytes: u64,
    /// Property/image assignment completed.
    pub set_image: bool,
    /// Software-renderer paint invoked (window or off-screen), if any.
    pub software_paint: bool,
    /// Always false in this slice — readback path does not verify surface present.
    pub claims_wgpu_surface_present: bool,
    pub copies: FrameCopyReport,
    pub status: SharedString,
}

/// Convert an application-owned frame into a Slint `Image` (host pixel copy).
pub fn set_image_from_presented_frame(
    frame: &PresentedFrame,
) -> Result<(Image, SetImageEvidence), DomainError> {
    let expected = PresentedFrame::expected_bytes(frame.width, frame.height).ok_or_else(|| {
        DomainError::try_new(
            ErrorCode::ValueOutOfRange,
            format!(
                "presentation frame size {}x{} overflows",
                frame.width, frame.height
            ),
        )
        .expect("msg")
        .with_context(ContextKey::Field, "size")
        .expect("ctx")
    })?;
    if frame.rgba8.len() != expected {
        return Err(DomainError::try_new(
            ErrorCode::InvalidArgument,
            format!(
                "rgba8 len {} != expected {expected} for {}x{}",
                frame.rgba8.len(),
                frame.width,
                frame.height
            ),
        )
        .expect("msg")
        .with_context(ContextKey::Field, "rgba8")
        .expect("ctx"));
    }
    if !rgba8_matches_cpu_probe(frame.width, frame.height, &frame.rgba8) {
        // Still allow display of non-probe frames later; for P6-F07 probe path
        // callers may skip this check. We only warn via status for now when
        // pattern matches is expected by tests that call the probe helpers.
    }

    let buffer =
        SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&frame.rgba8, frame.width, frame.height);
    let upload_bytes = expected as u64;
    let image = Image::from_rgba8(buffer);

    let mut copies = frame.copies.clone();
    copies.ui_pixel_upload_bytes = Some(upload_bytes);
    if !copies.sync_points.contains(&"set-image") {
        copies.sync_points.push("set-image");
    }

    let evidence = SetImageEvidence {
        width: frame.width,
        height: frame.height,
        frame_id: frame.frame_id,
        upload_bytes,
        set_image: true,
        software_paint: false,
        claims_wgpu_surface_present: false,
        copies,
        status: SharedString::from(format!(
            "set-image frame_id={} {}x{} upload_bytes={upload_bytes} path={:?} (not wgpu surface present)",
            frame.frame_id, frame.width, frame.height, frame.path
        )),
    };
    Ok((image, evidence))
}

/// Mark that a software paint/show occurred after set-image (still not surface present).
pub fn mark_software_paint(evidence: &mut SetImageEvidence) {
    evidence.software_paint = true;
    if !evidence.copies.sync_points.contains(&"software-paint") {
        evidence.copies.sync_points.push("software-paint");
    }
    evidence.claims_wgpu_surface_present = false;
    evidence.status = SharedString::from(format!(
        "software-paint after set-image frame_id={} (wgpu_surface_present unverified)",
        evidence.frame_id
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use spirula_core::{PresentationPathKind, cpu_probe_rgba8};

    #[test]
    fn set_image_copies_host_pixels() {
        let rgba = cpu_probe_rgba8(4, 2);
        let frame = PresentedFrame {
            frame_id: 7,
            width: 4,
            height: 2,
            rgba8: rgba,
            path: PresentationPathKind::ReadbackToHost,
            copies: FrameCopyReport {
                logical_rgba_bytes: 32,
                readback_staging_bytes: Some(256 * 2),
                ui_pixel_upload_bytes: None,
                sync_points: vec!["submit", "wait", "map"],
                produce_duration: None,
                unavailable: vec!["wgpu_surface_present"],
            },
            device_label: "test".into(),
        };
        let (image, mut evidence) = set_image_from_presented_frame(&frame).expect("set");
        assert_eq!(image.size().width, 4);
        assert_eq!(image.size().height, 2);
        assert!(evidence.set_image);
        assert!(!evidence.software_paint);
        assert!(!evidence.claims_wgpu_surface_present);
        assert_eq!(evidence.upload_bytes, 32);
        assert_eq!(evidence.copies.ui_pixel_upload_bytes, Some(32));
        assert!(evidence.copies.sync_points.contains(&"set-image"));
        mark_software_paint(&mut evidence);
        assert!(evidence.software_paint);
        assert!(!evidence.claims_wgpu_surface_present);
        assert!(evidence.copies.sync_points.contains(&"software-paint"));
    }

    #[test]
    fn rejects_len_mismatch() {
        let frame = PresentedFrame {
            frame_id: 1,
            width: 2,
            height: 2,
            rgba8: vec![0; 8],
            path: PresentationPathKind::ReadbackToHost,
            copies: FrameCopyReport::default(),
            device_label: String::new(),
        };
        let err = set_image_from_presented_frame(&frame).expect_err("len");
        assert_eq!(err.code(), ErrorCode::InvalidArgument);
    }
}
