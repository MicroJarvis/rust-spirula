//! Synthetic P2-F03: budget, handles, submit/wait/drop accounting (no host GPU).

use spirula_core::ErrorCode;
use spirula_gpu::{
    AdapterCapabilities, AdapterClass, AdapterIdentity, AdapterLimits, BackendKind, BufferDesc,
    BufferUsage, CapabilityRequest, FeatureBits, FieldAvailability, GpuContext, ResourceLabel,
    Texture2dDesc, TextureFormat, TextureUsage, rgba8_bytes_per_row,
};
use std::time::Duration;

fn sample_adapter() -> AdapterCapabilities {
    AdapterCapabilities {
        identity: AdapterIdentity {
            index: 0,
            name: "synthetic".into(),
            backend: BackendKind::Metal,
            class: AdapterClass::DiscreteGpu,
            vendor: 0,
            device: 0,
            driver: "test".into(),
            driver_info: String::new(),
            dedicated_vram_bytes: FieldAvailability::Unavailable {
                reason: "synthetic",
            },
            uuid: FieldAvailability::Unavailable {
                reason: "synthetic",
            },
        },
        features: FeatureBits::default(),
        limits: AdapterLimits {
            max_buffer_size: 1 << 30,
            max_storage_buffers_per_shader_stage: 8,
            max_compute_workgroup_storage_size: 16384,
        },
    }
}

fn open_ctx() -> GpuContext {
    GpuContext::open_synthetic(
        &CapabilityRequest::diagnose_default(),
        sample_adapter(),
        false,
    )
    .expect("synthetic open")
}

#[test]
fn synthetic_buffer_roundtrip_and_budget_zero() {
    let mut ctx = open_ctx();
    let label = ResourceLabel::parse("spirula:gpu:buffer:probe").unwrap();
    let buf = ctx
        .create_buffer(BufferDesc {
            label,
            size: 16,
            usage: BufferUsage::COPY_SRC_DST,
        })
        .unwrap();
    let pattern = (0u8..16).collect::<Vec<_>>();
    ctx.upload_buffer(&buf, 0, &pattern).unwrap();
    let sub = ctx.submit().unwrap();
    ctx.wait(sub, Some(Duration::from_secs(1))).unwrap();
    let ticket = ctx.readback_buffer(&buf, 0, 16).unwrap();
    let sub2 = ctx.submit().unwrap();
    ctx.wait(sub2, Some(Duration::from_secs(1))).unwrap();
    let out = ctx.map_readback(ticket).unwrap();
    assert_eq!(out, pattern);
    drop(buf);
    let acc = ctx.resource_accounting();
    assert_eq!(acc.current_bytes, 0, "all accounted bytes released");
    assert!(acc.peak_bytes >= 16);
    assert_eq!(acc.source, spirula_gpu::RESOURCE_ACCOUNT_SOURCE);
}

#[test]
fn synthetic_texture_non_256_row_pitch_roundtrip() {
    let mut ctx = open_ctx();
    // width=1 → raw row 4 bytes, pitched to 256
    assert_eq!(rgba8_bytes_per_row(1).unwrap(), 256);
    let tex = ctx
        .create_texture_2d(Texture2dDesc {
            label: ResourceLabel::parse("spirula:gpu:texture:probe").unwrap(),
            width: 1,
            height: 2,
            format: TextureFormat::Rgba8Unorm,
            usage: TextureUsage::COPY_SRC_DST,
        })
        .unwrap();
    let pixels = vec![1u8, 2, 3, 4, 5, 6, 7, 8];
    ctx.upload_texture_rgba8(&tex, &pixels).unwrap();
    let sub = ctx.submit().unwrap();
    ctx.wait(sub, None).unwrap();
    let ticket = ctx.readback_texture_rgba8(&tex).unwrap();
    let sub2 = ctx.submit().unwrap();
    ctx.wait(sub2, None).unwrap();
    let out = ctx.map_readback(ticket).unwrap();
    assert_eq!(out, pixels);
    drop(tex);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);
}

#[test]
fn synthetic_budget_preflight_rejects() {
    let mut ctx = open_ctx();
    ctx.set_resource_budget(Some(8));
    let Err(err) = ctx.create_buffer(BufferDesc {
        label: ResourceLabel::parse("spirula:gpu:buffer:big").unwrap(),
        size: 64,
        usage: BufferUsage::COPY_DST,
    }) else {
        panic!("expected budget preflight failure");
    };
    assert_eq!(err.code(), ErrorCode::AllocationFailed);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);
}

#[test]
fn synthetic_illegal_range_and_label() {
    assert!(ResourceLabel::parse("bad").is_err());
    let mut ctx = open_ctx();
    let buf = ctx
        .create_buffer(BufferDesc {
            label: ResourceLabel::parse("spirula:gpu:buffer:r").unwrap(),
            size: 8,
            usage: BufferUsage::COPY_SRC_DST,
        })
        .unwrap();
    let err = ctx.upload_buffer(&buf, 4, &[0u8; 8]).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
}

#[test]
fn synthetic_drop_after_submit_still_waits_then_zeros() {
    let mut ctx = open_ctx();
    let buf = ctx
        .create_buffer(BufferDesc {
            label: ResourceLabel::parse("spirula:gpu:buffer:drop").unwrap(),
            size: 32,
            usage: BufferUsage::COPY_SRC_DST,
        })
        .unwrap();
    ctx.upload_buffer(&buf, 0, &[9u8; 32]).unwrap();
    let sub = ctx.submit().unwrap();
    drop(buf); // destroy pending while in-flight
    ctx.wait(sub, None).unwrap();
    assert_eq!(ctx.resource_accounting().current_bytes, 0);
}

#[test]
fn synthetic_clone_does_not_double_account() {
    let mut ctx = open_ctx();
    let a = ctx
        .create_buffer(BufferDesc {
            label: ResourceLabel::parse("spirula:gpu:buffer:share").unwrap(),
            size: 24,
            usage: BufferUsage::COPY_DST,
        })
        .unwrap();
    let before = ctx.resource_accounting().current_bytes;
    let b = a.clone();
    assert_eq!(ctx.resource_accounting().current_bytes, before);
    drop(a);
    assert_eq!(ctx.resource_accounting().current_bytes, before);
    drop(b);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);
}

#[test]
fn synthetic_cross_context_and_reopen_reject() {
    let mut a = open_ctx();
    let mut b = open_ctx();
    let buf = a
        .create_buffer(BufferDesc {
            label: ResourceLabel::parse("spirula:gpu:buffer:x").unwrap(),
            size: 4,
            usage: BufferUsage::COPY_DST,
        })
        .unwrap();
    let err = b.upload_buffer(&buf, 0, &[1, 2, 3, 4]).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidArgument);

    a.reopen_synthetic(
        &CapabilityRequest::diagnose_default(),
        sample_adapter(),
        false,
    )
    .unwrap();
    let err = a.upload_buffer(&buf, 0, &[1, 2, 3, 4]).unwrap_err();
    assert_eq!(err.code(), ErrorCode::DeviceLost);
}

#[test]
fn synthetic_wait_timeout_retains_budget_then_retry_completes() {
    let mut ctx = open_ctx();
    let buf = ctx
        .create_buffer(BufferDesc {
            label: ResourceLabel::parse("spirula:gpu:buffer:timeout").unwrap(),
            size: 16,
            usage: BufferUsage::COPY_SRC_DST,
        })
        .unwrap();
    ctx.upload_buffer(&buf, 0, &[1u8; 16]).unwrap();
    let sub = ctx.submit().unwrap();
    let before = ctx.resource_accounting().current_bytes;
    assert!(
        before > 0,
        "staging+buffer should be accounted after submit"
    );

    ctx.inject_next_wait_timeout();
    let err = ctx.wait(sub, Some(Duration::from_millis(1))).unwrap_err();
    assert_eq!(err.code(), ErrorCode::SubmitFailed);
    assert_eq!(
        ctx.resource_accounting().current_bytes,
        before,
        "timeout must not free in-flight staging/resources"
    );

    // Later wait completes and upload staging is reclaimed.
    ctx.wait(sub, None).unwrap();
    drop(buf);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);
}

#[test]
fn synthetic_map_deadline_retains_staging_for_retry() {
    let mut ctx = open_ctx();
    let buf = ctx
        .create_buffer(BufferDesc {
            label: ResourceLabel::parse("spirula:gpu:buffer:map-deadline").unwrap(),
            size: 8,
            usage: BufferUsage::COPY_SRC_DST,
        })
        .unwrap();
    ctx.upload_buffer(&buf, 0, &[2u8; 8]).unwrap();
    let s = ctx.submit().unwrap();
    ctx.wait(s, None).unwrap();
    let ticket = ctx.readback_buffer(&buf, 0, 8).unwrap();
    let s2 = ctx.submit().unwrap();
    ctx.wait(s2, None).unwrap();
    let before = ctx.resource_accounting().current_bytes;

    ctx.inject_next_map_deadline();
    let err = ctx.map_readback(ticket.clone()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::SubmitFailed);
    assert_eq!(
        ctx.resource_accounting().current_bytes,
        before,
        "map deadline must not free staging after GPU-ready"
    );

    // Same ticket retries after deadline; GPU work was already confirmed complete.
    let out = ctx.map_readback(ticket).unwrap();
    assert_eq!(out, vec![2u8; 8]);
    drop(buf);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);
}

#[test]
fn synthetic_repeat_cycles_no_budget_leak() {
    let mut ctx = open_ctx();
    ctx.set_resource_budget(Some(1024));
    for _ in 0..8 {
        let buf = ctx
            .create_buffer(BufferDesc {
                label: ResourceLabel::parse("spirula:gpu:buffer:cycle").unwrap(),
                size: 64,
                usage: BufferUsage::COPY_SRC_DST,
            })
            .unwrap();
        ctx.upload_buffer(&buf, 0, &[7u8; 64]).unwrap();
        let s = ctx.submit().unwrap();
        ctx.wait(s, None).unwrap();
        let t = ctx.readback_buffer(&buf, 0, 64).unwrap();
        let s2 = ctx.submit().unwrap();
        ctx.wait(s2, None).unwrap();
        let _ = ctx.map_readback(t).unwrap();
        drop(buf);
        assert_eq!(ctx.resource_accounting().current_bytes, 0);
    }
}
