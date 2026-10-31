//! P4-F06 application-layer 800×600 stage-wall evidence (not the parent
//! `p4_f06_review.rs` arbitration suite). Reuses the F05 65-source pressure
//! scene (487500 pairs). Required-GPU env fails rather than skips.
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use spirula_application::{
    ApplicationService, Command, CommandPayload, CommandResult, Event, SessionLimits, SubmitOutcome,
};
use spirula_core::{GaussianSplat, OutcomeView, Quat, SplatSet, Vec3};
use spirula_gpu::CapabilityRequest;
use spirula_io::write_ply_splats;

fn require_host() -> bool {
    match spirula_gpu::GpuContext::open(&CapabilityRequest::diagnose_default()) {
        Ok(_ctx) => true,
        Err(err) => {
            assert!(
                std::env::var_os("SPIRULA_REQUIRE_RENDER_GPU").is_none(),
                "required GPU unavailable: {err:?}"
            );
            eprintln!("p4_f06 application metrics GPU test skipped: {err:?}");
            false
        }
    }
}

fn pressure_splats() -> SplatSet {
    let splats = (0..65)
        .map(|i| {
            GaussianSplat::try_new(
                Vec3::try_new((i % 3) as f64 * 0.001, 0.0, -2.0).unwrap(),
                Quat::try_from_wxyz(0.9238795325, 0.0, 0.0, 0.3826834324).unwrap(),
                Vec3::try_new(100.0_f64.ln(), 100.0_f64.ln(), 100.0_f64.ln()).unwrap(),
                0.0,
                Vec3::try_new(0.0, 0.0, 0.0).unwrap(),
            )
            .unwrap()
        })
        .collect();
    SplatSet::try_from_splats(splats).unwrap()
}

fn write_pressure_fixture(dir: &std::path::Path) -> (PathBuf, PathBuf) {
    let ply = dir.join("scene.ply");
    write_ply_splats(&ply, &pressure_splats(), None).expect("write pressure ply");
    let request = dir.join("request.json");
    // 800×600 Constant-background preview; budgets sized for F05 pressure peak.
    fs::write(
        &request,
        r#"{
  "schema": "spirula-preview-request",
  "version": 1,
  "splat_ply": "scene.ply",
  "camera": {
    "model": "PINHOLE",
    "width": 800,
    "height": 600,
    "fx": 600,
    "fy": 610,
    "cx": 400,
    "cy": 300,
    "c2w": {
      "rotation_row_major": [1, 0, 0, 0, 1, 0, 0, 0, 1],
      "translation": [0, 0, 0]
    }
  },
  "background": [0.1, 0.2, 0.3],
  "thresholds": {
    "alpha_cutoff": 0.01,
    "early_out_transmittance": 0.0001
  },
  "budgets": {
    "max_pixels": 480000,
    "max_host_bytes": 536870912,
    "max_gpu_bytes": 536870912,
    "max_file_bytes": 33554432,
    "max_image_bytes": 12582912
  }
}"#,
    )
    .unwrap();
    (request, dir.join("out.png"))
}

#[test]
fn application_preview_800x600_pressure_reports_stage_walls_and_payload_bytes() {
    if !require_host() {
        return;
    }
    let harness = spirula_io::temp_harness::TempHarness::new().unwrap();
    let (request, out) = write_pressure_fixture(harness.path());
    let app = ApplicationService::start(SessionLimits::for_test());
    let id = match app.submit(Command::new(CommandPayload::Preview {
        request_path: request.display().to_string(),
        output_path: out.display().to_string(),
    })) {
        SubmitOutcome::Accepted { id } => id,
        SubmitOutcome::Rejected { error } => panic!("preview rejected: {error}"),
    };
    assert!(
        app.wait_until(Duration::from_secs(120), |_| {
            matches!(app.command_result(id), CommandResult::Ready(_))
        }),
        "preview terminal timeout"
    );
    let terminal = match app.command_result(id) {
        CommandResult::Ready(env) => env,
        other => panic!("expected Ready, got {other:?}"),
    };
    match &terminal.event {
        Event::Completed {
            outcome: OutcomeView::Completed { .. },
        } => {}
        other => panic!("unexpected terminal {other:?}"),
    }

    let snap = app.snapshot();
    let replay = app
        .replay(spirula_application::EventCursor::new(snap.session_id, 0))
        .expect("replay");
    let spirula_application::ReplayOutcome::ControlEvents(events) = replay else {
        panic!("resync unexpected");
    };
    let published = events.iter().find_map(|e| match &e.event {
        Event::PreviewPublished {
            width,
            height,
            splat_count,
            rgb8_bytes,
            png_bytes,
            wall_load_ns,
            wall_render_ns,
            wall_encode_ns,
            wall_publish_ns,
            ..
        } if e.command_id == Some(id) => Some((
            *width,
            *height,
            *splat_count,
            *rgb8_bytes,
            *png_bytes,
            *wall_load_ns,
            *wall_render_ns,
            *wall_encode_ns,
            *wall_publish_ns,
        )),
        _ => None,
    });
    let (w, h, splats, rgb8, png, load_ns, render_ns, encode_ns, publish_ns) =
        published.expect("PreviewPublished");
    assert_eq!((w, h), (800, 600));
    assert_eq!(splats, 65);
    assert_eq!(rgb8, 800 * 600 * 3);
    assert!(png > 0, "png payload bytes");
    // Host wall-clock stage labels (not GPU timestamps / bus counters).
    assert!(load_ns > 0, "wall_load_ns");
    assert!(render_ns > 0, "wall_render_ns");
    assert!(encode_ns > 0, "wall_encode_ns");
    assert!(publish_ns > 0, "wall_publish_ns");
    assert!(out.is_file(), "PNG published to disk");
    eprintln!(
        "p4_f06_application_metrics 800x600 pressure splats={splats} rgb8_bytes={rgb8} png_bytes={png} wall_load_ns={load_ns} wall_render_ns={render_ns} wall_encode_ns={encode_ns} wall_publish_ns={publish_ns} (payload sizes + wall-clock stages; not RSS/VRAM/bus-moved-bytes)"
    );
}
