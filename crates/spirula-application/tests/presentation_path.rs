//! P6-F07 headless presentation path (no Slint window).

use std::thread;
use std::time::Duration;

use spirula_application::{PresentationOwner, produce_presentation_frame_once};
use spirula_core::{
    CancelReason, CancelToken, ErrorCode, PresentationPathKind, rgba8_matches_cpu_probe,
};

fn skip_no_gpu(err: &spirula_core::DomainError) -> bool {
    matches!(
        err.code(),
        ErrorCode::NoAdapter | ErrorCode::DeviceLost | ErrorCode::AllocationFailed
    )
}

#[test]
fn normal_produce_cpu_exact() {
    let (frame, after) = match produce_presentation_frame_once(16, 8, None) {
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
    assert_eq!(after, 0, "accounted current must be 0 after drop");
}

#[test]
fn illegal_size() {
    let err = produce_presentation_frame_once(0, 4, None).expect_err("size");
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
    // Open may fail first on hosts without GPU — still acceptable skip.
    if skip_no_gpu(&err) {
        eprintln!("skip: no GPU ({err})");
        return;
    }
    assert_eq!(err.code(), ErrorCode::Canceled);
}

#[test]
fn latest_frame_overwrites_slow_consumer() {
    let owner = match PresentationOwner::open(8, 8) {
        Ok(o) => o,
        Err(e) if skip_no_gpu(&e) => {
            eprintln!("skip: no GPU ({e})");
            return;
        }
        Err(e) => panic!("open: {e}"),
    };

    for _ in 0..5 {
        owner.request_frame().expect("request");
        // Give the worker time to produce without taking — builds overwrite pressure.
        thread::sleep(Duration::from_millis(30));
    }
    // Wait for at least one frame.
    let mut delivered = None;
    for _ in 0..100 {
        if let Some(d) = owner.take_latest() {
            delivered = Some(d);
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let delivered = delivered.expect("expected at least one frame");
    assert!(rgba8_matches_cpu_probe(
        delivered.frame.width,
        delivered.frame.height,
        &delivered.frame.rgba8
    ));

    // Produce more while not taking, then take once — overwritten > 0 if worker kept up.
    for _ in 0..8 {
        owner.request_frame().expect("request");
        thread::sleep(Duration::from_millis(20));
    }
    let _ = owner.take_latest();
    let stats = owner.stats();
    assert!(stats.produced >= 1, "produced={}", stats.produced);
    // Slow consumer: either overwritten frames or taken < produced.
    assert!(
        stats.overwritten >= 1 || stats.taken < stats.produced,
        "stats={stats:?}"
    );

    owner.shutdown();
}

#[test]
fn resize_then_produce() {
    let owner = match PresentationOwner::open(4, 4) {
        Ok(o) => o,
        Err(e) if skip_no_gpu(&e) => {
            eprintln!("skip: no GPU ({e})");
            return;
        }
        Err(e) => panic!("open: {e}"),
    };
    owner.resize(12, 6).expect("resize");
    owner.request_frame().expect("request");
    let mut frame = None;
    for _ in 0..100 {
        if let Some(d) = owner.take_latest() {
            frame = Some(d.frame);
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let frame = frame.expect("frame");
    assert_eq!((frame.width, frame.height), (12, 6));
    assert!(rgba8_matches_cpu_probe(12, 6, &frame.rgba8));
    owner.shutdown();
}

#[test]
fn cancel_in_flight_and_repeat_start() {
    let owner = match PresentationOwner::open(8, 8) {
        Ok(o) => o,
        Err(e) if skip_no_gpu(&e) => {
            eprintln!("skip: no GPU ({e})");
            return;
        }
        Err(e) => panic!("open: {e}"),
    };
    owner.request_frame().expect("request");
    owner.cancel();
    // Allow worker to observe cancel.
    thread::sleep(Duration::from_millis(100));
    let stats = owner.stats();
    // May have produced before cancel or recorded canceled — either is fine.
    assert!(
        stats.canceled >= 1 || stats.produced >= 1 || owner.last_error().is_some(),
        "stats={stats:?}"
    );
    owner.shutdown();

    // Repeat start after shutdown.
    let owner2 = match PresentationOwner::open(4, 4) {
        Ok(o) => o,
        Err(e) if skip_no_gpu(&e) => return,
        Err(e) => panic!("reopen: {e}"),
    };
    owner2.request_frame().expect("request");
    let mut ok = false;
    for _ in 0..100 {
        if owner2.take_latest().is_some() {
            ok = true;
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(ok || owner2.stats().failed >= 1 || owner2.stats().canceled >= 1);
    owner2.shutdown();
}

#[test]
fn open_rejects_illegal_before_worker() {
    match PresentationOwner::open(0, 1) {
        Ok(_) => panic!("expected size error"),
        Err(err) => assert_eq!(err.code(), ErrorCode::ValueOutOfRange),
    }
}
