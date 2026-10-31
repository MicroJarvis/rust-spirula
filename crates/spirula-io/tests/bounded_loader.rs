use spirula_core::{CancelReason, CancelToken, ErrorCode};
use spirula_io::{
    BoundedDatasetLoader, BoundedLoaderConfig, ColmapLoadConfig, SamplerState, load_colmap_dataset,
};
use std::path::PathBuf;

fn tiny() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/datasets/tiny-colmap")
}
fn dataset() -> spirula_io::ParsedDataset {
    load_colmap_dataset(&tiny(), &ColmapLoadConfig::default(), None).unwrap()
}

#[test]
fn deterministic_state_roundtrip_and_decode() {
    let ds = dataset();
    let mut sampler = SamplerState::from_dataset(&ds, 9).unwrap();
    let json = sampler.to_json();
    let mut rebuilt = SamplerState::from_json(&json).unwrap();
    assert_eq!(sampler, rebuilt);
    let loader = BoundedDatasetLoader::new(
        ds,
        BoundedLoaderConfig {
            worker_count: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let frame = loader.next(&mut sampler, None).unwrap();
    assert_eq!(frame.rgb.len(), (frame.width * frame.height * 3) as usize);
    let again = loader.next(&mut rebuilt, None).unwrap();
    assert_eq!(frame.frame_index, again.frame_index);
    assert!(loader.metrics().cache_hits >= 1);
}

#[test]
fn over_budget_and_cancel_are_typed() {
    let ds = dataset();
    let err = BoundedDatasetLoader::new(
        ds.clone(),
        BoundedLoaderConfig {
            cache_byte_cap: 1,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
    let loader = BoundedDatasetLoader::new(ds, Default::default()).unwrap();
    let token = CancelToken::new();
    token.request(CancelReason::User);
    let sampler = SamplerState::from_dataset(loader.dataset(), 1).unwrap();
    let err = loader.prefetch(&sampler, 1, Some(&token)).unwrap_err();
    assert_eq!(err.code(), ErrorCode::Canceled);
}

#[test]
fn independent_cache_and_inflight_accounts_and_rebuild() {
    let mut ds = dataset();
    let mut second = ds.frames[0].clone();
    second.image_id = 2;
    ds.frames.push(second);
    ds.split.train = vec![0, 1];
    let frame_bytes = (ds.frames[0].width * ds.frames[0].height * 3) as u64;
    let loader = BoundedDatasetLoader::new(
        ds,
        BoundedLoaderConfig {
            cache_byte_cap: frame_bytes,
            in_flight_decode_byte_cap: frame_bytes * 2,
            prefetch_queue_cap: 2,
            worker_count: 2,
            seed: 7,
        },
    )
    .unwrap();
    assert_eq!(loader.metrics().decode_worker_count, 2);
    let sampler = loader.new_sampler().unwrap();
    assert_eq!(loader.prefetch(&sampler, 2, None).unwrap(), 2);
    loader.drain_prefetch(None).unwrap();
    let metrics = loader.metrics();
    assert!(metrics.cache_bytes <= frame_bytes);
    assert!(metrics.in_flight_bytes == 0);
    assert!(metrics.queue_count == 0);
    assert!(metrics.cache_entries <= 1);
    let mut rebuilt = SamplerState::from_json(&sampler.to_json()).unwrap();
    loader.rebuild_prefetch(&rebuilt, 2, None).unwrap();
    loader.drain_prefetch(None).unwrap();
    assert_eq!(rebuilt, sampler);
    let _ = loader.next(&mut rebuilt, None).unwrap();
}

#[test]
fn worker_failure_is_single_result_and_retry_recovers() {
    let mut ds = dataset();
    let h = spirula_io::TempHarness::new().unwrap();
    let bad = h.path().join("recover.png");
    std::fs::write(&bad, b"not an image").unwrap();
    ds.frames[0].path = bad.clone();
    let fixture_image = tiny().join("images/tiny.png");
    let loader = BoundedDatasetLoader::new(
        ds,
        BoundedLoaderConfig {
            worker_count: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let mut sampler = loader.new_sampler().unwrap();
    let err = loader.next(&mut sampler, None).unwrap_err();
    assert_eq!(err.code(), ErrorCode::DecodeFailed);
    assert_eq!(sampler.cursor, 0, "failed decode must not advance sampler");
    std::fs::copy(fixture_image, &bad).unwrap();
    assert!(loader.retry_failed(0).unwrap());
    let frame = loader.next(&mut sampler, None).unwrap();
    assert_eq!(frame.frame_index, 0);
    assert_eq!(sampler.cursor, 1);
}

#[test]
fn cancel_drain_and_malformed_state_are_stable() {
    let ds = dataset();
    let loader = BoundedDatasetLoader::new(
        ds,
        BoundedLoaderConfig {
            worker_count: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let sampler = loader.new_sampler().unwrap();
    let cancel = CancelToken::new();
    cancel.request(CancelReason::Shutdown);
    assert_eq!(
        loader
            .prefetch(&sampler, 1, Some(&cancel))
            .unwrap_err()
            .code(),
        ErrorCode::Canceled
    );
    let mut malformed = SamplerState {
        version: 1,
        seed: 0,
        epoch: 0,
        cursor: 0,
        frame_indices: vec![usize::MAX],
    };
    assert_eq!(
        loader.next(&mut malformed, None).unwrap_err().code(),
        ErrorCode::ValueOutOfRange
    );
}

#[test]
fn loader_cancel_drains_pending_accounting() {
    let mut ds = dataset();
    let mut second = ds.frames[0].clone();
    second.image_id = 2;
    ds.frames.push(second);
    ds.split.train = vec![0, 1];
    let loader = BoundedDatasetLoader::new(
        ds,
        BoundedLoaderConfig {
            worker_count: 1,
            prefetch_queue_cap: 2,
            ..Default::default()
        },
    )
    .unwrap();
    let sampler = loader.new_sampler().unwrap();
    let _ = loader.prefetch(&sampler, 2, None);
    loader.cancel_token().request(CancelReason::Shutdown);
    loader.drain_prefetch(None).unwrap();
    let metrics = loader.metrics();
    assert_eq!(metrics.queue_count, 0);
    assert_eq!(metrics.in_flight_bytes, 0);
}
