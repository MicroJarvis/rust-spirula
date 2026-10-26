//! P2-F04: narrow compute shader / pipeline key / cache / dispatch.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use spirula_core::{ContextKey, DomainError, ErrorCode, UserAction};

use crate::context::{ContextGeneration, ContextId, ContextStatus, GpuContext};
use crate::resource::{
    BufferHandle, COPY_BUFFER_ALIGNMENT, GpuBuffer, check_copy_buffer_align, wgpu_scoped,
};

/// Embedded affine-u32 WGSL (`shaders/affine-u32.wgsl`; header name matches path).
pub const AFFINE_U32_WGSL: &str = include_str!("../shaders/affine-u32.wgsl");
pub const AFFINE_U32_NAME: &str = "affine-u32";
pub const AFFINE_U32_VERSION: u32 = 1;
pub const AFFINE_U32_ENTRY: &str = "main";
/// Default / primary affine bind-layout epoch (same ABI as v2).
pub const AFFINE_U32_BIND_LAYOUT_VERSION: u32 = 1;
/// Second supported epoch: identical binding ABI; exists so key-field
/// `bind_layout_version` changes produce a real cache miss / new pipeline id.
pub const AFFINE_U32_BIND_LAYOUT_VERSION_V2: u32 = 2;
pub const AFFINE_U32_WORKGROUP_SIZE: u32 = 64;
/// Bound size for affine `Params` uniform (narrow range; not entire buffer).
pub const AFFINE_U32_PARAMS_BYTES: u64 = 16;

/// P2-F04 narrow affine ABI supports exactly these layout epochs (not a
/// general layout registry). Unsupported versions → typed reject.
pub fn affine_bind_layout_supported(version: u32) -> bool {
    version == AFFINE_U32_BIND_LAYOUT_VERSION || version == AFFINE_U32_BIND_LAYOUT_VERSION_V2
}

static NEXT_PIPELINE_ID: AtomicU64 = AtomicU64::new(1);

fn shader_err(msg: impl Into<String>) -> DomainError {
    DomainError::try_new(ErrorCode::ShaderFailed, msg)
        .expect("non-empty")
        .with_user_action(UserAction::CorrectInput)
}

fn inp(msg: impl Into<String>, field: &str) -> DomainError {
    DomainError::try_new(ErrorCode::InvalidArgument, msg)
        .expect("non-empty")
        .with_user_action(UserAction::CorrectInput)
        .with_context(ContextKey::Field, field)
        .expect("field")
}

fn range_err(msg: impl Into<String>) -> DomainError {
    DomainError::try_new(ErrorCode::ValueOutOfRange, msg)
        .expect("non-empty")
        .with_user_action(UserAction::CorrectInput)
}

fn gpu_lost(msg: impl Into<String>) -> DomainError {
    DomainError::try_new(ErrorCode::DeviceLost, msg)
        .expect("non-empty")
        .with_user_action(UserAction::RecreateDevice)
}

/// CPU reference for the affine-u32 kernel.
pub fn cpu_affine_u32(input: &[u32], scale: u32, bias: u32, mode: u32) -> Vec<u32> {
    input
        .iter()
        .map(|&x| x.wrapping_mul(scale).wrapping_add(bias).wrapping_add(mode))
        .collect()
}

/// 16-byte uniform params matching `shaders/affine-u32.wgsl` `Params`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AffineU32Params {
    pub scale: u32,
    pub bias: u32,
    pub count: u32,
    pub _pad: u32,
}

impl AffineU32Params {
    pub fn new(scale: u32, bias: u32, count: u32) -> Self {
        Self {
            scale,
            bias,
            count,
            _pad: 0,
        }
    }

    pub fn as_bytes(&self) -> [u8; 16] {
        let mut out = [0u8; 16];
        out[0..4].copy_from_slice(&self.scale.to_le_bytes());
        out[4..8].copy_from_slice(&self.bias.to_le_bytes());
        out[8..12].copy_from_slice(&self.count.to_le_bytes());
        out[12..16].copy_from_slice(&self._pad.to_le_bytes());
        out
    }
}

/// Parsed `// spirula-shader: <name> version: <n>` header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShaderHeader {
    pub name: String,
    pub version: u32,
}

pub fn parse_shader_header(source: &str) -> Result<ShaderHeader, DomainError> {
    let first = source
        .lines()
        .next()
        .ok_or_else(|| shader_err("empty shader source"))?;
    let rest = first
        .strip_prefix("// spirula-shader:")
        .ok_or_else(|| {
            shader_err("shader first line must be '// spirula-shader: <name> version: <n>'")
        })?
        .trim();
    let mut parts = rest.split_whitespace();
    let name = parts
        .next()
        .ok_or_else(|| shader_err("missing shader name in header"))?
        .to_owned();
    if parts.next() != Some("version:") {
        return Err(shader_err("shader header missing 'version:' token"));
    }
    let version: u32 = parts
        .next()
        .ok_or_else(|| shader_err("shader header missing version number"))?
        .parse()
        .map_err(|_| shader_err("shader version must be an integer"))?;
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(shader_err(
            "shader name must be lowercase kebab (ascii alnum/dash)",
        ));
    }
    Ok(ShaderHeader { name, version })
}

fn source_digest(source: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut h);
    h.finish()
}

/// Sorted override constants (name → u32) included in the pipeline key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SpecConstants {
    pub values: Vec<(String, u32)>,
}

impl SpecConstants {
    pub fn empty() -> Self {
        Self { values: Vec::new() }
    }

    pub fn single(name: &str, value: u32) -> Self {
        Self {
            values: vec![(name.to_owned(), value)],
        }
    }

    pub fn sorted(mut self) -> Self {
        self.values.sort_by(|a, b| a.0.cmp(&b.0));
        self
    }
}

/// Cache key: any field change must miss.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PipelineKey {
    pub shader_name: String,
    pub shader_version: u32,
    pub entry_point: String,
    pub spec: SpecConstants,
    pub bind_layout_version: u32,
}

#[derive(Debug, Clone)]
pub struct ComputePipelineDesc {
    pub source: String,
    pub entry_point: String,
    pub spec: SpecConstants,
    pub bind_layout_version: u32,
    pub label: crate::resource::ResourceLabel,
}

#[derive(Clone)]
pub struct ComputePipelineHandle {
    inner: Arc<PipelineInner>,
}

impl std::fmt::Debug for ComputePipelineHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComputePipelineHandle")
            .field("id", &self.inner.id)
            .field("context_id", &self.inner.context_id)
            .field("generation", &self.inner.generation)
            .field("key", &self.inner.key)
            .finish()
    }
}

struct PipelineInner {
    context_id: ContextId,
    generation: ContextGeneration,
    id: u64,
    key: PipelineKey,
    #[allow(dead_code)]
    cache: Arc<Mutex<PipelineCache>>,
}

impl ComputePipelineHandle {
    pub fn context_id(&self) -> ContextId {
        self.inner.context_id
    }
    pub fn generation(&self) -> ContextGeneration {
        self.inner.generation
    }
    pub fn id(&self) -> u64 {
        self.inner.id
    }
    pub fn key(&self) -> &PipelineKey {
        &self.inner.key
    }
}

pub(crate) struct PipelineCache {
    context_id: ContextId,
    generation: ContextGeneration,
    invalidated: bool,
    entries: HashMap<PipelineKey, CachedPipeline>,
    pub hits: u64,
    pub misses: u64,
}

enum CachedPipeline {
    Wgpu {
        pipeline: wgpu::ComputePipeline,
        bind_layout: wgpu::BindGroupLayout,
        source_digest: u64,
        id: u64,
    },
    Synthetic {
        source_digest: u64,
        id: u64,
        mode: u32,
    },
}

impl PipelineCache {
    pub(crate) fn new(context_id: ContextId, generation: ContextGeneration) -> Self {
        Self {
            context_id,
            generation,
            invalidated: false,
            entries: HashMap::new(),
            hits: 0,
            misses: 0,
        }
    }

    pub(crate) fn invalidate_all(&mut self) {
        self.invalidated = true;
        self.entries.clear();
    }

    fn ensure_live(
        &self,
        ctx_id: ContextId,
        generation: ContextGeneration,
    ) -> Result<(), DomainError> {
        if self.invalidated {
            return Err(gpu_lost("pipeline cache invalidated (closed/lost/reopen)"));
        }
        if ctx_id != self.context_id {
            return Err(inp(
                "pipeline handle belongs to a different context",
                "pipeline",
            ));
        }
        if generation != self.generation {
            return Err(gpu_lost("pipeline handle generation is stale"));
        }
        Ok(())
    }

    fn entry_id(entry: &CachedPipeline) -> u64 {
        match entry {
            CachedPipeline::Wgpu { id, .. } | CachedPipeline::Synthetic { id, .. } => *id,
        }
    }

    /// Resolve by pipeline **id** (not key). Same-key source rebuild must not
    /// let an old handle dispatch the new body.
    fn lookup_by_handle(
        &self,
        handle: &ComputePipelineHandle,
    ) -> Result<&CachedPipeline, DomainError> {
        self.ensure_live(handle.context_id(), handle.generation())?;
        for (key, entry) in &self.entries {
            if Self::entry_id(entry) == handle.id() {
                if key != handle.key() {
                    return Err(inp(
                        "pipeline handle key does not match cached entry",
                        "pipeline",
                    ));
                }
                return Ok(entry);
            }
        }
        Err(inp(
            "pipeline handle is stale or unknown (replaced source or never cached)",
            "pipeline",
        ))
    }
}

/// Observable cache counters for tests/diagnose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipelineCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub entries: usize,
}

impl GpuContext {
    pub(crate) fn pipeline_cache_arc(&self) -> Arc<Mutex<PipelineCache>> {
        Arc::clone(&self.pipeline_cache)
    }

    pub fn pipeline_cache_stats(&self) -> PipelineCacheStats {
        self.pipeline_cache
            .lock()
            .map(|c| PipelineCacheStats {
                hits: c.hits,
                misses: c.misses,
                entries: c.entries.len(),
            })
            .unwrap_or(PipelineCacheStats {
                hits: 0,
                misses: 0,
                entries: 0,
            })
    }

    /// Obtain (create or cache-hit) a compute pipeline for the given source/key.
    pub fn obtain_compute_pipeline(
        &mut self,
        desc: ComputePipelineDesc,
    ) -> Result<ComputePipelineHandle, DomainError> {
        self.require_live_for_pipeline()?;
        let header = parse_shader_header(&desc.source)?;
        if !affine_bind_layout_supported(desc.bind_layout_version) {
            return Err(inp(
                format!(
                    "unsupported bind_layout_version {} (affine supports {} and {})",
                    desc.bind_layout_version,
                    AFFINE_U32_BIND_LAYOUT_VERSION,
                    AFFINE_U32_BIND_LAYOUT_VERSION_V2
                ),
                "bind_layout_version",
            ));
        }
        let digest = source_digest(&desc.source);
        let key = PipelineKey {
            shader_name: header.name.clone(),
            shader_version: header.version,
            entry_point: desc.entry_point.clone(),
            spec: desc.spec.clone().sorted(),
            bind_layout_version: desc.bind_layout_version,
        };

        let mut scope_inject = self.take_scope_inject();
        let mut cache = self.pipeline_cache.lock().expect("pipeline cache");
        cache.ensure_live(self.id(), self.generation())?;

        if let Some(existing) = cache.entries.get(&key) {
            let (id, cached_digest) = match existing {
                CachedPipeline::Wgpu {
                    id, source_digest, ..
                }
                | CachedPipeline::Synthetic {
                    id, source_digest, ..
                } => (*id, *source_digest),
            };
            if cached_digest == digest {
                cache.hits = cache.hits.saturating_add(1);
                drop(cache);
                return Ok(ComputePipelineHandle {
                    inner: Arc::new(PipelineInner {
                        context_id: self.id(),
                        generation: self.generation(),
                        id,
                        key,
                        cache: self.pipeline_cache_arc(),
                    }),
                });
            }
            // Digest differs: rebuild below. Keep the old entry until the new
            // pipeline is fully created so a failed replace cannot drop the
            // prior body, and old handles stay id-addressable until replace.
        }

        cache.misses = cache.misses.saturating_add(1);
        let id = NEXT_PIPELINE_ID.fetch_add(1, Ordering::SeqCst);
        let mode = key
            .spec
            .values
            .iter()
            .find(|(n, _)| n == "MODE")
            .map(|(_, v)| *v)
            .unwrap_or(0);

        match self.backend_device_queue() {
            Some((device, _)) => {
                let module = match wgpu_scoped(
                    &device,
                    crate::diagnostics::ScopeClass::Resource,
                    scope_inject.take(),
                    || {
                        device.create_shader_module(wgpu::ShaderModuleDescriptor {
                            label: Some(desc.label.as_str()),
                            source: wgpu::ShaderSource::Wgsl(desc.source.as_str().into()),
                        })
                    },
                ) {
                    Ok(m) => m,
                    Err(e) => {
                        drop(cache);
                        let err = enrich_shader_error(e, &header.name);
                        return Err(self.fail_gpu(
                            err,
                            crate::diagnostics::GpuOperation::Create,
                            Some(&desc.label),
                            None,
                            None,
                            None,
                        ));
                    }
                };

                // v1 and v2 share the same binding ABI; epoch is a key field only.
                let layout_label =
                    format!("spirula:gpu:bgl:affine-u32-v{}", desc.bind_layout_version);
                let pl_label = format!("spirula:gpu:pl:affine-u32-v{}", desc.bind_layout_version);
                let bind_layout = match wgpu_scoped(
                    &device,
                    crate::diagnostics::ScopeClass::Resource,
                    None,
                    || {
                        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                            label: Some(layout_label.as_str()),
                            entries: &[
                                wgpu::BindGroupLayoutEntry {
                                    binding: 0,
                                    visibility: wgpu::ShaderStages::COMPUTE,
                                    ty: wgpu::BindingType::Buffer {
                                        ty: wgpu::BufferBindingType::Uniform,
                                        has_dynamic_offset: false,
                                        min_binding_size: std::num::NonZeroU64::new(16),
                                    },
                                    count: None,
                                },
                                wgpu::BindGroupLayoutEntry {
                                    binding: 1,
                                    visibility: wgpu::ShaderStages::COMPUTE,
                                    ty: wgpu::BindingType::Buffer {
                                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                                        has_dynamic_offset: false,
                                        min_binding_size: None,
                                    },
                                    count: None,
                                },
                                wgpu::BindGroupLayoutEntry {
                                    binding: 2,
                                    visibility: wgpu::ShaderStages::COMPUTE,
                                    ty: wgpu::BindingType::Buffer {
                                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                                        has_dynamic_offset: false,
                                        min_binding_size: None,
                                    },
                                    count: None,
                                },
                            ],
                        })
                    },
                ) {
                    Ok(l) => l,
                    Err(e) => {
                        drop(cache);
                        let err = enrich_shader_error(e, &header.name);
                        return Err(self.fail_gpu(
                            err,
                            crate::diagnostics::GpuOperation::Create,
                            Some(&desc.label),
                            None,
                            None,
                            None,
                        ));
                    }
                };
                let pipeline_layout = match wgpu_scoped(
                    &device,
                    crate::diagnostics::ScopeClass::Resource,
                    None,
                    || {
                        device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                            label: Some(pl_label.as_str()),
                            bind_group_layouts: &[Some(&bind_layout)],
                            immediate_size: 0,
                        })
                    },
                ) {
                    Ok(l) => l,
                    Err(e) => {
                        drop(cache);
                        let err = enrich_shader_error(e, &header.name);
                        return Err(self.fail_gpu(
                            err,
                            crate::diagnostics::GpuOperation::Create,
                            Some(&desc.label),
                            None,
                            None,
                            None,
                        ));
                    }
                };

                let constants: Vec<(&str, f64)> = key
                    .spec
                    .values
                    .iter()
                    .map(|(n, v)| (n.as_str(), f64::from(*v)))
                    .collect();
                let pipeline = match wgpu_scoped(
                    &device,
                    crate::diagnostics::ScopeClass::Resource,
                    None,
                    || {
                        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                            label: Some(desc.label.as_str()),
                            layout: Some(&pipeline_layout),
                            module: &module,
                            entry_point: Some(desc.entry_point.as_str()),
                            compilation_options: wgpu::PipelineCompilationOptions {
                                constants: &constants,
                                ..Default::default()
                            },
                            cache: None,
                        })
                    },
                ) {
                    Ok(p) => p,
                    Err(e) => {
                        drop(cache);
                        let err = enrich_shader_error(e, &header.name);
                        return Err(self.fail_gpu(
                            err,
                            crate::diagnostics::GpuOperation::Create,
                            Some(&desc.label),
                            None,
                            None,
                            None,
                        ));
                    }
                };

                cache.entries.insert(
                    key.clone(),
                    CachedPipeline::Wgpu {
                        pipeline,
                        bind_layout,
                        source_digest: digest,
                        id,
                    },
                );
            }
            None => {
                cache.entries.insert(
                    key.clone(),
                    CachedPipeline::Synthetic {
                        source_digest: digest,
                        id,
                        mode,
                    },
                );
            }
        }
        drop(cache);

        Ok(ComputePipelineHandle {
            inner: Arc::new(PipelineInner {
                context_id: self.id(),
                generation: self.generation(),
                id,
                key,
                cache: self.pipeline_cache_arc(),
            }),
        })
    }

    /// Convenience: obtain the embedded affine-u32 pipeline with the given MODE.
    pub fn obtain_affine_u32_pipeline(
        &mut self,
        mode: u32,
    ) -> Result<ComputePipelineHandle, DomainError> {
        self.obtain_compute_pipeline(ComputePipelineDesc {
            source: AFFINE_U32_WGSL.to_owned(),
            entry_point: AFFINE_U32_ENTRY.to_owned(),
            spec: SpecConstants::single("MODE", mode),
            bind_layout_version: AFFINE_U32_BIND_LAYOUT_VERSION,
            label: crate::resource::ResourceLabel::parse("spirula:gpu:pipeline:affine-u32")
                .expect("label"),
        })
    }

    /// Encode affine-u32 dispatch (pending until submit).
    ///
    /// `count` is the launch extent (workgroups cover `0..count`). The uniform
    /// `Params.count` is the shader write guard: only indices `i < Params.count`
    /// are written (CPU / synthetic / host agree). Buffers must be sized for the
    /// launch extent; elements beyond `Params.count` are left unchanged.
    pub fn dispatch_affine_u32(
        &mut self,
        pipeline: &ComputePipelineHandle,
        params_buf: &BufferHandle,
        input: &BufferHandle,
        output: &BufferHandle,
        count: u32,
    ) -> Result<(), DomainError> {
        self.require_live_for_pipeline()?;
        if count == 0 {
            return Ok(());
        }
        let bytes = u64::from(count)
            .checked_mul(4)
            .ok_or_else(|| range_err("affine count*4 overflow"))?;
        check_copy_buffer_align(0, bytes)?;
        let _ = COPY_BUFFER_ALIGNMENT;

        validate_affine_aliases(params_buf, input, output)?;

        let limits = self.report().map(|r| r.device_limits);
        let max_wg = limits
            .map(|l| l.max_compute_workgroups_per_dimension)
            .unwrap_or(65535);
        let max_uniform = limits
            .map(|l| l.max_uniform_buffer_binding_size)
            .unwrap_or(64 << 10);
        let max_storage = limits
            .map(|l| l.max_storage_buffer_binding_size)
            .unwrap_or(128 << 20);
        let groups = count.div_ceil(AFFINE_U32_WORKGROUP_SIZE);
        if groups == 0 || groups > max_wg {
            return Err(range_err(format!(
                "dispatch workgroups {groups} exceeds device max_compute_workgroups_per_dimension {max_wg}"
            )));
        }
        if AFFINE_U32_PARAMS_BYTES > max_uniform {
            return Err(range_err(format!(
                "affine params bind size {AFFINE_U32_PARAMS_BYTES} exceeds max_uniform_buffer_binding_size {max_uniform}"
            )));
        }
        if bytes > max_storage {
            return Err(range_err(format!(
                "affine storage bind size {bytes} exceeds max_storage_buffer_binding_size {max_storage}"
            )));
        }

        let scope_inject = self.take_scope_inject();
        if let Some(inj) = scope_inject {
            // Deterministic probe on both host and synthetic — preserve typed code.
            return Err(self.fail_gpu(
                inj.into_domain_error(crate::diagnostics::ScopeClass::Resource),
                crate::diagnostics::GpuOperation::Dispatch,
                None,
                None,
                None,
                None,
            ));
        }
        let is_synthetic = self.backend_device_queue().is_none();
        if is_synthetic {
            return self.dispatch_affine_synthetic(pipeline, params_buf, input, output, count);
        }

        let (device, _) = self
            .backend_device_queue()
            .ok_or_else(|| gpu_lost("device missing during dispatch"))?;

        let cache = self.pipeline_cache.lock().expect("pipeline cache");
        let (gpu_pipe, bind_layout) = match cache.lookup_by_handle(pipeline)? {
            CachedPipeline::Wgpu {
                pipeline: p,
                bind_layout,
                ..
            } => (p.clone(), bind_layout.clone()),
            CachedPipeline::Synthetic { .. } => {
                return Err(inp("pipeline not in wgpu cache", "pipeline"));
            }
        };
        drop(cache);

        let mut led = self.resource_ledger.lock().expect("ledger");
        led.ensure_live(pipeline.context_id(), pipeline.generation())?;
        led.ensure_live(params_buf.context_id(), params_buf.generation())?;
        led.ensure_live(input.context_id(), input.generation())?;
        led.ensure_live(output.context_id(), output.generation())?;

        validate_affine_buffers(&led, params_buf, input, output, bytes)?;

        let params_b = led
            .wgpu_buffer(&params_buf.id())
            .ok_or_else(|| gpu_lost("params gpu missing"))?;
        let in_b = led
            .wgpu_buffer(&input.id())
            .ok_or_else(|| gpu_lost("input gpu missing"))?;
        let out_b = led
            .wgpu_buffer(&output.id())
            .ok_or_else(|| gpu_lost("output gpu missing"))?;

        let params_size = std::num::NonZeroU64::new(AFFINE_U32_PARAMS_BYTES).expect("16");
        let storage_size = std::num::NonZeroU64::new(bytes).expect("count*4");

        // Narrow ranges: bind Params as 16B and storage as count*4 so a larger
        // buffer allocation cannot exceed enabled max_*_buffer_binding_size.
        // Error-scope bind/encode/finish so validation cannot panic exit 101.
        let finished = match wgpu_scoped(
            &device,
            crate::diagnostics::ScopeClass::Resource,
            None,
            || {
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("spirula:gpu:bg:affine-u32"),
                    layout: &bind_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer: &params_b,
                                offset: 0,
                                size: Some(params_size),
                            }),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer: &in_b,
                                offset: 0,
                                size: Some(storage_size),
                            }),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer: &out_b,
                                offset: 0,
                                size: Some(storage_size),
                            }),
                        },
                    ],
                });
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("spirula:gpu:encoder:affine-u32"),
                });
                {
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("spirula:gpu:compute:affine-u32"),
                        timestamp_writes: None,
                    });
                    pass.set_pipeline(&gpu_pipe);
                    pass.set_bind_group(0, &bind_group, &[]);
                    pass.dispatch_workgroups(groups, 1, 1);
                }
                encoder.finish()
            },
        ) {
            Ok(cmd) => cmd,
            Err(e) => {
                // No pending_commands / note_pending on failure.
                // Keep typed GPU identity (SP-GPU-004 OOM, SP-GPU-003 submit-class,
                // SP-INP-003 resource validation) — do not remap all scopes to input.
                drop(led);
                return Err(self.fail_gpu(
                    e,
                    crate::diagnostics::GpuOperation::Dispatch,
                    None,
                    None,
                    None,
                    None,
                ));
            }
        };

        led.pending_commands
            .push(crate::resource::PendingGpuCommand {
                command: finished,
                staging_id: None,
            });
        led.note_pending_buffer(params_buf.id());
        led.note_pending_buffer(input.id());
        led.note_pending_buffer(output.id());
        Ok(())
    }

    fn dispatch_affine_synthetic(
        &mut self,
        pipeline: &ComputePipelineHandle,
        params_buf: &BufferHandle,
        input: &BufferHandle,
        output: &BufferHandle,
        count: u32,
    ) -> Result<(), DomainError> {
        let bytes = u64::from(count) * 4;
        let cache = self.pipeline_cache.lock().expect("pipeline cache");
        let mode = match cache.lookup_by_handle(pipeline)? {
            CachedPipeline::Synthetic { mode, .. } => *mode,
            CachedPipeline::Wgpu { .. } => {
                return Err(inp("pipeline not in synthetic cache", "pipeline"));
            }
        };
        drop(cache);

        let mut led = self.resource_ledger.lock().expect("ledger");
        led.ensure_live(pipeline.context_id(), pipeline.generation())?;
        led.ensure_live(params_buf.context_id(), params_buf.generation())?;
        led.ensure_live(input.context_id(), input.generation())?;
        led.ensure_live(output.context_id(), output.generation())?;
        validate_affine_buffers(&led, params_buf, input, output, bytes)?;

        let (scale, bias, params_count) = {
            let v = match led
                .buffers
                .get(&params_buf.id())
                .and_then(|e| e.gpu.as_ref())
            {
                Some(GpuBuffer::Synthetic(v)) => v,
                _ => return Err(gpu_lost("params synthetic missing")),
            };
            if v.len() < AFFINE_U32_PARAMS_BYTES as usize {
                return Err(range_err("params synthetic too small"));
            }
            (
                u32::from_le_bytes(v[0..4].try_into().unwrap()),
                u32::from_le_bytes(v[4..8].try_into().unwrap()),
                u32::from_le_bytes(v[8..12].try_into().unwrap()),
            )
        };
        // Match WGSL: write only i < Params.count within the launch extent.
        let write_count = count.min(params_count) as usize;
        let input_words = {
            let v = match led.buffers.get(&input.id()).and_then(|e| e.gpu.as_ref()) {
                Some(GpuBuffer::Synthetic(v)) => v,
                _ => return Err(gpu_lost("input synthetic missing")),
            };
            let mut words = Vec::with_capacity(write_count);
            for i in 0..write_count {
                let o = i * 4;
                words.push(u32::from_le_bytes(v[o..o + 4].try_into().unwrap()));
            }
            words
        };
        let out_words = cpu_affine_u32(&input_words, scale, bias, mode);
        if let Some(GpuBuffer::Synthetic(v)) = led
            .buffers
            .get_mut(&output.id())
            .and_then(|e| e.gpu.as_mut())
        {
            for (i, w) in out_words.iter().enumerate() {
                let o = i * 4;
                v[o..o + 4].copy_from_slice(&w.to_le_bytes());
            }
            // Indices in [write_count, count) remain unchanged (shader guard).
        }
        led.note_pending_buffer(params_buf.id());
        led.note_pending_buffer(input.id());
        led.note_pending_buffer(output.id());
        Ok(())
    }

    fn require_live_for_pipeline(&mut self) -> Result<(), DomainError> {
        self.poll_host_lost();
        if self.status() != ContextStatus::Live {
            return Err(gpu_lost(match self.status() {
                ContextStatus::Lost => "device lost; pipelines unavailable",
                ContextStatus::Closed => "context closed; pipelines unavailable",
                ContextStatus::CreateFailed => "create failed; pipelines unavailable",
                ContextStatus::Live => "context not live; pipelines unavailable",
            }));
        }
        Ok(())
    }
}

fn validate_affine_aliases(
    params_buf: &BufferHandle,
    input: &BufferHandle,
    output: &BufferHandle,
) -> Result<(), DomainError> {
    if input.id() == output.id() {
        return Err(inp(
            "affine input and output must not alias the same buffer (STORAGE_READ vs STORAGE_READ_WRITE)",
            "buffer",
        ));
    }
    if params_buf.id() == input.id() || params_buf.id() == output.id() {
        return Err(inp(
            "affine params must not alias input/output storage buffers",
            "buffer",
        ));
    }
    Ok(())
}

fn validate_affine_buffers(
    led: &crate::resource::ResourceLedger,
    params_buf: &BufferHandle,
    input: &BufferHandle,
    output: &BufferHandle,
    bytes: u64,
) -> Result<(), DomainError> {
    let params_entry = led
        .buffers
        .get(&params_buf.id())
        .ok_or_else(|| inp("unknown params buffer", "buffer"))?;
    if !params_entry.usage.uniform {
        return Err(inp("params buffer missing uniform usage", "usage"));
    }
    if params_entry.size < AFFINE_U32_PARAMS_BYTES {
        return Err(range_err("params buffer must be at least 16 bytes"));
    }

    let in_entry = led
        .buffers
        .get(&input.id())
        .ok_or_else(|| inp("unknown input buffer", "buffer"))?;
    if !in_entry.usage.storage {
        return Err(inp("input buffer missing storage usage", "usage"));
    }
    if in_entry.size < bytes {
        return Err(range_err("input buffer smaller than count*4"));
    }

    let out_entry = led
        .buffers
        .get(&output.id())
        .ok_or_else(|| inp("unknown output buffer", "buffer"))?;
    if !out_entry.usage.storage {
        return Err(inp("output buffer missing storage usage", "usage"));
    }
    if out_entry.size < bytes {
        return Err(range_err("output buffer smaller than count*4"));
    }
    Ok(())
}

fn enrich_shader_error(err: DomainError, name: &str) -> DomainError {
    if err.code() == ErrorCode::AllocationFailed {
        return err;
    }
    let msg = err.message().to_owned();
    let line = parse_wgsl_error_line(&msg).unwrap_or(1);
    shader_err(format!("shader '{name}' compile/pipeline failed: {msg}"))
        .with_context(ContextKey::Path, format!("shaders/{name}.wgsl"))
        .expect("path once")
        .with_context(ContextKey::Line, line.to_string())
        .expect("line once")
}

fn parse_wgsl_error_line(msg: &str) -> Option<u32> {
    if let Some(i) = msg.find("line ") {
        let rest = &msg[i + 5..];
        let n: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(line) = n.parse::<u32>() {
            return Some(line);
        }
    }
    for token in msg.split(|c: char| c.is_whitespace() || c == ',' || c == '(' || c == ')') {
        let parts: Vec<&str> = token.split(':').collect();
        if parts.len() >= 2
            && let Ok(line) = parts[parts.len().saturating_sub(2)].parse::<u32>()
            && line > 0
            && line < 100_000
        {
            return Some(line);
        }
    }
    None
}
