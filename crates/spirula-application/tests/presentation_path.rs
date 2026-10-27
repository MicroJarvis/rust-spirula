//! P6-F07 headless presentation path (deterministic barriers; no Slint window).

use std::time::Duration;

use spirula_application::{PresentationOwner, PresentationStatus, produce_presentation_frame_once};
use spirula_core::{
    CancelReason, CancelToken, ErrorCode, PresentationPathKind, cpu_probe_corners, rgba8_corners,
    rgba8_matches_cpu_probe,
};
use spirula_gpu::{COPY_BYTES_PER_ROW_ALIGNMENT, rgba8_bytes_per_row, rgba8_padded_bytes};

fn skip_no_gpu(err: &spirula_core::DomainError) -> bool {
    matches!(
        err.code(),
        ErrorCode::NoAdapter | ErrorCode::DeviceLost | ErrorCode::AllocationFailed
    )
}

fn ready_owner(w: u32, h: u32) -> Option<PresentationOwner> {
    let owner = match PresentationOwner::start(w, h) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("skip: start failed ({e})");
            return None;
        }
    };
    match owner.wait_ready(Duration::from_secs(30)) {
        Ok(()) => Some(owner),
        Err(e) if skip_no_gpu(&e) => {
            eprintln!("skip: no GPU ({e})");
            owner.shutdown();
            None
        }
        Err(e) => panic!("wait_ready: {e}"),
    }
}

#[test]
fn normal_produce_cpu_exact() {
    let (frame, after, retained) = match produce_presentation_frame_once(16, 8, None) {
        Ok(v) => v,
        Err(e) if skip_no_gpu(&e) => {
            eprintln!("skip: no GPU ({e})");
            return;
        }
        Err(e) => panic!("unexpected: {e}"),
    };
    assert_eq!(frame.width, 16);
    assert_eq!(frame.height, 8);
    assert!(rgba8_matches_cpu_probe(16, 8, &frame.rgba8));
    assert_eq!(frame.path, PresentationPathKind::ReadbackToHost);
    assert!(frame.copies.sync_points.contains(&"submit"));
    assert!(frame.copies.sync_points.contains(&"wait"));
    assert!(frame.copies.sync_points.contains(&"map"));
    assert_eq!(frame.copies.logical_rgba_bytes, 16 * 8 * 4);
    assert!(frame.copies.unavailable.contains(&"wgpu_surface_present"));
    assert_eq!(after, 0, "current_bytes must be exactly 0");
    assert_eq!(retained, 0, "retained must be exactly 0");
}

#[test]
fn pitch_corners_1x1_63x2_65x3() {
    for (w, h) in [(1u32, 1u32), (63, 2), (65, 3)] {
        let (frame, after, retained) = match produce_presentation_frame_once(w, h, None) {
            Ok(v) => v,
            Err(e) if skip_no_gpu(&e) => {
                eprintln!("skip: no GPU ({e})");
                return;
            }
            Err(e) => panic!("unexpected: {e}"),
        };
        let bpr = rgba8_bytes_per_row(w).unwrap();
        let padded = rgba8_padded_bytes(w, h).unwrap();
        assert_eq!(bpr % COPY_BYTES_PER_ROW_ALIGNMENT, 0);
        assert_eq!(frame.copies.readback_bytes_per_row, Some(bpr));
        assert_eq!(frame.copies.readback_staging_bytes, Some(padded));
        assert_eq!(frame.rgba8.len(), (w * h * 4) as usize);
        let expected = cpu_probe_corners(w, h).unwrap();
        assert_eq!(rgba8_corners(w, h, &frame.rgba8), Some(expected));
        assert_eq!(after, 0);
        assert_eq!(retained, 0);
    }
}

#[test]
fn illegal_size() {
    let err = produce_presentation_frame_once(0, 4, None).expect_err("size");
    assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
    let err = produce_presentation_frame_once(4097, 1, None).expect_err("oversize");
    assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
}

#[test]
fn cancel_before_produce() {
    let token = CancelToken::new();
    token.request(CancelReason::User);
    let err = match produce_presentation_frame_once(8, 8, Some(&token)) {
        Err(e) => e,
        Ok(_) => panic!("expected cancel"),
    };
    if skip_no_gpu(&err) {
        eprintln!("skip: no GPU ({err})");
        return;
    }
    assert_eq!(err.code(), ErrorCode::Canceled);
}

#[test]
fn resize_clears_cached_slot_generation_gate() {
    let Some(owner) = ready_owner(4, 4) else {
        return;
    };
    let d = owner
        .request_and_wait_frame(Duration::from_secs(30))
        .expect("4x4 frame");
    assert_eq!((d.frame.width, d.frame.height), (4, 4));
    // Resize must clear slot; cached 4x4 must not be takeable as 65x3.
    owner.resize(65, 3).expect("resize");
    assert!(
        owner.take_latest().is_none(),
        "cached prior-generation frame must not be taken after resize"
    );
    let d2 = owner
        .request_and_wait_frame(Duration::from_secs(30))
        .expect("65x3 frame");
    assert_eq!((d2.frame.width, d2.frame.height), (65, 3));
    assert_eq!(d2.frame.size_generation, owner.size_generation());
    owner.shutdown();
}

#[test]
fn resize_burst_latest_wins_no_stale_cmd_replay() {
    // Metal counterexample: queued Resize cmds + full channel lost the final
    // 65x3 and replayed 23x23. Resize is shared-state only (never queued).
    let Some(owner) = ready_owner(1024, 1024) else {
        return;
    };
    let _ = owner.request_and_wait_frame(Duration::from_secs(60));
    for i in 0..1000u32 {
        owner
            .resize(20 + (i % 7), 20 + (i % 5))
            .expect("burst resize");
    }
    owner.resize(65, 3).expect("final");
    assert_eq!(owner.size(), (65, 3));
    let d = owner
        .request_and_wait_frame(Duration::from_secs(60))
        .expect("after burst");
    assert_eq!(
        (d.frame.width, d.frame.height),
        (65, 3),
        "stale resize must not roll back shared size"
    );
    owner.shutdown();
}

#[test]
fn stall_slow_consumer_and_coalesce_stop() {
    let Some(owner) = ready_owner(8, 8) else {
        return;
    };
    // Deterministic slow consumer: request → wait_epoch, never take.
    const CYCLES: u64 = 6;
    for _ in 0..CYCLES {
        let before = owner.produce_epoch();
        owner.request_frame().expect("request");
        owner
            .wait_produce_epoch(before, Duration::from_secs(30))
            .expect("epoch");
    }
    let stats = owner.stats();
    assert!(
        stats.produced >= 1,
        "expected produces under stall, stats={stats:?}"
    );
    assert!(
        stats.overwritten >= CYCLES.saturating_sub(1) || stats.produced >= CYCLES,
        "latest-slot should overwrite while untaken, stats={stats:?}"
    );
    // Slot holds at most one current frame (byte ceiling).
    if let Some(d) = owner.take_latest() {
        let ceiling = (d.frame.width as u64) * (d.frame.height as u64) * 4;
        assert_eq!(d.frame.rgba8.len() as u64, ceiling);
    }

    // 1000 non-blocking requests coalesce on capacity-1; must not block caller.
    let t_req = std::time::Instant::now();
    for _ in 0..1000 {
        owner.request_frame().expect("coalesce request");
    }
    assert!(
        t_req.elapsed() < Duration::from_secs(1),
        "1000 request_frame calls blocked"
    );

    let t0 = std::time::Instant::now();
    owner.cancel();
    owner.shutdown();
    assert!(
        t0.elapsed() < Duration::from_secs(5),
        "shutdown blocked under stall"
    );
}

#[test]
fn close_no_resurrect() {
    let Some(mut owner) = ready_owner(4, 4) else {
        return;
    };
    owner.close();
    assert!(matches!(owner.status(), PresentationStatus::Closed));
    assert!(owner.request_frame().is_err(), "no resurrect after close");
    assert!(owner.resize(8, 8).is_err(), "no resize after close");
    assert!(owner.take_latest().is_none());
    // Fresh start is allowed (new owner), not resurrect.
    if let Some(o2) = ready_owner(4, 4) {
        o2.shutdown();
    }
}

#[test]
fn open_rejects_illegal_before_worker() {
    match PresentationOwner::start(0, 1) {
        Ok(_) => panic!("expected size error"),
        Err(err) => assert_eq!(err.code(), ErrorCode::ValueOutOfRange),
    }
}

#[test]
fn worker_opens_gpu_status_opening_then_ready() {
    let owner = match PresentationOwner::start(8, 8) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("skip: {e}");
            return;
        }
    };
    // May already be Ready if scheduling is fast; Opening is allowed initially.
    match owner.status() {
        PresentationStatus::Opening | PresentationStatus::Ready { .. } => {}
        PresentationStatus::Failed { code, message } => {
            if matches!(
                code,
                ErrorCode::NoAdapter | ErrorCode::DeviceLost | ErrorCode::AllocationFailed
            ) {
                eprintln!("skip: {message}");
                owner.shutdown();
                return;
            }
            panic!("unexpected fail: {code:?} {message}");
        }
        PresentationStatus::Closed => panic!("closed too early"),
    }
    owner.wait_ready(Duration::from_secs(30)).expect("ready");
    assert!(matches!(owner.status(), PresentationStatus::Ready { .. }));
    owner.shutdown();
}
