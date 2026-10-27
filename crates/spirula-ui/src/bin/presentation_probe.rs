//! Local CUA window probe for P6-F07 (software renderer).
//!
//! Does **not** claim wgpu surface present verification. Frames come from
//! application presentation (GPU readback → host → Slint Image upload).

use std::thread;
use std::time::Duration;

use spirula_application::PresentationOwner;
use spirula_ui::{mark_software_paint, set_image_from_presented_frame};

slint::slint! {
    export component PresentationProbe inherits Window {
        title: "Spirula P6-F07 presentation probe";
        preferred-width: 480px;
        preferred-height: 360px;
        in-out property <image> frame-image;
        in-out property <string> status-text: "starting…";
        VerticalLayout {
            padding: 8px;
            spacing: 8px;
            Text {
                text: root.status-text;
                wrap: word-wrap;
            }
            Image {
                source: root.frame-image;
                image-fit: contain;
            }
        }
    }
}

fn main() {
    let ui = PresentationProbe::new().expect("slint UI");
    ui.set_status_text("opening presentation owner…".into());

    let owner = match PresentationOwner::open(64, 48) {
        Ok(o) => o,
        Err(e) => {
            ui.set_status_text(format!("open failed: {e} (no GPU?)").into());
            let _ = ui.run();
            return;
        }
    };

    let label = owner.device_label();
    owner.request_frame().expect("request");
    let mut delivered = None;
    for _ in 0..200 {
        if let Some(d) = owner.take_latest() {
            delivered = Some(d);
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }

    match delivered {
        Some(d) => match set_image_from_presented_frame(&d.frame) {
            Ok((image, mut evidence)) => {
                ui.set_frame_image(image);
                mark_software_paint(&mut evidence);
                ui.set_status_text(
                    format!(
                        "{}\n{}\ndevice={label}\nproduce_to_take={:?}\nclaims_wgpu_surface_present={}",
                        evidence.status,
                        format_copies(&evidence.copies),
                        d.produce_to_take,
                        evidence.claims_wgpu_surface_present
                    )
                    .into(),
                );
            }
            Err(e) => ui.set_status_text(format!("set-image failed: {e}").into()),
        },
        None => {
            ui.set_status_text(
                format!(
                    "no frame; stats={:?} last_error={:?} device={label}",
                    owner.stats(),
                    owner.last_error().map(|e| e.to_string())
                )
                .into(),
            );
        }
    }

    // Window show / event loop = software paint path, not wgpu present.
    let _ = ui.run();
    owner.shutdown();
}

fn format_copies(c: &spirula_core::FrameCopyReport) -> String {
    format!(
        "logical_rgba={} staging={:?} ui_upload={:?} sync={:?} unavailable={:?}",
        c.logical_rgba_bytes,
        c.readback_staging_bytes,
        c.ui_pixel_upload_bytes,
        c.sync_points,
        c.unavailable
    )
}
