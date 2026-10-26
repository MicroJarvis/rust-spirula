//! Host headless buffer + non-256-aligned-width texture round-trip (P2-F03).

use spirula_core::ErrorCode;
use spirula_gpu::{
    BufferDesc, BufferUsage, CapabilityRequest, GpuContext, OpenFailureStage, ResourceLabel,
    Texture2dDesc, TextureFormat, TextureUsage, WGPU_VERSION, rgba8_bytes_per_row,
};
use std::time::{Duration, Instant};

#[test]
fn host_buffer_and_texture_roundtrip_or_precise_unavailable() {
    let request = CapabilityRequest::diagnose_default();
    match GpuContext::open(&request) {
        Ok(mut ctx) => {
            let t0 = Instant::now();
            let buf = ctx
                .create_buffer(BufferDesc {
                    label: ResourceLabel::parse("spirula:gpu:buffer:host-rt").unwrap(),
                    size: 64,
                    usage: BufferUsage::COPY_SRC_DST,
                })
                .expect("create buffer");
            let pattern: Vec<u8> = (0..64).map(|i| (i * 3) as u8).collect();
            ctx.upload_buffer(&buf, 0, &pattern).unwrap();
            let sub = ctx.submit().unwrap();
            ctx.wait(sub, Some(Duration::from_secs(5))).unwrap();
            let ticket = ctx.readback_buffer(&buf, 0, 64).unwrap();
            let sub2 = ctx.submit().unwrap();
            ctx.wait(sub2, Some(Duration::from_secs(5))).unwrap();
            let out = ctx.map_readback(&ticket).unwrap();
            assert_eq!(out, pattern);
            let upload_ns = t0.elapsed().as_nanos();

            assert_ne!(rgba8_bytes_per_row(3).unwrap() % 256, 1); // pitch aligned
            assert_eq!(3 * 4, 12);
            assert_eq!(rgba8_bytes_per_row(3).unwrap(), 256);
            let tex = ctx
                .create_texture_2d(Texture2dDesc {
                    label: ResourceLabel::parse("spirula:gpu:texture:host-rt").unwrap(),
                    width: 3,
                    height: 2,
                    format: TextureFormat::Rgba8Unorm,
                    usage: TextureUsage::COPY_SRC_DST,
                })
                .unwrap();
            let pixels: Vec<u8> = (0..24).collect();
            ctx.upload_texture_rgba8(&tex, &pixels).unwrap();
            let s = ctx.submit().unwrap();
            ctx.wait(s, Some(Duration::from_secs(5))).unwrap();
            let ticket = ctx.readback_texture_rgba8(&tex).unwrap();
            let s2 = ctx.submit().unwrap();
            ctx.wait(s2, Some(Duration::from_secs(5))).unwrap();
            let out = ctx.map_readback(&ticket).unwrap();
            assert_eq!(out, pixels);

            let acc = ctx.resource_accounting();
            eprintln!(
                "host_resource wgpu={} backend={:?} accounted current={} peak={} source={} upload_wait_ns≈{}",
                WGPU_VERSION,
                ctx.report().map(|r| r.backend),
                acc.current_bytes,
                acc.peak_bytes,
                acc.source,
                upload_ns
            );
            drop(buf);
            drop(tex);
            assert_eq!(ctx.resource_accounting().current_bytes, 0);

            // Host: illegal 2D size must be typed error (not wgpu panic/exit 101).
            let max_dim = ctx
                .report()
                .expect("live report")
                .device_limits
                .max_texture_dimension_2d;
            let over = max_dim.saturating_add(1).max(1_000_000);
            let Err(err) = ctx.create_texture_2d(Texture2dDesc {
                label: ResourceLabel::parse("spirula:gpu:texture:oversize").unwrap(),
                width: over,
                height: 1,
                format: TextureFormat::Rgba8Unorm,
                usage: TextureUsage::COPY_SRC_DST,
            }) else {
                panic!("expected oversize texture to fail on host");
            };
            assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
            assert!(
                err.message().contains("max_texture_dimension_2d"),
                "msg={}",
                err.message()
            );
            assert_eq!(ctx.resource_accounting().current_bytes, 0);

            ctx.close();
        }
        Err(failure) => {
            eprintln!(
                "host_resource_unavailable stage={:?} code={} message={}",
                failure.stage,
                failure.error.code().as_str(),
                failure.error.message()
            );
            assert!(matches!(
                failure.stage,
                OpenFailureStage::EnumerateEmpty
                    | OpenFailureStage::Select
                    | OpenFailureStage::DeviceCreate
            ));
            match failure.stage {
                OpenFailureStage::DeviceCreate => {
                    assert_eq!(failure.error.code().as_str(), "SP-GPU-004");
                }
                _ => assert_eq!(failure.error.code().as_str(), "SP-GPU-001"),
            }
        }
    }
}
