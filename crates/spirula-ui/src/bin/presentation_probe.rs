//! Local CUA window probe for P6-F07 (software renderer).
//!
//! GPU opens on the presentation worker. UI never blocks waiting for GPU.
//! Cycles application resize → request → Slint display so CUA can verify
//! different size frames (not only stretching a fixed 64×48). Generation
//! gates stay in force. Does **not** claim wgpu surface present.
//!
//! Close path: callbacks / timer / paint notifier hold `Weak` refs; owner is
//! `take()`n after `run` returns so shutdown always completes (exit 0).

use std::cell::RefCell;
use std::process::ExitCode;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, RenderingState, Timer, TimerMode};
use spirula_application::PresentationStatus;
use spirula_ui::{
    ProbeDriver, SetImageEvidence, drive_owner, image_from_presented_frame, mark_property_assigned,
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
    owner: Option<spirula_application::PresentationOwner>,
    driver: ProbeDriver,
    evidence: Option<SetImageEvidence>,
    paint_marked: bool,
    cycle_idx: usize,
    last_auto: Instant,
    device_label: String,
}

fn main() -> ExitCode {
    match run_probe() {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => code,
    }
}

fn run_probe() -> Result<(), ExitCode> {
    let ui = PresentationProbe::new().map_err(|e| {
        eprintln!("probe: slint UI init failed: {e:?}");
        ExitCode::FAILURE
    })?;
    ui.set_status_text("starting presentation worker (GPU opens off UI thread)…".into());

    let owner = match spirula_application::PresentationOwner::start(64, 48) {
        Ok(o) => o,
        Err(e) => {
            ui.set_status_text(format!("start failed: {e}").into());
            let _ = ui.run();
            return Ok(());
        }
    };

    let state = Rc::new(RefCell::new(ProbeState {
        owner: Some(owner),
        driver: ProbeDriver::new(64, 48),
        evidence: None,
        paint_marked: false,
        cycle_idx: 0,
        last_auto: Instant::now(),
        device_label: String::new(),
    }));

    // Weak callbacks: window/close must not keep ProbeState alive past cleanup.
    {
        let weak = Rc::downgrade(&state);
        ui.on_size_a(move || set_want(&weak, 64, 48));
        let weak = Rc::downgrade(&state);
        ui.on_size_b(move || set_want(&weak, 65, 3));
        let weak = Rc::downgrade(&state);
        ui.on_size_c(move || set_want(&weak, 128, 72));
        let weak = Rc::downgrade(&state);
        ui.on_cycle_size(move || {
            let Some(st) = weak.upgrade() else {
                return;
            };
            let mut s = st.borrow_mut();
            s.cycle_idx = (s.cycle_idx + 1) % SIZES.len();
            let (w, h) = SIZES[s.cycle_idx];
            s.driver.set_want(w, h);
        });
    }

    let state_paint = Rc::downgrade(&state);
    let ui_paint = ui.as_weak();
    match ui
        .window()
        .set_rendering_notifier(move |rendering_state, _api| {
            match rendering_state {
                RenderingState::AfterRendering => {}
                _ => return,
            }
            let Some(st_rc) = state_paint.upgrade() else {
                return;
            };
            let mut st = st_rc.borrow_mut();
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
    let state_tick = Rc::downgrade(&state);
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(16), move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let Some(st_rc) = state_tick.upgrade() else {
            return;
        };
        let mut st = st_rc.borrow_mut();
        let ProbeState {
            owner,
            driver,
            evidence,
            paint_marked,
            cycle_idx,
            last_auto,
            device_label,
        } = &mut *st;
        let Some(owner) = owner.as_ref() else {
            return;
        };
        match owner.status() {
            PresentationStatus::Opening => {
                ui.set_status_text("Opening GPU on presentation worker…".into());
            }
            PresentationStatus::Failed { code, message } => {
                ui.set_status_text(format!("GPU open failed: {code:?}: {message}").into());
            }
            PresentationStatus::Closed => {
                ui.set_status_text("presentation closed".into());
            }
            PresentationStatus::Ready { device_label: label } => {
                if device_label.is_empty() {
                    *device_label = label;
                }
                // Auto-cycle sizes every 2.5s after first assign so CUA can see
                // a multi-size stream without only stretching a fixed buffer.
                if driver.assigned_count >= 1 && last_auto.elapsed() >= Duration::from_millis(2500)
                {
                    *cycle_idx = (*cycle_idx + 1) % SIZES.len();
                    let (w, h) = SIZES[*cycle_idx];
                    driver.set_want(w, h);
                    *last_auto = Instant::now();
                }

                if let Err(e) = drive_owner(driver, owner) {
                    ui.set_status_text(format!("drive failed: {e}").into());
                    return;
                }

                if let Some(d) = owner.take_latest() {
                    let assigned = driver.try_assign(
                        d.frame.size_generation,
                        d.frame.width,
                        d.frame.height,
                    );
                    if !assigned {
                        return;
                    }
                    match image_from_presented_frame(&d.frame) {
                        Ok((image, mut set_evidence)) => {
                            ui.set_frame_image(image);
                            mark_property_assigned(&mut set_evidence);
                            ui.set_size_label(
                                format!(
                                    "{}×{} generation={}",
                                    d.frame.width, d.frame.height, d.frame.size_generation
                                )
                                .into(),
                            );
                            *paint_marked = false;
                            ui.set_status_text(
                                format!(
                                    "{}\n{}\ndevice={}\nproduce_to_take={:?}\n\
                                     want={:?} applied={:?} awaiting={:?} assigned={}\n\
                                     stats={:?}\n\
                                     software_paint=unverified until AfterRendering\n\
                                     claims_wgpu_surface_present=false",
                                    set_evidence.status,
                                    format_copies(&set_evidence.copies),
                                    device_label,
                                    d.produce_to_take,
                                    driver.want,
                                    driver.applied,
                                    driver.awaiting_generation,
                                    driver.assigned_count,
                                    owner.stats(),
                                )
                                .into(),
                            );
                            *evidence = Some(set_evidence);
                        }
                        Err(e) => {
                            ui.set_status_text(format!("image build failed: {e}").into());
                        }
                    }
                } else if driver.awaiting_generation.is_some() {
                    ui.set_status_text(
                        format!(
                            "Ready; waiting {}×{} generation≥{:?}… device={} stats={:?} assigned={}",
                            driver.want.0,
                            driver.want.1,
                            driver.awaiting_generation,
                            device_label,
                            owner.stats(),
                            driver.assigned_count,
                        )
                        .into(),
                    );
                }
            }
        }
    });

    let run_res = ui.run();
    // Stop timer before releasing state (drops tick Strong/Weak upgrades).
    drop(timer);
    // Clear Slint callbacks so they drop any residual upgrades promptly.
    ui.on_size_a(|| {});
    ui.on_size_b(|| {});
    ui.on_size_c(|| {});
    ui.on_cycle_size(|| {});

    let owner = state.borrow_mut().owner.take();
    // Drop UI before shutdown join so paint notifier cannot race.
    drop(ui);
    if let Some(owner) = owner {
        owner.shutdown();
    }
    // Strong Rc should be unique now; ignore leftover Weak-only holds.
    drop(state);

    if let Err(e) = run_res {
        eprintln!("probe: ui.run ended with error: {e:?}");
        return Err(ExitCode::FAILURE);
    }
    eprintln!("probe: clean shutdown (owner released)");
    Ok(())
}

fn set_want(weak: &Weak<RefCell<ProbeState>>, w: u32, h: u32) {
    let Some(st) = weak.upgrade() else {
        return;
    };
    st.borrow_mut().driver.set_want(w, h);
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
