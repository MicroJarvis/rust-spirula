//! Local CUA window probe for P6-F07 (software renderer).
//!
//! GPU opens on the presentation worker. UI never blocks waiting for GPU.
//! set-image / software-paint flags are marked only after real assign / paint.
//! Does **not** claim wgpu surface present.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use slint::{ComponentHandle, RenderingState, Timer, TimerMode};
use spirula_application::{PresentationOwner, PresentationStatus};
use spirula_ui::{
    SetImageEvidence, image_from_presented_frame, mark_property_assigned,
    mark_software_paint_verified,
};

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

struct ProbeState {
    owner: PresentationOwner,
    evidence: Option<SetImageEvidence>,
    frame_set: bool,
    paint_marked: bool,
}

fn main() {
    let ui = PresentationProbe::new().expect("slint UI");
    ui.set_status_text("starting presentation worker (GPU opens off UI thread)…".into());

    let owner = match PresentationOwner::start(64, 48) {
        Ok(o) => o,
        Err(e) => {
            ui.set_status_text(format!("start failed: {e}").into());
            let _ = ui.run();
            return;
        }
    };

    let state = Rc::new(RefCell::new(ProbeState {
        owner,
        evidence: None,
        frame_set: false,
        paint_marked: false,
    }));

    // Rendering notifier: only AfterRendering may mark software-paint verified.
    // Software backend may return Unsupported — leave software_paint unverified.
    let state_paint = Rc::clone(&state);
    let ui_paint = ui.as_weak();
    match ui
        .window()
        .set_rendering_notifier(move |rendering_state, _api| {
            match rendering_state {
                RenderingState::AfterRendering => {}
                _ => return,
            }
            let mut st = state_paint.borrow_mut();
            if st.paint_marked {
                return;
            }
            let status_line = match st.evidence.as_mut() {
                Some(ev) if ev.set_image => {
                    mark_software_paint_verified(ev);
                    Some(format!(
                        "{}\nsoftware_paint=verified claims_wgpu_surface_present=false",
                        ev.status
                    ))
                }
                _ => None,
            };
            if let Some(line) = status_line {
                st.paint_marked = true;
                drop(st);
                if let Some(ui) = ui_paint.upgrade() {
                    ui.set_status_text(line.into());
                }
            }
        }) {
        Ok(()) => {}
        Err(e) => {
            ui.set_status_text(
                format!(
                    "rendering notifier unavailable ({e:?}); software_paint stays unverified \
                     (not wgpu surface present)"
                )
                .into(),
            );
        }
    }

    let ui_weak = ui.as_weak();
    let state_tick = Rc::clone(&state);
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(16), move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let mut st = state_tick.borrow_mut();
        match st.owner.status() {
            PresentationStatus::Opening => {
                ui.set_status_text("Opening GPU on presentation worker…".into());
            }
            PresentationStatus::Failed { code, message } => {
                ui.set_status_text(format!("GPU open failed: {code:?}: {message}").into());
            }
            PresentationStatus::Closed => {
                ui.set_status_text("presentation closed".into());
            }
            PresentationStatus::Ready { device_label } => {
                if !st.frame_set {
                    let _ = st.owner.request_frame();
                    if let Some(d) = st.owner.take_latest() {
                        match image_from_presented_frame(&d.frame) {
                            Ok((image, mut evidence)) => {
                                ui.set_frame_image(image);
                                mark_property_assigned(&mut evidence);
                                ui.set_status_text(
                                    format!(
                                        "{}\n{}\ndevice={device_label}\nproduce_to_take={:?}\n\
                                         software_paint=unverified until AfterRendering\n\
                                         claims_wgpu_surface_present=false",
                                        evidence.status,
                                        format_copies(&evidence.copies),
                                        d.produce_to_take
                                    )
                                    .into(),
                                );
                                st.evidence = Some(evidence);
                                st.frame_set = true;
                            }
                            Err(e) => {
                                ui.set_status_text(format!("image build failed: {e}").into());
                            }
                        }
                    } else {
                        ui.set_status_text(
                            format!(
                                "Ready; waiting for frame… device={device_label} stats={:?}",
                                st.owner.stats()
                            )
                            .into(),
                        );
                    }
                }
            }
        }
    });

    // Event loop: non-blocking GPU/frame updates via timer. Keep window visible
    // for CUA (display / resize / close). Do not treat launch alone as feature done.
    let _ = ui.run();
    drop(timer);
    let owner = Rc::try_unwrap(state)
        .unwrap_or_else(|_| panic!("probe state still shared"))
        .into_inner()
        .owner;
    owner.shutdown();
}

fn format_copies(c: &spirula_core::FrameCopyReport) -> String {
    format!(
        "logical_rgba={} bpr={:?} staging={:?} ui_upload={:?} sync={:?} unavailable={:?}",
        c.logical_rgba_bytes,
        c.readback_bytes_per_row,
        c.readback_staging_bytes,
        c.ui_pixel_upload_bytes,
        c.sync_points,
        c.unavailable
    )
}
