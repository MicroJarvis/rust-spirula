//! Host headless affine compute: upload → dispatch → readback (P2-F04).

use spirula_core::{ContextKey, ErrorCode};
use spirula_gpu::{
    AFFINE_U32_BIND_LAYOUT_VERSION, AFFINE_U32_ENTRY, AFFINE_U32_WGSL, AffineU32Params, BufferDesc,
    BufferUsage, CapabilityRequest, ComputePipelineDesc, GpuContext, RESOURCE_ACCOUNT_SOURCE,
    ResourceLabel, SpecConstants, WGPU_VERSION, cpu_affine_u32,
};
use std::time::{Duration, Instant};

fn label(s: &str) -> ResourceLabel {
    ResourceLabel::parse(s).unwrap()
}

fn words_to_bytes(words: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(words.len() * 4);
    for w in words {
        out.extend_from_slice(&w.to_le_bytes());
    }
    out
}

fn bytes_to_words(bytes: &[u8]) -> Vec<u32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect()
}

#[test]
fn host_affine_compute_roundtrip_or_precise_unavailable() {
    let request = CapabilityRequest::diagnose_default();
    match GpuContext::open(&request) {
        Ok(mut ctx) => {
            let backend = ctx.report().map(|r| r.backend);
            let name = ctx
                .report()
                .map(|r| r.adapter.identity.name.clone())
                .unwrap_or_default();
            let limits = ctx.report().map(|r| r.device_limits);

            let count = 100u32; // not a multiple of wg=64
            let input: Vec<u32> = (0..count)
                .map(|i| i.wrapping_mul(5).wrapping_add(2))
                .collect();
            let scale = 3u32;
            let bias = 17u32;

            let params = ctx
                .create_buffer(BufferDesc {
                    label: label("spirula:gpu:buffer:affine-params"),
                    size: 16,
                    usage: BufferUsage::UNIFORM_UPLOAD,
                })
                .unwrap();
            let in_buf = ctx
                .create_buffer(BufferDesc {
                    label: label("spirula:gpu:buffer:affine-in"),
                    size: u64::from(count) * 4,
                    usage: BufferUsage::STORAGE_COPY,
                })
                .unwrap();
            let out_buf = ctx
                .create_buffer(BufferDesc {
                    label: label("spirula:gpu:buffer:affine-out"),
                    size: u64::from(count) * 4,
                    usage: BufferUsage::STORAGE_COPY,
                })
                .unwrap();

            ctx.upload_buffer(
                &params,
                0,
                &AffineU32Params::new(scale, bias, count).as_bytes(),
            )
            .unwrap();
            ctx.upload_buffer(&in_buf, 0, &words_to_bytes(&input))
                .unwrap();
            let s = ctx.submit().unwrap();
            ctx.wait(s, Some(Duration::from_secs(5))).unwrap();

            let t_cold = Instant::now();
            let pipe0 = ctx.obtain_affine_u32_pipeline(0).unwrap();
            let cold_ns = t_cold.elapsed().as_nanos();
            let t_warm = Instant::now();
            let pipe0b = ctx.obtain_affine_u32_pipeline(0).unwrap();
            let warm_ns = t_warm.elapsed().as_nanos();
            assert_eq!(pipe0.id(), pipe0b.id());
            assert_eq!(ctx.pipeline_cache_stats().hits, 1);

            let t_compute = Instant::now();
            ctx.dispatch_affine_u32(&pipe0, &params, &in_buf, &out_buf, count)
                .unwrap();
            let s = ctx.submit().unwrap();
            ctx.wait(s, Some(Duration::from_secs(5))).unwrap();
            let ticket = ctx
                .readback_buffer(&out_buf, 0, u64::from(count) * 4)
                .unwrap();
            let s = ctx.submit().unwrap();
            ctx.wait(s, Some(Duration::from_secs(5))).unwrap();
            let got0 = bytes_to_words(&ctx.map_readback(&ticket).unwrap());
            let compute_ns = t_compute.elapsed().as_nanos();
            assert_eq!(got0, cpu_affine_u32(&input, scale, bias, 0));

            let pipe1 = ctx.obtain_affine_u32_pipeline(4).unwrap();
            assert_ne!(pipe0.id(), pipe1.id());
            ctx.dispatch_affine_u32(&pipe1, &params, &in_buf, &out_buf, count)
                .unwrap();
            let s = ctx.submit().unwrap();
            ctx.wait(s, Some(Duration::from_secs(5))).unwrap();
            let ticket = ctx
                .readback_buffer(&out_buf, 0, u64::from(count) * 4)
                .unwrap();
            let s = ctx.submit().unwrap();
            ctx.wait(s, Some(Duration::from_secs(5))).unwrap();
            let got1 = bytes_to_words(&ctx.map_readback(&ticket).unwrap());
            assert_eq!(got1, cpu_affine_u32(&input, scale, bias, 4));
            assert_ne!(got0, got1);

            // Bad WGSL → SP-GPU-005 with path/line/message; retry ok; no poisoned cache.
            let entries_before = ctx.pipeline_cache_stats().entries;
            let bad_src = "\
// spirula-shader: bad-shader version: 1
fn broken( { this is not wgsl
";
            let err = ctx
                .obtain_compute_pipeline(ComputePipelineDesc {
                    source: bad_src.into(),
                    entry_point: AFFINE_U32_ENTRY.into(),
                    spec: SpecConstants::empty(),
                    bind_layout_version: AFFINE_U32_BIND_LAYOUT_VERSION,
                    label: label("spirula:gpu:pipeline:bad"),
                })
                .unwrap_err();
            assert_eq!(err.code(), ErrorCode::ShaderFailed);
            assert_eq!(err.code().as_str(), "SP-GPU-005");
            assert!(
                err.context_value(ContextKey::Path)
                    .is_some_and(|p| p.contains("bad-shader")),
                "path ctx={:?}",
                err.context()
            );
            assert!(
                err.context_value(ContextKey::Line).is_some(),
                "line ctx={:?}",
                err.context()
            );
            assert!(
                !err.message().is_empty(),
                "compiler message should be present"
            );
            assert_eq!(ctx.pipeline_cache_stats().entries, entries_before);
            let retry = ctx.obtain_affine_u32_pipeline(0).unwrap();
            assert_eq!(retry.id(), pipe0.id());

            // Illegal usage → typed, accounting unchanged.
            let wrong = ctx
                .create_buffer(BufferDesc {
                    label: label("spirula:gpu:buffer:no-uniform"),
                    size: 16,
                    usage: BufferUsage::COPY_SRC_DST,
                })
                .unwrap();
            let before = ctx.resource_accounting().current_bytes;
            assert_eq!(
                ctx.dispatch_affine_u32(&pipe0, &wrong, &in_buf, &out_buf, count)
                    .unwrap_err()
                    .code(),
                ErrorCode::InvalidArgument
            );
            assert_eq!(ctx.resource_accounting().current_bytes, before);

            // Alias input==output → typed (no wgpu panic).
            assert_eq!(
                ctx.dispatch_affine_u32(&pipe0, &params, &in_buf, &in_buf, count)
                    .unwrap_err()
                    .code(),
                ErrorCode::InvalidArgument
            );
            assert_eq!(ctx.resource_accounting().current_bytes, before);

            // params.count guard: only first element written.
            ctx.upload_buffer(&params, 0, &AffineU32Params::new(scale, bias, 1).as_bytes())
                .unwrap();
            let s = ctx.submit().unwrap();
            ctx.wait(s, Some(Duration::from_secs(5))).unwrap();
            // Clear output then dispatch launch=4 with Params.count=1.
            let zeros = vec![0u8; (count as usize) * 4];
            ctx.upload_buffer(&out_buf, 0, &zeros).unwrap();
            let s = ctx.submit().unwrap();
            ctx.wait(s, Some(Duration::from_secs(5))).unwrap();
            ctx.dispatch_affine_u32(&pipe0, &params, &in_buf, &out_buf, count)
                .unwrap();
            let s = ctx.submit().unwrap();
            ctx.wait(s, Some(Duration::from_secs(5))).unwrap();
            let ticket = ctx
                .readback_buffer(&out_buf, 0, u64::from(count) * 4)
                .unwrap();
            let s = ctx.submit().unwrap();
            ctx.wait(s, Some(Duration::from_secs(5))).unwrap();
            let guarded = bytes_to_words(&ctx.map_readback(&ticket).unwrap());
            assert_eq!(guarded[0], cpu_affine_u32(&[input[0]], scale, bias, 0)[0]);
            assert!(guarded[1..].iter().all(|&w| w == 0));

            // Stale handle after same-key source rebuild → typed reject.
            let changed = AFFINE_U32_WGSL.replace("+ MODE;", "+ MODE + 100u;");
            let new_pipe = ctx
                .obtain_compute_pipeline(ComputePipelineDesc {
                    source: changed,
                    entry_point: AFFINE_U32_ENTRY.into(),
                    spec: SpecConstants::single("MODE", 0),
                    bind_layout_version: AFFINE_U32_BIND_LAYOUT_VERSION,
                    label: label("spirula:gpu:pipeline:changed"),
                })
                .unwrap();
            assert_ne!(pipe0.id(), new_pipe.id());
            assert_eq!(
                ctx.dispatch_affine_u32(&pipe0, &params, &in_buf, &out_buf, 4)
                    .unwrap_err()
                    .code(),
                ErrorCode::InvalidArgument
            );
            // Failed bad replace must not drop the live new pipeline.
            let bad = "// spirula-shader: affine-u32 version: 1\nfn broken( {\n";
            assert_eq!(
                ctx.obtain_compute_pipeline(ComputePipelineDesc {
                    source: bad.into(),
                    entry_point: AFFINE_U32_ENTRY.into(),
                    spec: SpecConstants::single("MODE", 0),
                    bind_layout_version: AFFINE_U32_BIND_LAYOUT_VERSION,
                    label: label("spirula:gpu:pipeline:bad-replace"),
                })
                .unwrap_err()
                .code(),
                ErrorCode::ShaderFailed
            );
            ctx.dispatch_affine_u32(&new_pipe, &params, &in_buf, &out_buf, 4)
                .unwrap();
            let s = ctx.submit().unwrap();
            ctx.wait(s, Some(Duration::from_secs(5))).unwrap();

            // Oversized params buffer still binds narrow 16B (no entire-buffer panic).
            let big_params = ctx
                .create_buffer(BufferDesc {
                    label: label("spirula:gpu:buffer:big-params"),
                    size: 65540,
                    usage: BufferUsage::UNIFORM_UPLOAD,
                })
                .unwrap();
            let mut big_bytes = vec![0u8; 65540];
            big_bytes[..16].copy_from_slice(&AffineU32Params::new(2, 5, 4).as_bytes());
            ctx.upload_buffer(&big_params, 0, &big_bytes).unwrap();
            let s = ctx.submit().unwrap();
            ctx.wait(s, Some(Duration::from_secs(5))).unwrap();
            ctx.dispatch_affine_u32(&new_pipe, &big_params, &in_buf, &out_buf, 4)
                .unwrap();
            let s = ctx.submit().unwrap();
            ctx.wait(s, Some(Duration::from_secs(5))).unwrap();

            let acc = ctx.resource_accounting();
            eprintln!(
                "host_compute wgpu={} device={:?} backend={:?} limits={:?} \
                 cold_pipeline_ns={} warm_pipeline_ns={} compute_phase_ns={} \
                 accounted peak={} current={} source={}",
                WGPU_VERSION,
                name,
                backend,
                limits,
                cold_ns,
                warm_ns,
                compute_ns,
                acc.peak_bytes,
                acc.current_bytes,
                acc.source
            );
            assert_eq!(acc.source, RESOURCE_ACCOUNT_SOURCE);

            drop(params);
            drop(in_buf);
            drop(out_buf);
            drop(wrong);
            drop(big_params);
            assert_eq!(ctx.resource_accounting().current_bytes, 0);
            ctx.close();
        }
        Err(failure) => {
            eprintln!(
                "host_compute unavailable: stage={:?} code={} msg={}",
                failure.stage,
                failure.error.code().as_str(),
                failure.error.message()
            );
            assert!(
                matches!(
                    failure.error.code().as_str(),
                    "SP-GPU-001" | "SP-GPU-004" | "SP-GPU-002"
                ),
                "unexpected open failure {}",
                failure.error.code().as_str()
            );
        }
    }
}

#[test]
fn host_mixed_compute_readback_abandon_removes_owned_command_only() {
    let request = CapabilityRequest::diagnose_default();
    let Ok(mut ctx) = GpuContext::open(&request) else {
        eprintln!("host_mixed_pending_drop skipped: device unavailable");
        return;
    };
    let pipe = ctx.obtain_affine_u32_pipeline(0).unwrap();
    let params = ctx
        .create_buffer(BufferDesc {
            label: label("spirula:gpu:buffer:pd-p"),
            size: 16,
            usage: BufferUsage::UNIFORM_UPLOAD,
        })
        .unwrap();
    let inn = ctx
        .create_buffer(BufferDesc {
            label: label("spirula:gpu:buffer:pd-i"),
            size: 16,
            usage: BufferUsage::STORAGE_COPY,
        })
        .unwrap();
    let out = ctx
        .create_buffer(BufferDesc {
            label: label("spirula:gpu:buffer:pd-o"),
            size: 16,
            usage: BufferUsage::STORAGE_COPY,
        })
        .unwrap();
    ctx.upload_buffer(&params, 0, &AffineU32Params::new(2, 5, 4).as_bytes())
        .unwrap();
    ctx.upload_buffer(&inn, 0, &words_to_bytes(&[1, 2, 3, 4]))
        .unwrap();
    let s = ctx.submit().unwrap();
    ctx.wait(s, Some(Duration::from_secs(5))).unwrap();

    ctx.dispatch_affine_u32(&pipe, &params, &inn, &out, 4)
        .unwrap();
    let other = ctx
        .create_buffer(BufferDesc {
            label: label("spirula:gpu:buffer:pd-other"),
            size: 4096,
            usage: BufferUsage::COPY_SRC_DST,
        })
        .unwrap();
    let ticket = ctx.readback_buffer(&other, 0, 4096).unwrap();
    let before = ctx.pending_resource_counts();
    eprintln!(
        "host_pending_drop before={:?} device={:?}",
        before,
        ctx.report().map(|r| &r.adapter.identity.name)
    );
    assert_eq!(before.commands, 2);
    assert_eq!(before.stagings, 1);
    assert_eq!(before.buffers, 4);
    assert_eq!(before.current_bytes, 8240);

    // Drop ticket then source: must remove only the readback-owned command.
    drop(ticket);
    drop(other);
    let after = ctx.pending_resource_counts();
    eprintln!("host_pending_drop after={after:?}");
    assert_eq!(after.commands, 1, "compute command must remain");
    assert_eq!(after.stagings, 0);
    assert_eq!(after.buffers, 3);
    assert_eq!(after.current_bytes, 48);

    let s = ctx.submit().unwrap();
    ctx.wait(s, Some(Duration::from_secs(5))).unwrap();
    let t = ctx.readback_buffer(&out, 0, 16).unwrap();
    let s = ctx.submit().unwrap();
    ctx.wait(s, Some(Duration::from_secs(5))).unwrap();
    assert_eq!(
        bytes_to_words(&ctx.map_readback(&t).unwrap()),
        vec![7, 9, 11, 13]
    );

    // Interleaved abandons: compute + two readbacks; drop tickets before sources.
    ctx.dispatch_affine_u32(&pipe, &params, &inn, &out, 4)
        .unwrap();
    let a = ctx
        .create_buffer(BufferDesc {
            label: label("spirula:gpu:buffer:pd-a"),
            size: 64,
            usage: BufferUsage::COPY_SRC_DST,
        })
        .unwrap();
    let b = ctx
        .create_buffer(BufferDesc {
            label: label("spirula:gpu:buffer:pd-b"),
            size: 64,
            usage: BufferUsage::COPY_SRC_DST,
        })
        .unwrap();
    let ta = ctx.readback_buffer(&a, 0, 64).unwrap();
    let tb = ctx.readback_buffer(&b, 0, 64).unwrap();
    assert_eq!(ctx.pending_resource_counts().commands, 3);
    assert_eq!(ctx.pending_resource_counts().stagings, 2);
    drop(tb);
    drop(ta);
    let mid = ctx.pending_resource_counts();
    assert_eq!(mid.commands, 1);
    assert_eq!(mid.stagings, 0);
    drop(b);
    drop(a);
    let s = ctx.submit().unwrap();
    ctx.wait(s, Some(Duration::from_secs(5))).unwrap();
    let t = ctx.readback_buffer(&out, 0, 16).unwrap();
    let s = ctx.submit().unwrap();
    ctx.wait(s, Some(Duration::from_secs(5))).unwrap();
    assert_eq!(
        bytes_to_words(&ctx.map_readback(&t).unwrap()),
        vec![7, 9, 11, 13]
    );

    drop(params);
    drop(inn);
    drop(out);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);
    ctx.close();
}
