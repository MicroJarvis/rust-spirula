//! P2-F03: narrow buffer/texture handles, staging upload/readback, byte budget.
//!
//! Not a general wgpu mirror. Labels, sizes, usages, and ranges are checked in
//! Rust before any wgpu call.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use spirula_core::{AccountedBytesTracker, ContextKey, DomainError, ErrorCode, UserAction};

use crate::context::{ContextGeneration, ContextId, ContextStatus, GpuContext};

/// wgpu copy row pitch alignment (bytes).
pub const COPY_BYTES_PER_ROW_ALIGNMENT: u32 = 256;

/// Counter source for scoped GPU resource accounting (not device-wide VRAM).
pub const RESOURCE_ACCOUNT_SOURCE: &str = "accounted_scoped:gpu_context_resources";

static NEXT_RESOURCE_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_SUBMISSION_ID: AtomicU64 = AtomicU64::new(1);

/// Validated `spirula:<domain>:<kind>:<name>` label.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResourceLabel(String);

impl ResourceLabel {
    pub fn parse(raw: &str) -> Result<Self, DomainError> {
        let parts: Vec<&str> = raw.split(':').collect();
        if parts.len() != 4 || parts[0] != "spirula" {
            return Err(inp(
                "resource label must be spirula:<domain>:<kind>:<name>",
                "label",
            ));
        }
        for (i, p) in parts.iter().enumerate() {
            if p.is_empty()
                || !p
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            {
                return Err(inp(
                    format!("resource label segment {i} must be lowercase kebab"),
                    "label",
                ));
            }
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ResourceLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Narrow buffer usage for this slice (copy paths only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferUsage {
    pub copy_src: bool,
    pub copy_dst: bool,
}

impl BufferUsage {
    pub const COPY_SRC: Self = Self {
        copy_src: true,
        copy_dst: false,
    };
    pub const COPY_DST: Self = Self {
        copy_src: false,
        copy_dst: true,
    };
    pub const COPY_SRC_DST: Self = Self {
        copy_src: true,
        copy_dst: true,
    };

    fn to_wgpu(self) -> wgpu::BufferUsages {
        let mut u = wgpu::BufferUsages::empty();
        if self.copy_src {
            u |= wgpu::BufferUsages::COPY_SRC;
        }
        if self.copy_dst {
            u |= wgpu::BufferUsages::COPY_DST;
        }
        u
    }
}

/// Narrow texture usage for this slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureUsage {
    pub copy_src: bool,
    pub copy_dst: bool,
}

impl TextureUsage {
    pub const COPY_SRC_DST: Self = Self {
        copy_src: true,
        copy_dst: true,
    };

    fn to_wgpu(self) -> wgpu::TextureUsages {
        let mut u = wgpu::TextureUsages::empty();
        if self.copy_src {
            u |= wgpu::TextureUsages::COPY_SRC;
        }
        if self.copy_dst {
            u |= wgpu::TextureUsages::COPY_DST;
        }
        u
    }
}

/// Sole measurable 2D format in P2-F03.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureFormat {
    Rgba8Unorm,
}

impl TextureFormat {
    pub const fn bytes_per_pixel(self) -> u32 {
        4
    }
}

#[derive(Debug, Clone)]
pub struct BufferDesc {
    pub label: ResourceLabel,
    pub size: u64,
    pub usage: BufferUsage,
}

#[derive(Debug, Clone)]
pub struct Texture2dDesc {
    pub label: ResourceLabel,
    pub width: u32,
    pub height: u32,
    pub format: TextureFormat,
    pub usage: TextureUsage,
}

/// Opaque buffer handle. `Clone` shares one allocation (no double accounting).
#[derive(Clone)]
pub struct BufferHandle {
    inner: Arc<BufferInner>,
}

struct BufferInner {
    context_id: ContextId,
    generation: ContextGeneration,
    id: u64,
    ledger: Arc<Mutex<ResourceLedger>>,
}

impl BufferHandle {
    pub fn context_id(&self) -> ContextId {
        self.inner.context_id
    }
    pub fn generation(&self) -> ContextGeneration {
        self.inner.generation
    }
    pub fn id(&self) -> u64 {
        self.inner.id
    }
}

impl Drop for BufferInner {
    fn drop(&mut self) {
        if let Ok(mut led) = self.ledger.lock() {
            led.on_buffer_handle_dropped(self.id);
        }
    }
}

/// Opaque texture handle. `Clone` shares one allocation.
#[derive(Clone)]
pub struct TextureHandle {
    inner: Arc<TextureInner>,
}

struct TextureInner {
    context_id: ContextId,
    generation: ContextGeneration,
    id: u64,
    ledger: Arc<Mutex<ResourceLedger>>,
}

impl TextureHandle {
    pub fn context_id(&self) -> ContextId {
        self.inner.context_id
    }
    pub fn generation(&self) -> ContextGeneration {
        self.inner.generation
    }
    pub fn id(&self) -> u64 {
        self.inner.id
    }
}

impl Drop for TextureInner {
    fn drop(&mut self) {
        if let Ok(mut led) = self.ledger.lock() {
            led.on_texture_handle_dropped(self.id);
        }
    }
}

/// Explicit GPU submission token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubmissionHandle {
    context_id: ContextId,
    generation: ContextGeneration,
    id: u64,
}

impl SubmissionHandle {
    pub fn context_id(self) -> ContextId {
        self.context_id
    }
    pub fn generation(self) -> ContextGeneration {
        self.generation
    }
    pub fn id(self) -> u64 {
        self.id
    }
}

/// Ticket for mapping a completed readback staging buffer.
#[derive(Debug)]
pub struct ReadbackTicket {
    context_id: ContextId,
    generation: ContextGeneration,
    staging_id: u64,
    logical_len: u64,
    /// Texture strip: bytes_per_row and height; None = raw buffer.
    texture_pitch: Option<(u32, u32)>,
}

#[derive(Debug, Clone)]
pub struct ResourceAccountingSnapshot {
    pub source: &'static str,
    pub allocated_bytes: u64,
    pub current_bytes: u64,
    pub peak_bytes: u64,
    pub budget_limit: Option<u64>,
}

pub fn rgba8_logical_bytes(width: u32, height: u32) -> Result<u64, DomainError> {
    let w = u64::from(width);
    let h = u64::from(height);
    w.checked_mul(h)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| range_err("texture logical byte size overflow"))
}

pub fn rgba8_bytes_per_row(width: u32) -> Result<u32, DomainError> {
    let raw = u64::from(width)
        .checked_mul(4)
        .ok_or_else(|| range_err("texture row byte size overflow"))?;
    let align = u64::from(COPY_BYTES_PER_ROW_ALIGNMENT);
    let pitched = raw
        .checked_add(align - 1)
        .ok_or_else(|| range_err("texture row pitch overflow"))?
        / align
        * align;
    u32::try_from(pitched).map_err(|_| range_err("texture row pitch exceeds u32"))
}

pub fn rgba8_padded_bytes(width: u32, height: u32) -> Result<u64, DomainError> {
    let bpr = u64::from(rgba8_bytes_per_row(width)?);
    bpr.checked_mul(u64::from(height))
        .ok_or_else(|| range_err("texture padded byte size overflow"))
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

fn submit_err(msg: impl Into<String>) -> DomainError {
    DomainError::try_new(ErrorCode::SubmitFailed, msg)
        .expect("non-empty")
        .with_user_action(UserAction::Retry)
}

enum GpuBuffer {
    Wgpu(wgpu::Buffer),
    Synthetic(Vec<u8>),
}

enum GpuTexture {
    Wgpu(wgpu::Texture),
    Synthetic {
        #[allow(dead_code)]
        width: u32,
        #[allow(dead_code)]
        height: u32,
        bytes_per_row: u32,
        data: Vec<u8>,
    },
}

struct BufferEntry {
    #[allow(dead_code)]
    label: ResourceLabel,
    size: u64,
    usage: BufferUsage,
    accounted: u64,
    handle_refs: u32,
    inflight: u32,
    destroy_pending: bool,
    gpu: Option<GpuBuffer>,
}

struct TextureEntry {
    #[allow(dead_code)]
    label: ResourceLabel,
    width: u32,
    height: u32,
    #[allow(dead_code)]
    format: TextureFormat,
    usage: TextureUsage,
    accounted: u64,
    handle_refs: u32,
    inflight: u32,
    destroy_pending: bool,
    gpu: Option<GpuTexture>,
}

struct StagingEntry {
    accounted: u64,
    kind: StagingKind,
    gpu: Option<GpuBuffer>,
    /// Set after wait completes; map consumes.
    mapped_ready: bool,
}

enum StagingKind {
    Upload,
    Readback {
        #[allow(dead_code)]
        logical_len: u64,
        #[allow(dead_code)]
        texture_pitch: Option<(u32, u32)>,
    },
}

struct SubmissionEntry {
    staging_ids: Vec<u64>,
    buffer_ids: Vec<u64>,
    texture_ids: Vec<u64>,
    wgpu_index: Option<wgpu::SubmissionIndex>,
    completed: bool,
}

pub(crate) struct ResourceLedger {
    pub tracker: AccountedBytesTracker,
    pub budget_limit: Option<u64>,
    context_id: ContextId,
    generation: ContextGeneration,
    invalidated: bool,
    buffers: HashMap<u64, BufferEntry>,
    textures: HashMap<u64, TextureEntry>,
    stagings: HashMap<u64, StagingEntry>,
    submissions: HashMap<u64, SubmissionEntry>,
    /// Pending encoder command buffers waiting for submit (wgpu path).
    pending_commands: Vec<wgpu::CommandBuffer>,
    pending_staging: Vec<u64>,
    pending_buffers: Vec<u64>,
    pending_textures: Vec<u64>,
}

impl ResourceLedger {
    pub(crate) fn new(context_id: ContextId, generation: ContextGeneration) -> Self {
        Self {
            tracker: AccountedBytesTracker::new(),
            budget_limit: None,
            context_id,
            generation,
            invalidated: false,
            buffers: HashMap::new(),
            textures: HashMap::new(),
            stagings: HashMap::new(),
            submissions: HashMap::new(),
            pending_commands: Vec::new(),
            pending_staging: Vec::new(),
            pending_buffers: Vec::new(),
            pending_textures: Vec::new(),
        }
    }

    pub(crate) fn snapshot(&self) -> ResourceAccountingSnapshot {
        ResourceAccountingSnapshot {
            source: RESOURCE_ACCOUNT_SOURCE,
            allocated_bytes: self.tracker.allocated_bytes(),
            current_bytes: self.tracker.current_bytes(),
            peak_bytes: self.tracker.peak_bytes(),
            budget_limit: self.budget_limit,
        }
    }

    pub(crate) fn set_budget_limit(&mut self, limit: Option<u64>) {
        self.budget_limit = limit;
    }

    pub(crate) fn invalidate_all(&mut self) {
        self.invalidated = true;
        self.pending_commands.clear();
        self.pending_staging.clear();
        self.pending_buffers.clear();
        self.pending_textures.clear();
        // Drop GPU objects; roll back remaining accounted bytes.
        for (_, mut b) in self.buffers.drain() {
            b.gpu = None;
            let _ = self.tracker.free(b.accounted);
        }
        for (_, mut t) in self.textures.drain() {
            t.gpu = None;
            let _ = self.tracker.free(t.accounted);
        }
        for (_, mut s) in self.stagings.drain() {
            s.gpu = None;
            let _ = self.tracker.free(s.accounted);
        }
        self.submissions.clear();
    }

    fn ensure_live(
        &self,
        ctx_id: ContextId,
        generation: ContextGeneration,
    ) -> Result<(), DomainError> {
        if self.invalidated {
            return Err(gpu_lost("resource ledger invalidated (closed/lost/reopen)"));
        }
        if ctx_id != self.context_id {
            return Err(inp(
                "resource handle belongs to a different context",
                "resource_handle",
            ));
        }
        if generation != self.generation {
            return Err(gpu_lost("resource handle generation is stale"));
        }
        Ok(())
    }

    fn preflight_or_err(&self, bytes: u64) -> Result<(), DomainError> {
        if let Some(limit) = self.budget_limit {
            self.tracker.preflight(bytes, limit)?;
        } else {
            // Still catch u64 overflow via a huge synthetic limit path: allocate path checks.
            let _ = self
                .tracker
                .current_bytes()
                .checked_add(bytes)
                .ok_or_else(|| range_err("accounted current_bytes would overflow u64"))?;
        }
        Ok(())
    }

    fn reserve(&mut self, bytes: u64) -> Result<(), DomainError> {
        self.preflight_or_err(bytes)?;
        self.tracker.allocate(bytes)
    }

    fn on_buffer_handle_dropped(&mut self, id: u64) {
        let Some(entry) = self.buffers.get_mut(&id) else {
            return;
        };
        entry.handle_refs = entry.handle_refs.saturating_sub(1);
        if entry.handle_refs == 0 {
            entry.destroy_pending = true;
            if entry.inflight == 0 {
                self.reclaim_buffer(id);
            }
        }
    }

    fn on_texture_handle_dropped(&mut self, id: u64) {
        let Some(entry) = self.textures.get_mut(&id) else {
            return;
        };
        entry.handle_refs = entry.handle_refs.saturating_sub(1);
        if entry.handle_refs == 0 {
            entry.destroy_pending = true;
            if entry.inflight == 0 {
                self.reclaim_texture(id);
            }
        }
    }

    fn reclaim_buffer(&mut self, id: u64) {
        if let Some(mut entry) = self.buffers.remove(&id) {
            entry.gpu = None;
            let _ = self.tracker.free(entry.accounted);
        }
    }

    fn reclaim_texture(&mut self, id: u64) {
        if let Some(mut entry) = self.textures.remove(&id) {
            entry.gpu = None;
            let _ = self.tracker.free(entry.accounted);
        }
    }
}

impl GpuContext {
    pub(crate) fn ledger_arc(&self) -> Arc<Mutex<ResourceLedger>> {
        Arc::clone(&self.resource_ledger)
    }

    /// Optional scoped resident budget for GPU resources + staging (P1-F08 style).
    pub fn set_resource_budget(&mut self, limit_bytes: Option<u64>) {
        if let Ok(mut led) = self.resource_ledger.lock() {
            led.set_budget_limit(limit_bytes);
        }
    }

    pub fn resource_accounting(&self) -> ResourceAccountingSnapshot {
        self.resource_ledger
            .lock()
            .map(|l| l.snapshot())
            .unwrap_or(ResourceAccountingSnapshot {
                source: RESOURCE_ACCOUNT_SOURCE,
                allocated_bytes: 0,
                current_bytes: 0,
                peak_bytes: 0,
                budget_limit: None,
            })
    }

    pub fn create_buffer(&mut self, desc: BufferDesc) -> Result<BufferHandle, DomainError> {
        self.require_live()?;
        if desc.size == 0 {
            return Err(range_err("buffer size must be > 0"));
        }
        if !desc.usage.copy_src && !desc.usage.copy_dst {
            return Err(inp(
                "buffer usage must include copy_src and/or copy_dst",
                "usage",
            ));
        }
        let max = self
            .report()
            .map(|r| r.device_limits.max_buffer_size)
            .unwrap_or(u64::MAX);
        if desc.size > max {
            return Err(range_err(format!(
                "buffer size {} exceeds device max_buffer_size {max}",
                desc.size
            )));
        }

        let mut led = self.resource_ledger.lock().expect("ledger");
        led.reserve(desc.size)?;
        let id = NEXT_RESOURCE_ID.fetch_add(1, Ordering::SeqCst);
        let gpu = match self.backend_device_queue() {
            Some((device, _)) => {
                let buf = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(desc.label.as_str()),
                    size: desc.size,
                    usage: desc.usage.to_wgpu(),
                    mapped_at_creation: false,
                });
                GpuBuffer::Wgpu(buf)
            }
            None => GpuBuffer::Synthetic(vec![0u8; desc.size as usize]),
        };
        led.buffers.insert(
            id,
            BufferEntry {
                label: desc.label.clone(),
                size: desc.size,
                usage: desc.usage,
                accounted: desc.size,
                handle_refs: 1,
                inflight: 0,
                destroy_pending: false,
                gpu: Some(gpu),
            },
        );
        drop(led);

        Ok(BufferHandle {
            inner: Arc::new(BufferInner {
                context_id: self.id(),
                generation: self.generation(),
                id,
                ledger: self.ledger_arc(),
            }),
        })
    }

    pub fn create_texture_2d(&mut self, desc: Texture2dDesc) -> Result<TextureHandle, DomainError> {
        self.require_live()?;
        if desc.width == 0 || desc.height == 0 {
            return Err(range_err("texture width/height must be > 0"));
        }
        if !matches!(desc.format, TextureFormat::Rgba8Unorm) {
            return Err(inp(
                "only TextureFormat::Rgba8Unorm is supported in P2-F03",
                "format",
            ));
        }
        if !desc.usage.copy_src || !desc.usage.copy_dst {
            return Err(inp(
                "texture usage must include copy_src and copy_dst for upload/readback",
                "usage",
            ));
        }
        let accounted = rgba8_padded_bytes(desc.width, desc.height)?;
        let bpr = rgba8_bytes_per_row(desc.width)?;

        let mut led = self.resource_ledger.lock().expect("ledger");
        led.reserve(accounted)?;
        let id = NEXT_RESOURCE_ID.fetch_add(1, Ordering::SeqCst);
        let gpu = match self.backend_device_queue() {
            Some((device, _)) => {
                let tex = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(desc.label.as_str()),
                    size: wgpu::Extent3d {
                        width: desc.width,
                        height: desc.height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: desc.usage.to_wgpu(),
                    view_formats: &[],
                });
                GpuTexture::Wgpu(tex)
            }
            None => GpuTexture::Synthetic {
                width: desc.width,
                height: desc.height,
                bytes_per_row: bpr,
                data: vec![0u8; accounted as usize],
            },
        };
        led.textures.insert(
            id,
            TextureEntry {
                label: desc.label.clone(),
                width: desc.width,
                height: desc.height,
                format: desc.format,
                usage: desc.usage,
                accounted,
                handle_refs: 1,
                inflight: 0,
                destroy_pending: false,
                gpu: Some(gpu),
            },
        );
        drop(led);

        Ok(TextureHandle {
            inner: Arc::new(TextureInner {
                context_id: self.id(),
                generation: self.generation(),
                id,
                ledger: self.ledger_arc(),
            }),
        })
    }

    /// Stage host→device buffer copy (pending until [`Self::submit`]).
    pub fn upload_buffer(
        &mut self,
        handle: &BufferHandle,
        offset: u64,
        data: &[u8],
    ) -> Result<(), DomainError> {
        self.require_live()?;
        let mut led = self.resource_ledger.lock().expect("ledger");
        led.ensure_live(handle.context_id(), handle.generation())?;
        let entry = led
            .buffers
            .get(&handle.id())
            .ok_or_else(|| inp("unknown buffer handle", "buffer"))?;
        if !entry.usage.copy_dst {
            return Err(inp("buffer missing copy_dst for upload", "usage"));
        }
        let end = offset
            .checked_add(data.len() as u64)
            .ok_or_else(|| range_err("upload offset+len overflow"))?;
        if end > entry.size {
            return Err(range_err("upload range exceeds buffer size"));
        }
        let size = data.len() as u64;
        led.reserve(size)?;
        let staging_id = NEXT_RESOURCE_ID.fetch_add(1, Ordering::SeqCst);

        match self.backend_device_queue() {
            Some((device, queue)) => {
                let staging = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("spirula:gpu:buffer:upload-staging"),
                    size,
                    usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::MAP_WRITE,
                    mapped_at_creation: true,
                });
                {
                    let mut view = staging
                        .slice(..)
                        .get_mapped_range_mut()
                        .map_err(|e| submit_err(format!("map upload staging failed: {e}")))?;
                    view.copy_from_slice(data);
                }
                staging.unmap();

                let dst = match led.buffers.get(&handle.id()).and_then(|e| e.gpu.as_ref()) {
                    Some(GpuBuffer::Wgpu(b)) => b,
                    _ => {
                        let _ = led.tracker.free(size);
                        return Err(gpu_lost("buffer gpu storage missing"));
                    }
                };
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("spirula:gpu:encoder:upload-buffer"),
                });
                encoder.copy_buffer_to_buffer(&staging, 0, dst, offset, size);
                led.pending_commands.push(encoder.finish());
                // Keep staging alive until submit/wait — store it.
                led.stagings.insert(
                    staging_id,
                    StagingEntry {
                        accounted: size,
                        kind: StagingKind::Upload,
                        gpu: Some(GpuBuffer::Wgpu(staging)),
                        mapped_ready: false,
                    },
                );
                let _ = queue; // copies submitted later
            }
            None => {
                // Synthetic: copy into host vec immediately into pending staging mirror.
                let mut staging_data = data.to_vec();
                if let Some(BufferEntry {
                    gpu: Some(GpuBuffer::Synthetic(dst)),
                    ..
                }) = led.buffers.get_mut(&handle.id())
                {
                    let start = offset as usize;
                    let end = start + data.len();
                    dst[start..end].copy_from_slice(&staging_data);
                    staging_data.clear();
                }
                led.stagings.insert(
                    staging_id,
                    StagingEntry {
                        accounted: size,
                        kind: StagingKind::Upload,
                        gpu: Some(GpuBuffer::Synthetic(vec![])),
                        mapped_ready: false,
                    },
                );
            }
        }
        led.pending_staging.push(staging_id);
        led.pending_buffers.push(handle.id());
        Ok(())
    }

    /// Stage packed RGBA8 host pixels → texture (pending until submit).
    /// `pixels` must be logical `width*height*4` tightly packed rows.
    pub fn upload_texture_rgba8(
        &mut self,
        handle: &TextureHandle,
        pixels: &[u8],
    ) -> Result<(), DomainError> {
        self.require_live()?;
        let mut led = self.resource_ledger.lock().expect("ledger");
        led.ensure_live(handle.context_id(), handle.generation())?;
        let (width, height, usage_ok) = {
            let e = led
                .textures
                .get(&handle.id())
                .ok_or_else(|| inp("unknown texture handle", "texture"))?;
            (e.width, e.height, e.usage.copy_dst)
        };
        if !usage_ok {
            return Err(inp("texture missing copy_dst for upload", "usage"));
        }
        let logical = rgba8_logical_bytes(width, height)?;
        if pixels.len() as u64 != logical {
            return Err(range_err(format!(
                "texture upload expects {logical} tightly packed bytes, got {}",
                pixels.len()
            )));
        }
        let bpr = rgba8_bytes_per_row(width)?;
        let padded = rgba8_padded_bytes(width, height)?;
        led.reserve(padded)?;
        let staging_id = NEXT_RESOURCE_ID.fetch_add(1, Ordering::SeqCst);

        // Expand tight rows into padded staging layout.
        let mut padded_bytes = vec![0u8; padded as usize];
        let row_logical = (width * 4) as usize;
        let row_pitch = bpr as usize;
        for y in 0..height as usize {
            let src = y * row_logical;
            let dst = y * row_pitch;
            padded_bytes[dst..dst + row_logical].copy_from_slice(&pixels[src..src + row_logical]);
        }

        match self.backend_device_queue() {
            Some((device, _queue)) => {
                let staging = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("spirula:gpu:buffer:tex-upload-staging"),
                    size: padded,
                    usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::MAP_WRITE,
                    mapped_at_creation: true,
                });
                {
                    let mut view = staging.slice(..).get_mapped_range_mut().map_err(|e| {
                        submit_err(format!("map texture upload staging failed: {e}"))
                    })?;
                    view.copy_from_slice(&padded_bytes);
                }
                staging.unmap();

                let tex = match led.textures.get(&handle.id()).and_then(|e| e.gpu.as_ref()) {
                    Some(GpuTexture::Wgpu(t)) => t,
                    _ => {
                        let _ = led.tracker.free(padded);
                        return Err(gpu_lost("texture gpu storage missing"));
                    }
                };
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("spirula:gpu:encoder:upload-texture"),
                });
                encoder.copy_buffer_to_texture(
                    wgpu::TexelCopyBufferInfo {
                        buffer: &staging,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(bpr),
                            rows_per_image: Some(height),
                        },
                    },
                    wgpu::TexelCopyTextureInfo {
                        texture: tex,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                );
                led.pending_commands.push(encoder.finish());
                led.stagings.insert(
                    staging_id,
                    StagingEntry {
                        accounted: padded,
                        kind: StagingKind::Upload,
                        gpu: Some(GpuBuffer::Wgpu(staging)),
                        mapped_ready: false,
                    },
                );
            }
            None => {
                if let Some(TextureEntry {
                    gpu:
                        Some(GpuTexture::Synthetic {
                            data,
                            bytes_per_row,
                            ..
                        }),
                    ..
                }) = led.textures.get_mut(&handle.id())
                {
                    let pitch = *bytes_per_row as usize;
                    for y in 0..height as usize {
                        let src = y * row_logical;
                        let dst = y * pitch;
                        data[dst..dst + row_logical]
                            .copy_from_slice(&pixels[src..src + row_logical]);
                    }
                }
                led.stagings.insert(
                    staging_id,
                    StagingEntry {
                        accounted: padded,
                        kind: StagingKind::Upload,
                        gpu: Some(GpuBuffer::Synthetic(vec![])),
                        mapped_ready: false,
                    },
                );
            }
        }
        led.pending_staging.push(staging_id);
        led.pending_textures.push(handle.id());
        Ok(())
    }

    /// Encode device→staging readback for a buffer range; map after wait.
    pub fn readback_buffer(
        &mut self,
        handle: &BufferHandle,
        offset: u64,
        size: u64,
    ) -> Result<ReadbackTicket, DomainError> {
        self.require_live()?;
        let mut led = self.resource_ledger.lock().expect("ledger");
        led.ensure_live(handle.context_id(), handle.generation())?;
        let entry = led
            .buffers
            .get(&handle.id())
            .ok_or_else(|| inp("unknown buffer handle", "buffer"))?;
        if !entry.usage.copy_src {
            return Err(inp("buffer missing copy_src for readback", "usage"));
        }
        let end = offset
            .checked_add(size)
            .ok_or_else(|| range_err("readback offset+size overflow"))?;
        if end > entry.size {
            return Err(range_err("readback range exceeds buffer size"));
        }
        led.reserve(size)?;
        let staging_id = NEXT_RESOURCE_ID.fetch_add(1, Ordering::SeqCst);

        match self.backend_device_queue() {
            Some((device, _)) => {
                let staging = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("spirula:gpu:buffer:readback-staging"),
                    size,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                let src = match led.buffers.get(&handle.id()).and_then(|e| e.gpu.as_ref()) {
                    Some(GpuBuffer::Wgpu(b)) => b,
                    _ => {
                        let _ = led.tracker.free(size);
                        return Err(gpu_lost("buffer gpu storage missing"));
                    }
                };
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("spirula:gpu:encoder:readback-buffer"),
                });
                encoder.copy_buffer_to_buffer(src, offset, &staging, 0, size);
                led.pending_commands.push(encoder.finish());
                led.stagings.insert(
                    staging_id,
                    StagingEntry {
                        accounted: size,
                        kind: StagingKind::Readback {
                            logical_len: size,
                            texture_pitch: None,
                        },
                        gpu: Some(GpuBuffer::Wgpu(staging)),
                        mapped_ready: false,
                    },
                );
            }
            None => {
                let data = match led.buffers.get(&handle.id()).and_then(|e| e.gpu.as_ref()) {
                    Some(GpuBuffer::Synthetic(v)) => {
                        v[offset as usize..(offset + size) as usize].to_vec()
                    }
                    _ => {
                        let _ = led.tracker.free(size);
                        return Err(gpu_lost("buffer gpu storage missing"));
                    }
                };
                led.stagings.insert(
                    staging_id,
                    StagingEntry {
                        accounted: size,
                        kind: StagingKind::Readback {
                            logical_len: size,
                            texture_pitch: None,
                        },
                        gpu: Some(GpuBuffer::Synthetic(data)),
                        mapped_ready: false,
                    },
                );
            }
        }
        led.pending_staging.push(staging_id);
        led.pending_buffers.push(handle.id());
        Ok(ReadbackTicket {
            context_id: self.id(),
            generation: self.generation(),
            staging_id,
            logical_len: size,
            texture_pitch: None,
        })
    }

    pub fn readback_texture_rgba8(
        &mut self,
        handle: &TextureHandle,
    ) -> Result<ReadbackTicket, DomainError> {
        self.require_live()?;
        let mut led = self.resource_ledger.lock().expect("ledger");
        led.ensure_live(handle.context_id(), handle.generation())?;
        let (width, height, usage_ok) = {
            let e = led
                .textures
                .get(&handle.id())
                .ok_or_else(|| inp("unknown texture handle", "texture"))?;
            (e.width, e.height, e.usage.copy_src)
        };
        if !usage_ok {
            return Err(inp("texture missing copy_src for readback", "usage"));
        }
        let bpr = rgba8_bytes_per_row(width)?;
        let padded = rgba8_padded_bytes(width, height)?;
        let logical = rgba8_logical_bytes(width, height)?;
        led.reserve(padded)?;
        let staging_id = NEXT_RESOURCE_ID.fetch_add(1, Ordering::SeqCst);

        match self.backend_device_queue() {
            Some((device, _)) => {
                let staging = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("spirula:gpu:buffer:tex-readback-staging"),
                    size: padded,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                let tex = match led.textures.get(&handle.id()).and_then(|e| e.gpu.as_ref()) {
                    Some(GpuTexture::Wgpu(t)) => t,
                    _ => {
                        let _ = led.tracker.free(padded);
                        return Err(gpu_lost("texture gpu storage missing"));
                    }
                };
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("spirula:gpu:encoder:readback-texture"),
                });
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: tex,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer: &staging,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(bpr),
                            rows_per_image: Some(height),
                        },
                    },
                    wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                );
                led.pending_commands.push(encoder.finish());
                led.stagings.insert(
                    staging_id,
                    StagingEntry {
                        accounted: padded,
                        kind: StagingKind::Readback {
                            logical_len: logical,
                            texture_pitch: Some((bpr, height)),
                        },
                        gpu: Some(GpuBuffer::Wgpu(staging)),
                        mapped_ready: false,
                    },
                );
            }
            None => {
                let data = match led.textures.get(&handle.id()).and_then(|e| e.gpu.as_ref()) {
                    Some(GpuTexture::Synthetic { data, .. }) => data.clone(),
                    _ => {
                        let _ = led.tracker.free(padded);
                        return Err(gpu_lost("texture gpu storage missing"));
                    }
                };
                led.stagings.insert(
                    staging_id,
                    StagingEntry {
                        accounted: padded,
                        kind: StagingKind::Readback {
                            logical_len: logical,
                            texture_pitch: Some((bpr, height)),
                        },
                        gpu: Some(GpuBuffer::Synthetic(data)),
                        mapped_ready: false,
                    },
                );
            }
        }
        led.pending_staging.push(staging_id);
        led.pending_textures.push(handle.id());
        Ok(ReadbackTicket {
            context_id: self.id(),
            generation: self.generation(),
            staging_id,
            logical_len: logical,
            texture_pitch: Some((bpr, height)),
        })
    }

    /// Submit all pending encoded copies. Does not wait.
    pub fn submit(&mut self) -> Result<SubmissionHandle, DomainError> {
        self.require_live()?;
        let mut led = self.resource_ledger.lock().expect("ledger");
        let sub_id = NEXT_SUBMISSION_ID.fetch_add(1, Ordering::SeqCst);
        let staging_ids = std::mem::take(&mut led.pending_staging);
        let buffer_ids = std::mem::take(&mut led.pending_buffers);
        let texture_ids = std::mem::take(&mut led.pending_textures);
        for id in &buffer_ids {
            if let Some(b) = led.buffers.get_mut(id) {
                b.inflight = b.inflight.saturating_add(1);
            }
        }
        for id in &texture_ids {
            if let Some(t) = led.textures.get_mut(id) {
                t.inflight = t.inflight.saturating_add(1);
            }
        }

        let wgpu_index = match self.backend_device_queue() {
            Some((_, queue)) => {
                let cmds = std::mem::take(&mut led.pending_commands);
                if cmds.is_empty() && staging_ids.is_empty() {
                    // Allow empty submit as no-op token for symmetry.
                }
                Some(queue.submit(cmds))
            }
            None => {
                led.pending_commands.clear();
                None
            }
        };

        led.submissions.insert(
            sub_id,
            SubmissionEntry {
                staging_ids,
                buffer_ids,
                texture_ids,
                wgpu_index,
                completed: false,
            },
        );

        Ok(SubmissionHandle {
            context_id: self.id(),
            generation: self.generation(),
            id: sub_id,
        })
    }

    /// Wait for a submission. `timeout` `None` waits indefinitely (host).
    pub fn wait(
        &mut self,
        submission: SubmissionHandle,
        timeout: Option<Duration>,
    ) -> Result<(), DomainError> {
        self.poll_host_lost();
        let mut led = self.resource_ledger.lock().expect("ledger");
        led.ensure_live(submission.context_id, submission.generation)?;
        let entry = led
            .submissions
            .get_mut(&submission.id)
            .ok_or_else(|| inp("unknown submission handle", "submission"))?;
        if entry.completed {
            return Ok(());
        }

        if let Some(index) = entry.wgpu_index.clone() {
            let device = match self.backend_device_queue() {
                Some((d, _)) => d,
                None => return Err(gpu_lost("device missing during wait")),
            };
            let poll = device.poll(wgpu::PollType::Wait {
                submission_index: Some(index),
                timeout,
            });
            match poll {
                Ok(_) => {}
                Err(wgpu::PollError::Timeout) => {
                    return Err(submit_err("submission wait timed out"));
                }
                Err(other) => {
                    return Err(submit_err(format!("submission wait failed: {other}")));
                }
            }
        }

        // Mark readback stagings ready; free upload stagings.
        let staging_ids = entry.staging_ids.clone();
        let buffer_ids = entry.buffer_ids.clone();
        let texture_ids = entry.texture_ids.clone();
        entry.completed = true;

        for sid in staging_ids {
            if let Some(st) = led.stagings.get_mut(&sid) {
                match st.kind {
                    StagingKind::Upload => {
                        // drop after wait
                        let accounted = st.accounted;
                        st.gpu = None;
                        let _ = led.tracker.free(accounted);
                        led.stagings.remove(&sid);
                    }
                    StagingKind::Readback { .. } => {
                        st.mapped_ready = true;
                    }
                }
            }
        }
        for id in &buffer_ids {
            if let Some(b) = led.buffers.get_mut(id) {
                b.inflight = b.inflight.saturating_sub(1);
            }
        }
        for id in &texture_ids {
            if let Some(t) = led.textures.get_mut(id) {
                t.inflight = t.inflight.saturating_sub(1);
            }
        }
        let reclaim_b: Vec<u64> = buffer_ids
            .into_iter()
            .filter(|id| {
                led.buffers
                    .get(id)
                    .is_some_and(|b| b.destroy_pending && b.inflight == 0 && b.handle_refs == 0)
            })
            .collect();
        for id in reclaim_b {
            led.reclaim_buffer(id);
        }
        let reclaim_t: Vec<u64> = texture_ids
            .into_iter()
            .filter(|id| {
                led.textures
                    .get(id)
                    .is_some_and(|t| t.destroy_pending && t.inflight == 0 && t.handle_refs == 0)
            })
            .collect();
        for id in reclaim_t {
            led.reclaim_texture(id);
        }
        Ok(())
    }

    /// Map a completed readback ticket into tightly packed host bytes.
    pub fn map_readback(&mut self, ticket: ReadbackTicket) -> Result<Vec<u8>, DomainError> {
        self.require_live()?;
        let mut led = self.resource_ledger.lock().expect("ledger");
        led.ensure_live(ticket.context_id, ticket.generation)?;
        let st = led
            .stagings
            .get_mut(&ticket.staging_id)
            .ok_or_else(|| inp("unknown readback ticket", "readback"))?;
        if !st.mapped_ready {
            return Err(inp(
                "readback staging not ready; wait for submission first",
                "readback",
            ));
        }

        let padded = match st.gpu.take() {
            Some(GpuBuffer::Wgpu(buf)) => {
                let slice = buf.slice(..);
                let (tx, rx) = std::sync::mpsc::channel();
                slice.map_async(wgpu::MapMode::Read, move |r| {
                    let _ = tx.send(r);
                });
                drop(led);
                // Poll until mapped.
                let device = self
                    .backend_device_queue()
                    .ok_or_else(|| gpu_lost("device missing during map"))?
                    .0;
                let start = Instant::now();
                loop {
                    let _ = device.poll(wgpu::PollType::Poll);
                    match rx.try_recv() {
                        Ok(Ok(())) => break,
                        Ok(Err(e)) => {
                            // re-lock to free staging account
                            let mut led = self.resource_ledger.lock().expect("ledger");
                            if let Some(st) = led.stagings.remove(&ticket.staging_id) {
                                let _ = led.tracker.free(st.accounted);
                            }
                            return Err(submit_err(format!("map_async failed: {e}")));
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => {
                            if start.elapsed() > Duration::from_secs(30) {
                                let mut led = self.resource_ledger.lock().expect("ledger");
                                if let Some(st) = led.stagings.remove(&ticket.staging_id) {
                                    let _ = led.tracker.free(st.accounted);
                                }
                                return Err(submit_err("map_async timed out"));
                            }
                            std::thread::yield_now();
                        }
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            let mut led = self.resource_ledger.lock().expect("ledger");
                            if let Some(st) = led.stagings.remove(&ticket.staging_id) {
                                let _ = led.tracker.free(st.accounted);
                            }
                            return Err(submit_err("map_async channel disconnected"));
                        }
                    }
                }
                let data = slice
                    .get_mapped_range()
                    .map_err(|e| submit_err(format!("get_mapped_range failed: {e}")))?
                    .to_vec();
                buf.unmap();
                let mut led = self.resource_ledger.lock().expect("ledger");
                if let Some(st) = led.stagings.remove(&ticket.staging_id) {
                    let _ = led.tracker.free(st.accounted);
                }
                data
            }
            Some(GpuBuffer::Synthetic(data)) => {
                let accounted = st.accounted;
                led.stagings.remove(&ticket.staging_id);
                let _ = led.tracker.free(accounted);
                data
            }
            None => return Err(gpu_lost("readback staging missing")),
        };

        if let Some((bpr, height)) = ticket.texture_pitch {
            let row_logical = (ticket.logical_len / u64::from(height)) as usize;
            let mut out = Vec::with_capacity(ticket.logical_len as usize);
            let pitch = bpr as usize;
            for y in 0..height as usize {
                let start = y * pitch;
                out.extend_from_slice(&padded[start..start + row_logical]);
            }
            Ok(out)
        } else {
            Ok(padded)
        }
    }

    fn require_live(&mut self) -> Result<(), DomainError> {
        self.poll_host_lost();
        if self.status() != ContextStatus::Live {
            return Err(handle_unavailable(self.status()));
        }
        Ok(())
    }
}

fn handle_unavailable(status: ContextStatus) -> DomainError {
    gpu_lost(match status {
        ContextStatus::Lost => "device lost; resources unavailable",
        ContextStatus::Closed => "context closed; resources unavailable",
        ContextStatus::CreateFailed => "create failed; resources unavailable",
        ContextStatus::Live => "context not live; resources unavailable",
    })
}
