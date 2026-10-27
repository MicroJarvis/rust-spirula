//! Local CUA window probe for P6-F07 (software renderer).
//!
//! GPU opens on the presentation worker. UI never blocks waiting for GPU.
//! Cycles application resize → request → Slint display so CUA can verify
//! different size frames (not only stretching a fixed 64×48). Generation
//! gates stay in force. Does **not** claim wgpu surface present.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

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
        // Tech controls (no std-widgets): drive application resize.
        callback cycle-size();
        callback size-a();
        callback size-b();
        callback size-c();
        in-out property <image> frame-image;
        in-out property <string> status-text: "starting…";
        in-out property <string> size-label: "—";
        VerticalLayout {
            padding: 8px;
            spacing: 8px;
            Text {
                text: root.status-text;
                wrap: word-wrap;
            }
            Text {
                text: "frame size: " + root.size-label;
            }
            HorizontalLayout {
                spacing: 8px;
                TouchArea {
                    clicked => { root.size-a(); }
                    Text { text: "[64×48]"; color: #2266aa; }
                }
                TouchArea {
                    clicked => { root.size-b(); }
                    Text { text: "[65×3]"; color: #2266aa; }
                }
                TouchArea {
                    clicked => { root.size-c(); }
                    Text { text: "[128×72]"; color: #2266aa; }
                }
                TouchArea {
                    clicked => { root.cycle-size(); }
                    Text { text: "[cycle]"; color: #2266aa; }
                }
            }
            Image {
                source: root.frame-image;
                image-fit: contain;
            }
        }
    }
}

const SIZES: [(u32, u32); 4] = [(64, 48), (65, 3), (63, 2), (128, 72)];

struct ProbeState {
    owner: PresentationOwner,
    evidence: Option<SetImageEvidence>,
    paint_marked: bool,
    /// Last size applied via application resize.
    applied: (u32, u32),
    /// Pending size requested by UI controls / auto-cycle.
    want: (u32, u32),
    cycle_idx: usize,
    last_auto: Instant,
    awaiting_generation: Option<u64>,
    device_label: String,
}

fn apply_want(st: &mut ProbeState) {
    let (w, h) = st.want;
    if (w, h) == st.applied && st.awaiting_generation.is_none() {
        return;
    }
    if st.owner.resize(w, h).is_err() {
        return;
    }
    st.applied = (w, h);
    let generation = st.owner.size_generation();
    st.awaiting_generation = Some(generation);
    let _ = st.owner.request_frame();
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
        paint_marked: false,
        applied: (64, 48),
        want: (64, 48),
        cycle_idx: 0,
        last_auto: Instant::now(),
        awaiting_generation: None,
        device_label: String::new(),
    }));

    {
        let st = Rc::clone(&state);
        ui.on_size_a(move || {
            st.borrow_mut().want = (64, 48);
        });
        let st = Rc::clone(&state);
        ui.on_size_b(move || {
            st.borrow_mut().want = (65, 3);
        });
        let st = Rc::clone(&state);
        ui.on_size_c(move || {
            st.borrow_mut().want = (128, 72);
        });
        let st = Rc::clone(&state);
        ui.on_cycle_size(move || {
            let mut s = st.borrow_mut();
            s.cycle_idx = (s.cycle_idx + 1) % SIZES.len();
            s.want = SIZES[s.cycle_idx];
        });
    }

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
                if st.device_label.is_empty() {
                    st.device_label = device_label;
                }
                // Auto-cycle sizes every 2.5s so CUA sees frame stream without
                // relying only on window stretch of a fixed buffer.
                if st.last_auto.elapsed() >= Duration::from_millis(2500) {
                    st.cycle_idx = (st.cycle_idx + 1) % SIZES.len();
                    st.want = SIZES[st.cycle_idx];
                    st.last_auto = Instant::now();
                }
                apply_want(&mut st);

                if let Some(d) = st.owner.take_latest() {
                    let gen_ok = st
                        .awaiting_generation
                        .is_none_or(|g| d.frame.size_generation >= g);
                    if !gen_ok {
                        return;
                    }
                    st.awaiting_generation = None;
                    match image_from_presented_frame(&d.frame) {
                        Ok((image, mut evidence)) => {
                            ui.set_frame_image(image);
                            mark_property_assigned(&mut evidence);
                            ui.set_size_label(
                                format!(
                                    "{}×{} generation={}",
                                    d.frame.width, d.frame.height, d.frame.size_generation
                                )
                                .into(),
                            );
                            st.paint_marked = false;
                            ui.set_status_text(
                                format!(
                                    "{}\n{}\ndevice={}\nproduce_to_take={:?}\n\
                                     want={:?} applied={:?} generation={}\n\
                                     software_paint=unverified until AfterRendering\n\
                                     claims_wgpu_surface_present=false",
                                    evidence.status,
                                    format_copies(&evidence.copies),
                                    st.device_label,
                                    d.produce_to_take,
                                    st.want,
                                    st.applied,
                                    d.frame.size_generation,
                                )
                                .into(),
                            );
                            st.evidence = Some(evidence);
                        }
                        Err(e) => {
                            ui.set_status_text(format!("image build failed: {e}").into());
                        }
                    }
                } else if st.awaiting_generation.is_some() {
                    let _ = st.owner.request_frame();
                    ui.set_status_text(
                        format!(
                            "Ready; waiting {}×{} generation≥{:?}… device={} stats={:?}",
                            st.want.0,
                            st.want.1,
                            st.awaiting_generation,
                            st.device_label,
                            st.owner.stats()
                        )
                        .into(),
                    );
                }
            }
        }
    });

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
