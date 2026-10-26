//! Synthetic P2-F04: pipeline key/cache, affine dispatch, drop/budget (no host GPU).

use spirula_core::ErrorCode;
use spirula_gpu::{
    AFFINE_U32_BIND_LAYOUT_VERSION, AFFINE_U32_ENTRY, AFFINE_U32_WGSL, AdapterCapabilities,
    AdapterClass, AdapterIdentity, AdapterLimits, AffineU32Params, BackendKind, BufferDesc,
    BufferUsage, CapabilityRequest, ComputePipelineDesc, FeatureBits, FieldAvailability,
    GpuContext, PipelineKey, RESOURCE_ACCOUNT_SOURCE, ResourceLabel, SpecConstants, cpu_affine_u32,
    parse_shader_header,
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

fn affine_desc(source: &str, entry: &str, mode: u32, bind: u32) -> ComputePipelineDesc {
    ComputePipelineDesc {
        source: source.to_owned(),
        entry_point: entry.to_owned(),
        spec: SpecConstants::single("MODE", mode),
        bind_layout_version: bind,
        label: label("spirula:gpu:pipeline:test"),
    }
}

#[test]
fn synthetic_header_and_cpu_reference() {
    let h = parse_shader_header(AFFINE_U32_WGSL).unwrap();
    assert_eq!(h.name, "affine-u32");
    assert_eq!(h.version, 1);
    assert_eq!(cpu_affine_u32(&[1, 2, 3], 4, 5, 7), vec![16, 20, 24]);
    assert!(parse_shader_header("not a header\n").is_err());
}

#[test]
fn synthetic_affine_non_wg_multiple_and_mode_change() {
    let mut ctx = open_ctx();
    // 100 is not a multiple of workgroup 64.
    let count = 100u32;
    let input: Vec<u32> = (0..count)
        .map(|i| i.wrapping_mul(3).wrapping_add(1))
        .collect();
    let scale = 7u32;
    let bias = 11u32;

    let params = ctx
        .create_buffer(BufferDesc {
            label: label("spirula:gpu:buffer:params"),
            size: 16,
            usage: BufferUsage::UNIFORM_UPLOAD,
        })
        .unwrap();
    let in_buf = ctx
        .create_buffer(BufferDesc {
            label: label("spirula:gpu:buffer:in"),
            size: u64::from(count) * 4,
            usage: BufferUsage::STORAGE_COPY,
        })
        .unwrap();
    let out_buf = ctx
        .create_buffer(BufferDesc {
            label: label("spirula:gpu:buffer:out"),
            size: u64::from(count) * 4,
            usage: BufferUsage::STORAGE_COPY,
        })
        .unwrap();

    let p = AffineU32Params::new(scale, bias, count);
    ctx.upload_buffer(&params, 0, &p.as_bytes()).unwrap();
    ctx.upload_buffer(&in_buf, 0, &words_to_bytes(&input))
        .unwrap();
    let s = ctx.submit().unwrap();
    ctx.wait(s, Some(Duration::from_secs(1))).unwrap();

    let pipe0 = ctx.obtain_affine_u32_pipeline(0).unwrap();
    ctx.dispatch_affine_u32(&pipe0, &params, &in_buf, &out_buf, count)
        .unwrap();
    let s = ctx.submit().unwrap();
    ctx.wait(s, Some(Duration::from_secs(1))).unwrap();
    let t = ctx
        .readback_buffer(&out_buf, 0, u64::from(count) * 4)
        .unwrap();
    let s = ctx.submit().unwrap();
    ctx.wait(s, Some(Duration::from_secs(1))).unwrap();
    let got0 = bytes_to_words(&ctx.map_readback(&t).unwrap());
    assert_eq!(got0, cpu_affine_u32(&input, scale, bias, 0));

    let pipe1 = ctx.obtain_affine_u32_pipeline(9).unwrap();
    assert_ne!(pipe0.id(), pipe1.id());
    ctx.dispatch_affine_u32(&pipe1, &params, &in_buf, &out_buf, count)
        .unwrap();
    let s = ctx.submit().unwrap();
    ctx.wait(s, Some(Duration::from_secs(1))).unwrap();
    let t = ctx
        .readback_buffer(&out_buf, 0, u64::from(count) * 4)
        .unwrap();
    let s = ctx.submit().unwrap();
    ctx.wait(s, Some(Duration::from_secs(1))).unwrap();
    let got1 = bytes_to_words(&ctx.map_readback(&t).unwrap());
    assert_eq!(got1, cpu_affine_u32(&input, scale, bias, 9));
    assert_ne!(got0, got1);

    drop(params);
    drop(in_buf);
    drop(out_buf);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);
    assert_eq!(ctx.resource_accounting().source, RESOURCE_ACCOUNT_SOURCE);
}

#[test]
fn synthetic_pipeline_key_fields_and_cache_hit() {
    let mut ctx = open_ctx();
    let base = AFFINE_U32_WGSL;
    let a = ctx
        .obtain_compute_pipeline(affine_desc(
            base,
            AFFINE_U32_ENTRY,
            0,
            AFFINE_U32_BIND_LAYOUT_VERSION,
        ))
        .unwrap();
    let stats_after_miss = ctx.pipeline_cache_stats();
    assert_eq!(stats_after_miss.misses, 1);
    assert_eq!(stats_after_miss.hits, 0);

    let a2 = ctx
        .obtain_compute_pipeline(affine_desc(
            base,
            AFFINE_U32_ENTRY,
            0,
            AFFINE_U32_BIND_LAYOUT_VERSION,
        ))
        .unwrap();
    assert_eq!(a.id(), a2.id());
    let stats_hit = ctx.pipeline_cache_stats();
    assert_eq!(stats_hit.hits, 1);
    assert_eq!(stats_hit.misses, 1);

    // spec MODE
    let b = ctx
        .obtain_compute_pipeline(affine_desc(
            base,
            AFFINE_U32_ENTRY,
            1,
            AFFINE_U32_BIND_LAYOUT_VERSION,
        ))
        .unwrap();
    assert_ne!(a.id(), b.id());

    // entry
    let c = ctx
        .obtain_compute_pipeline(affine_desc(
            base,
            "main_alt",
            0,
            AFFINE_U32_BIND_LAYOUT_VERSION,
        ))
        .unwrap();
    assert_ne!(a.id(), c.id());

    // name in header
    let renamed = base.replacen("affine-u32", "affine-u32-b", 1);
    let d = ctx
        .obtain_compute_pipeline(affine_desc(
            &renamed,
            AFFINE_U32_ENTRY,
            0,
            AFFINE_U32_BIND_LAYOUT_VERSION,
        ))
        .unwrap();
    assert_ne!(a.id(), d.id());

    // version in header
    let reversioned = base.replacen("version: 1", "version: 2", 1);
    let e = ctx
        .obtain_compute_pipeline(affine_desc(
            &reversioned,
            AFFINE_U32_ENTRY,
            0,
            AFFINE_U32_BIND_LAYOUT_VERSION,
        ))
        .unwrap();
    assert_ne!(a.id(), e.id());

    // bind_layout_version unsupported → typed reject (distinct key shape)
    let err = ctx
        .obtain_compute_pipeline(affine_desc(base, AFFINE_U32_ENTRY, 0, 99))
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidArgument);
    let k1 = PipelineKey {
        shader_name: "affine-u32".into(),
        shader_version: 1,
        entry_point: "main".into(),
        spec: SpecConstants::single("MODE", 0).sorted(),
        bind_layout_version: 1,
    };
    let k2 = PipelineKey {
        bind_layout_version: 2,
        ..k1.clone()
    };
    assert_ne!(k1, k2);

    // same key, different source body → digest miss / rebuild (no quiet reuse)
    let mutated = format!("{base}\n// body-change-marker\n");
    let before = ctx.pipeline_cache_stats();
    let f = ctx
        .obtain_compute_pipeline(affine_desc(
            &mutated,
            AFFINE_U32_ENTRY,
            0,
            AFFINE_U32_BIND_LAYOUT_VERSION,
        ))
        .unwrap();
    let after = ctx.pipeline_cache_stats();
    assert!(after.misses > before.misses);
    assert_ne!(a.id(), f.id());
}

#[test]
fn synthetic_illegal_usage_range_and_header_retry() {
    let mut ctx = open_ctx();
    let pipe = ctx.obtain_affine_u32_pipeline(0).unwrap();
    let params = ctx
        .create_buffer(BufferDesc {
            label: label("spirula:gpu:buffer:params-bad"),
            size: 16,
            usage: BufferUsage::COPY_SRC_DST, // missing uniform
        })
        .unwrap();
    let in_buf = ctx
        .create_buffer(BufferDesc {
            label: label("spirula:gpu:buffer:in-bad"),
            size: 16,
            usage: BufferUsage::STORAGE_COPY,
        })
        .unwrap();
    let out_buf = ctx
        .create_buffer(BufferDesc {
            label: label("spirula:gpu:buffer:out-bad"),
            size: 16,
            usage: BufferUsage::STORAGE_COPY,
        })
        .unwrap();
    let before = ctx.resource_accounting().current_bytes;
    assert_eq!(
        ctx.dispatch_affine_u32(&pipe, &params, &in_buf, &out_buf, 4)
            .unwrap_err()
            .code(),
        ErrorCode::InvalidArgument
    );
    assert_eq!(ctx.resource_accounting().current_bytes, before);

    // undersized output
    let tiny = ctx
        .create_buffer(BufferDesc {
            label: label("spirula:gpu:buffer:tiny"),
            size: 4,
            usage: BufferUsage::STORAGE_COPY,
        })
        .unwrap();
    let params_ok = ctx
        .create_buffer(BufferDesc {
            label: label("spirula:gpu:buffer:params-ok"),
            size: 16,
            usage: BufferUsage::UNIFORM_UPLOAD,
        })
        .unwrap();
    assert_eq!(
        ctx.dispatch_affine_u32(&pipe, &params_ok, &in_buf, &tiny, 4)
            .unwrap_err()
            .code(),
        ErrorCode::ValueOutOfRange
    );

    // bad header → ShaderFailed; good source retries
    let bad = ctx
        .obtain_compute_pipeline(affine_desc(
            "totally broken\n",
            AFFINE_U32_ENTRY,
            0,
            AFFINE_U32_BIND_LAYOUT_VERSION,
        ))
        .unwrap_err();
    assert_eq!(bad.code(), ErrorCode::ShaderFailed);
    assert_eq!(ctx.pipeline_cache_stats().entries, 1); // only prior pipe
    let ok = ctx.obtain_affine_u32_pipeline(0).unwrap();
    assert_eq!(ok.id(), pipe.id()); // cache hit after failed attempt
}

#[test]
fn synthetic_context_generation_and_cross_context_reject() {
    let mut a = open_ctx();
    let mut b = open_ctx();
    let pipe_a = a.obtain_affine_u32_pipeline(0).unwrap();
    let pipe_b = b.obtain_affine_u32_pipeline(0).unwrap();

    let params = a
        .create_buffer(BufferDesc {
            label: label("spirula:gpu:buffer:p"),
            size: 16,
            usage: BufferUsage::UNIFORM_UPLOAD,
        })
        .unwrap();
    let inn = a
        .create_buffer(BufferDesc {
            label: label("spirula:gpu:buffer:i"),
            size: 16,
            usage: BufferUsage::STORAGE_COPY,
        })
        .unwrap();
    let out = a
        .create_buffer(BufferDesc {
            label: label("spirula:gpu:buffer:o"),
            size: 16,
            usage: BufferUsage::STORAGE_COPY,
        })
        .unwrap();

    assert_eq!(
        a.dispatch_affine_u32(&pipe_b, &params, &inn, &out, 4)
            .unwrap_err()
            .code(),
        ErrorCode::InvalidArgument
    );

    a.reopen_synthetic(
        &CapabilityRequest::diagnose_default(),
        sample_adapter(),
        false,
    )
    .unwrap();
    assert_eq!(a.pipeline_cache_stats().entries, 0);
    let err = a
        .dispatch_affine_u32(&pipe_a, &params, &inn, &out, 4)
        .unwrap_err();
    assert!(
        err.code() == ErrorCode::DeviceLost || err.code() == ErrorCode::InvalidArgument,
        "code={:?}",
        err.code()
    );
}

#[test]
fn synthetic_dispatch_drop_before_and_after_submit() {
    let mut ctx = open_ctx();
    let pipe = ctx.obtain_affine_u32_pipeline(0).unwrap();
    let count = 8u32;
    let params = ctx
        .create_buffer(BufferDesc {
            label: label("spirula:gpu:buffer:p2"),
            size: 16,
            usage: BufferUsage::UNIFORM_UPLOAD,
        })
        .unwrap();
    let inn = ctx
        .create_buffer(BufferDesc {
            label: label("spirula:gpu:buffer:i2"),
            size: u64::from(count) * 4,
            usage: BufferUsage::STORAGE_COPY,
        })
        .unwrap();
    let out = ctx
        .create_buffer(BufferDesc {
            label: label("spirula:gpu:buffer:o2"),
            size: u64::from(count) * 4,
            usage: BufferUsage::STORAGE_COPY,
        })
        .unwrap();
    let p = AffineU32Params::new(2, 3, count);
    ctx.upload_buffer(&params, 0, &p.as_bytes()).unwrap();
    ctx.upload_buffer(&inn, 0, &words_to_bytes(&(0..count).collect::<Vec<_>>()))
        .unwrap();
    let s = ctx.submit().unwrap();
    ctx.wait(s, Some(Duration::from_secs(1))).unwrap();

    ctx.dispatch_affine_u32(&pipe, &params, &inn, &out, count)
        .unwrap();
    let pending = ctx.resource_accounting().current_bytes;
    assert!(pending > 0);
    drop(params);
    drop(inn);
    drop(out);
    // pending hold retains until submit/wait
    assert_eq!(ctx.resource_accounting().current_bytes, pending);
    let s = ctx.submit().unwrap();
    ctx.wait(s, Some(Duration::from_secs(1))).unwrap();
    assert_eq!(ctx.resource_accounting().current_bytes, 0);

    // after-submit retain then drop zeros
    let params = ctx
        .create_buffer(BufferDesc {
            label: label("spirula:gpu:buffer:p3"),
            size: 16,
            usage: BufferUsage::UNIFORM_UPLOAD,
        })
        .unwrap();
    let inn = ctx
        .create_buffer(BufferDesc {
            label: label("spirula:gpu:buffer:i3"),
            size: u64::from(count) * 4,
            usage: BufferUsage::STORAGE_COPY,
        })
        .unwrap();
    let out = ctx
        .create_buffer(BufferDesc {
            label: label("spirula:gpu:buffer:o3"),
            size: u64::from(count) * 4,
            usage: BufferUsage::STORAGE_COPY,
        })
        .unwrap();
    ctx.upload_buffer(&params, 0, &AffineU32Params::new(1, 0, count).as_bytes())
        .unwrap();
    ctx.upload_buffer(&inn, 0, &words_to_bytes(&(0..count).collect::<Vec<_>>()))
        .unwrap();
    let s = ctx.submit().unwrap();
    ctx.wait(s, Some(Duration::from_secs(1))).unwrap();
    ctx.dispatch_affine_u32(&pipe, &params, &inn, &out, count)
        .unwrap();
    let s = ctx.submit().unwrap();
    ctx.wait(s, Some(Duration::from_secs(1))).unwrap();
    let retained = ctx.resource_accounting().current_bytes;
    assert!(retained > 0);
    drop(params);
    drop(inn);
    drop(out);
    assert_eq!(ctx.resource_accounting().current_bytes, 0);
}

#[test]
fn synthetic_inject_lost_clears_pipeline_cache() {
    let mut ctx = open_ctx();
    let _ = ctx.obtain_affine_u32_pipeline(0).unwrap();
    assert_eq!(ctx.pipeline_cache_stats().entries, 1);
    ctx.inject_lost("pipeline lost test");
    assert_eq!(ctx.pipeline_cache_stats().entries, 0);
    assert_eq!(
        ctx.obtain_affine_u32_pipeline(0).unwrap_err().code(),
        ErrorCode::DeviceLost
    );
}
