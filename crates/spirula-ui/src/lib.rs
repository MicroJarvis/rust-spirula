//! Slint components, view models, commands, and UI-facing event translation.
//!
//! P6-F07: host [`PresentedFrame`] → Slint `Image` path. No `wgpu` types and
//! no direct GPU/IO. Binding: `docs/phase-6/presentation-path.md`.

#![forbid(unsafe_code)]

mod probe_driver;

pub use probe_driver::{ProbeDriver, ProbeStep, drive_owner};

use slint::{Image, Rgba8Pixel, SharedPixelBuffer, SharedString};
use spirula_core::{
    ContextKey, DomainError, ErrorCode, FrameCopyReport, PresentedFrame, rgba8_matches_cpu_probe,
};

/// Evidence for host→Slint conversion. Flags start **false / unverified**.
///
/// - Building an [`Image`] alone is **not** property assignment.
/// - Property assign must call [`mark_property_assigned`] after a real
///   `set_*` on the component.
/// - Software paint must call [`mark_software_paint_verified`] only from a
///   verified paint path (e.g. `RenderingState::AfterRendering`). Unknown
///   stays unverified — never claim paint before the window draws.
#[derive(Debug, Clone)]
pub struct SetImageEvidence {
    pub width: u32,
    pub height: u32,
    pub frame_id: u64,
    pub upload_bytes: u64,
    /// True only after [`mark_property_assigned`].
    pub set_image: bool,
    /// True only after [`mark_software_paint_verified`].
    pub software_paint: bool,
    /// Always false in this slice — readback path does not verify surface present.
    pub claims_wgpu_surface_present: bool,
    pub copies: FrameCopyReport,
    pub status: SharedString,
}

/// Build a Slint `Image` from a host frame (pixel copy only).
///
/// Does **not** mark set-image or software-paint. Sync label is
/// `host-pixel-copy` until a real property assign / paint is verified.
pub fn image_from_presented_frame(
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
    let _ = rgba8_matches_cpu_probe(frame.width, frame.height, &frame.rgba8);

    let buffer =
        SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&frame.rgba8, frame.width, frame.height);
    let upload_bytes = expected as u64;
    let image = Image::from_rgba8(buffer);

    let mut copies = frame.copies.clone();
    copies.ui_pixel_upload_bytes = Some(upload_bytes);
    if !copies.sync_points.contains(&"host-pixel-copy") {
        copies.sync_points.push("host-pixel-copy");
    }

    let evidence = SetImageEvidence {
        width: frame.width,
        height: frame.height,
        frame_id: frame.frame_id,
        upload_bytes,
        set_image: false,
        software_paint: false,
        claims_wgpu_surface_present: false,
        copies,
        status: SharedString::from(format!(
            "host-pixel-copy frame_id={} {}x{} upload_bytes={upload_bytes} \
             set_image=unverified software_paint=unverified (not wgpu surface present)",
            frame.frame_id, frame.width, frame.height
        )),
    };
    Ok((image, evidence))
}

/// Mark that a component property was actually assigned (`set_frame_image`, …).
pub fn mark_property_assigned(evidence: &mut SetImageEvidence) {
    evidence.set_image = true;
    if !evidence.copies.sync_points.contains(&"set-image") {
        evidence.copies.sync_points.push("set-image");
    }
    evidence.claims_wgpu_surface_present = false;
    evidence.status = SharedString::from(format!(
        "set-image frame_id={} {}x{} upload_bytes={} software_paint=unverified \
         (not wgpu surface present)",
        evidence.frame_id, evidence.width, evidence.height, evidence.upload_bytes
    ));
}

/// Mark software paint only after a verified render callback (e.g. AfterRendering).
pub fn mark_software_paint_verified(evidence: &mut SetImageEvidence) {
    evidence.software_paint = true;
    if !evidence.copies.sync_points.contains(&"software-paint") {
        evidence.copies.sync_points.push("software-paint");
    }
    evidence.claims_wgpu_surface_present = false;
    evidence.status = SharedString::from(format!(
        "software-paint verified after set-image frame_id={} (wgpu_surface_present unverified)",
        evidence.frame_id
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use spirula_core::{PresentationPathKind, cpu_probe_rgba8};

    fn sample_frame() -> PresentedFrame {
        PresentedFrame {
            frame_id: 7,
            size_generation: 1,
            width: 4,
            height: 2,
            rgba8: cpu_probe_rgba8(4, 2),
            path: PresentationPathKind::ReadbackToHost,
            copies: FrameCopyReport {
                logical_rgba_bytes: 32,
                readback_bytes_per_row: Some(256),
                readback_staging_bytes: Some(512),
                ui_pixel_upload_bytes: None,
                sync_points: vec!["submit", "wait", "map"],
                produce_duration: None,
                unavailable: vec!["wgpu_surface_present"],
            },
            device_label: "test".into(),
        }
    }

    #[test]
    fn build_image_does_not_claim_set_or_paint() {
        let (image, evidence) = image_from_presented_frame(&sample_frame()).expect("build");
        assert_eq!(image.size().width, 4);
        assert!(!evidence.set_image);
        assert!(!evidence.software_paint);
        assert!(!evidence.claims_wgpu_surface_present);
        assert!(evidence.copies.sync_points.contains(&"host-pixel-copy"));
        assert!(!evidence.copies.sync_points.contains(&"set-image"));
    }

    #[test]
    fn mark_assign_then_paint() {
        let (image, mut evidence) = image_from_presented_frame(&sample_frame()).expect("build");
        mark_property_assigned(&mut evidence);
        assert!(evidence.set_image);
        assert!(!evidence.software_paint);
        let _ = image;
        mark_software_paint_verified(&mut evidence);
        assert!(evidence.software_paint);
        assert!(!evidence.claims_wgpu_surface_present);
    }

    #[test]
    fn rejects_len_mismatch() {
        let frame = PresentedFrame {
            frame_id: 1,
            size_generation: 0,
            width: 2,
            height: 2,
            rgba8: vec![0; 8],
            path: PresentationPathKind::ReadbackToHost,
            copies: FrameCopyReport::default(),
            device_label: String::new(),
        };
        let err = image_from_presented_frame(&frame).expect_err("len");
        assert_eq!(err.code(), ErrorCode::InvalidArgument);
    }
}
