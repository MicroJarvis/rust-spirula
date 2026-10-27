//! P2-F03: narrow buffer/texture handles, staging upload/readback, byte budget.
//!
//! Not a general wgpu mirror. Labels, sizes, usages, and ranges are checked in
//! Rust before any wgpu call.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use spirula_core::{AccountedBytesTracker, ContextKey, DomainError, ErrorCode, UserAction};

use crate::context::{ContextGeneration, ContextId, ContextStatus, GpuContext};

/// wgpu copy row pitch alignment (bytes).
pub const COPY_BYTES_PER_ROW_ALIGNMENT: u32 = 256;

/// wgpu `COPY_BUFFER_ALIGNMENT` for buffer↔buffer copy offset/size and
/// mapped-at-creation staging sizes (bytes).
pub const COPY_BUFFER_ALIGNMENT: u64 = 4;

/// Counter source for scoped GPU resource accounting (not device-wide VRAM).
pub const RESOURCE_ACCOUNT_SOURCE: &str = "accounted_scoped:gpu_context_resources";

/// Read-only snapshot of pre-submit pending queues (immutable copy for tests/diagnose).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingResourceCounts {
    pub commands: usize,
    pub stagings: usize,
    pub buffers: usize,
    pub textures: usize,
    pub current_bytes: u64,
}

/// One encoded GPU command with optional staging ownership.
///
/// Upload/readback commands own a staging id; compute commands do not. Ticket
/// Drop must remove by this ownership link — never by assuming
/// `pending_commands.len() == pending_staging.len()` or shared indices.
pub(crate) struct PendingGpuCommand {
    pub command: wgpu::CommandBuffer,
    pub staging_id: Option<u64>,
}

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

/// Narrow buffer usage (copy + P2-F04 storage/uniform as needed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferUsage {
    pub copy_src: bool,
    pub copy_dst: bool,
    pub storage: bool,
    pub uniform: bool,
}

impl BufferUsage {
    pub const COPY_SRC: Self = Self {
        copy_src: true,
        copy_dst: false,
        storage: false,
        uniform: false,
    };
    pub const COPY_DST: Self = Self {
        copy_src: false,
        copy_dst: true,
        storage: false,
        uniform: false,
    };
    pub const COPY_SRC_DST: Self = Self {
        copy_src: true,
        copy_dst: true,
        storage: false,
        uniform: false,
    };
    /// Storage + copy for compute round-trip buffers.
    pub const STORAGE_COPY: Self = Self {
        copy_src: true,
        copy_dst: true,
        storage: true,
        uniform: false,
    };
    /// Uniform + copy_dst for host upload of params.
    pub const UNIFORM_UPLOAD: Self = Self {
        copy_src: false,
        copy_dst: true,
        storage: false,
        uniform: true,
    };

    fn to_wgpu(self) -> wgpu::BufferUsages {
        let mut u = wgpu::BufferUsages::empty();
        if self.copy_src {
            u |= wgpu::BufferUsages::COPY_SRC;
        }
        if self.copy_dst {
            u |= wgpu::BufferUsages::COPY_DST;
        }
        if self.storage {
            u |= wgpu::BufferUsages::STORAGE;
        }
        if self.uniform {
            u |= wgpu::BufferUsages::UNIFORM;
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
///
/// `Clone` shares ownership (Arc). Drop of the **last** clone abandons staging:
/// frees immediately when GPU-ready or still pending; if submitted but incomplete,
/// marks abandon-on-complete so wait reaps without requiring `map_readback`.
/// Successful `map_readback` consumes the staging; further Drops are no-ops.
/// `map_readback` borrows the ticket so a not-ready / deadline `Err` never
/// destroys the only reclaim path.
#[derive(Clone)]
pub struct ReadbackTicket {
    inner: Arc<ReadbackTicketInner>,
}

struct ReadbackTicketInner {
    context_id: ContextId,
    generation: ContextGeneration,
    staging_id: u64,
    logical_len: u64,
    /// Texture strip: bytes_per_row and height; None = raw buffer.
    texture_pitch: Option<(u32, u32)>,
    ledger: Arc<Mutex<ResourceLedger>>,
    /// Set after a successful map frees staging (or explicit consume).
    consumed: AtomicBool,
}

impl std::fmt::Debug for ReadbackTicket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadbackTicket")
            .field("context_id", &self.inner.context_id)
            .field("generation", &self.inner.generation)
            .field("staging_id", &self.inner.staging_id)
            .field("logical_len", &self.inner.logical_len)
            .field("texture_pitch", &self.inner.texture_pitch)
            .field("consumed", &self.inner.consumed.load(Ordering::SeqCst))
            .finish()
    }
}

impl ReadbackTicket {
    pub fn context_id(&self) -> ContextId {
        self.inner.context_id
    }
    pub fn generation(&self) -> ContextGeneration {
        self.inner.generation
    }
    pub fn staging_id(&self) -> u64 {
        self.inner.staging_id
    }
}

impl Drop for ReadbackTicketInner {
    fn drop(&mut self) {
        if self.consumed.load(Ordering::SeqCst) {
            return;
        }
        if let Ok(mut led) = self.ledger.lock() {
            led.on_readback_ticket_dropped(self.staging_id);
        }
    }
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

pub(crate) fn check_copy_buffer_align(offset: u64, size: u64) -> Result<(), DomainError> {
    if !offset.is_multiple_of(COPY_BUFFER_ALIGNMENT) {
        return Err(range_err(format!(
            "buffer copy offset {offset} is not aligned to COPY_BUFFER_ALIGNMENT ({COPY_BUFFER_ALIGNMENT})"
        )));
    }
    if !size.is_multiple_of(COPY_BUFFER_ALIGNMENT) {
        return Err(range_err(format!(
            "buffer copy size {size} is not aligned to COPY_BUFFER_ALIGNMENT ({COPY_BUFFER_ALIGNMENT})"
        )));
    }
    Ok(())
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

pub(crate) enum GpuBuffer {
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

pub(crate) struct BufferEntry {
    #[allow(dead_code)]
    label: ResourceLabel,
    pub(crate) size: u64,
    pub(crate) usage: BufferUsage,
    accounted: u64,
    handle_refs: u32,
    /// Encoded but not yet submitted; GPU may still reference the resource.
    pending_hold: u32,
    inflight: u32,
    destroy_pending: bool,
    pub(crate) gpu: Option<GpuBuffer>,
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
    /// Encoded but not yet submitted; GPU may still reference the resource.
    pending_hold: u32,
    inflight: u32,
    destroy_pending: bool,
    gpu: Option<GpuTexture>,
}

enum PendingHoldTarget {
    Buffer(u64),
    Texture(u64),
}

struct StagingEntry {
    accounted: u64,
    kind: StagingKind,
    gpu: Option<GpuBuffer>,
    /// Set after wait completes; map consumes.
    mapped_ready: bool,
    /// Last ticket dropped while submission still incomplete; free on wait.
    abandon_when_complete: bool,
    /// Resource that received a pending_hold for this encode (cleared on submit).
    pending_hold_on: Option<PendingHoldTarget>,
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
    /// Enriched post-submit scope failure (SP-GPU-003/004). Survives diagnostic
    /// ring eviction; wait reclaims then returns this as the canonical Err.
    /// Stored without local `source` chain ([`DomainError::without_source`]).
    scope_failure: Option<DomainError>,
}

pub(crate) struct ResourceLedger {
    pub tracker: AccountedBytesTracker,
    pub budget_limit: Option<u64>,
    context_id: ContextId,
    generation: ContextGeneration,
    invalidated: bool,
    pub(crate) buffers: HashMap<u64, BufferEntry>,
    textures: HashMap<u64, TextureEntry>,
    stagings: HashMap<u64, StagingEntry>,
    submissions: HashMap<u64, SubmissionEntry>,
    /// Pending encoder commands waiting for submit (wgpu path).
    pub(crate) pending_commands: Vec<PendingGpuCommand>,
    pending_staging: Vec<u64>,
    pending_buffers: Vec<u64>,
    pending_textures: Vec<u64>,
    /// Test hook: next [`GpuContext::wait`] returns timeout without completing.
    inject_wait_timeout: bool,
    /// Test hook: next [`GpuContext::map_readback`] returns deadline without freeing.
    inject_map_deadline: bool,
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
            inject_wait_timeout: false,
            inject_map_deadline: false,
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

    pub(crate) fn ensure_live(
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
            if entry.inflight == 0 && entry.pending_hold == 0 {
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
            if entry.inflight == 0 && entry.pending_hold == 0 {
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

    fn maybe_reclaim_buffer(&mut self, id: u64) {
        if self.buffers.get(&id).is_some_and(|b| {
            b.destroy_pending && b.handle_refs == 0 && b.inflight == 0 && b.pending_hold == 0
        }) {
            self.reclaim_buffer(id);
        }
    }

    fn maybe_reclaim_texture(&mut self, id: u64) {
        if self.textures.get(&id).is_some_and(|t| {
            t.destroy_pending && t.handle_refs == 0 && t.inflight == 0 && t.pending_hold == 0
        }) {
            self.reclaim_texture(id);
        }
    }

    fn free_staging(&mut self, staging_id: u64) {
        if let Some(mut st) = self.stagings.remove(&staging_id) {
            st.gpu = None;
            let _ = self.tracker.free(st.accounted);
        }
    }

    /// Last `ReadbackTicket` dropped without a successful map.
    fn on_readback_ticket_dropped(&mut self, staging_id: u64) {
        let Some(st) = self.stagings.get(&staging_id) else {
            return;
        };
        if !matches!(st.kind, StagingKind::Readback { .. }) {
            return;
        }
        if st.mapped_ready {
            self.free_staging(staging_id);
            return;
        }
        // Still encoded pending (not submitted): remove the command that owns
        // this staging (by staging_id, not by parallel index), then holds +
        // staging. Compute commands carry `staging_id: None` and are untouched.
        // Synthetic may have staging without a wgpu command — free staging only.
        if let Some(idx) = self.pending_staging.iter().position(|&id| id == staging_id) {
            if let Some(cmd_idx) = self
                .pending_commands
                .iter()
                .position(|c| c.staging_id == Some(staging_id))
            {
                self.pending_commands.remove(cmd_idx);
            }
            self.pending_staging.remove(idx);
            let hold = self
                .stagings
                .get(&staging_id)
                .and_then(|s| match &s.pending_hold_on {
                    Some(PendingHoldTarget::Buffer(id)) => Some(PendingHoldTarget::Buffer(*id)),
                    Some(PendingHoldTarget::Texture(id)) => Some(PendingHoldTarget::Texture(*id)),
                    None => None,
                });
            match hold {
                Some(PendingHoldTarget::Buffer(bid)) => {
                    if let Some(i) = self.pending_buffers.iter().rposition(|&id| id == bid) {
                        self.pending_buffers.remove(i);
                    }
                    if let Some(b) = self.buffers.get_mut(&bid) {
                        b.pending_hold = b.pending_hold.saturating_sub(1);
                    }
                    self.free_staging(staging_id);
                    self.maybe_reclaim_buffer(bid);
                }
                Some(PendingHoldTarget::Texture(tid)) => {
                    if let Some(i) = self.pending_textures.iter().rposition(|&id| id == tid) {
                        self.pending_textures.remove(i);
                    }
                    if let Some(t) = self.textures.get_mut(&tid) {
                        t.pending_hold = t.pending_hold.saturating_sub(1);
                    }
                    self.free_staging(staging_id);
                    self.maybe_reclaim_texture(tid);
                }
                None => {
                    self.free_staging(staging_id);
                }
            }
            return;
        }
        // Submitted, GPU not finished: wait will reap.
        if let Some(st) = self.stagings.get_mut(&staging_id) {
            st.abandon_when_complete = true;
        }
    }

    fn pending_counts(&self) -> PendingResourceCounts {
        PendingResourceCounts {
            commands: self.pending_commands.len(),
            stagings: self.pending_staging.len(),
            buffers: self.pending_buffers.len(),
            textures: self.pending_textures.len(),
            current_bytes: self.tracker.current_bytes(),
        }
    }

    pub(crate) fn note_pending_buffer(&mut self, id: u64) {
        if let Some(b) = self.buffers.get_mut(&id) {
            b.pending_hold = b.pending_hold.saturating_add(1);
        }
        self.pending_buffers.push(id);
    }

    pub(crate) fn wgpu_buffer(&self, id: &u64) -> Option<wgpu::Buffer> {
        match self.buffers.get(id).and_then(|e| e.gpu.as_ref()) {
            Some(GpuBuffer::Wgpu(b)) => Some(b.clone()),
            _ => None,
        }
    }

    fn note_pending_texture(&mut self, id: u64) {
        if let Some(t) = self.textures.get_mut(&id) {
            t.pending_hold = t.pending_hold.saturating_add(1);
        }
        self.pending_textures.push(id);
    }
}

/// Capture wgpu validation / OOM for one device call; typed error instead of panic.
pub(crate) fn wgpu_scoped<T>(
    device: &wgpu::Device,
    class: crate::diagnostics::ScopeClass,
    inject: Option<crate::diagnostics::ScopeErrorInject>,
    op: impl FnOnce() -> T,
) -> Result<T, DomainError> {
    if let Some(inj) = inject {
        return Err(inj.into_domain_error(class));
    }
    let oom_scope = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let validation_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let value = op();
    let validation = pollster::block_on(validation_scope.pop());
    let oom = pollster::block_on(oom_scope.pop());
    if let Some(err) = validation {
        return Err(crate::diagnostics::map_wgpu_scope_error(err, class));
    }
    if let Some(err) = oom {
        return Err(crate::diagnostics::map_wgpu_scope_error(err, class));
    }
    Ok(value)
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

    /// Test/diagnose hook: next `wait` returns timeout after releasing the ledger
    /// (unlocked window) without completing or freeing this submission.
    pub fn inject_next_wait_timeout(&mut self) {
        if let Ok(mut led) = self.resource_ledger.lock() {
            led.inject_wait_timeout = true;
        }
    }

    /// Test hook: next `map_readback` issues `map_async` (host) then takes the
    /// deadline cancel path (`unmap` aborts pending map) — not a pre-map Err.
    pub fn inject_next_map_deadline(&mut self) {
        if let Ok(mut led) = self.resource_ledger.lock() {
            led.inject_map_deadline = true;
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

    /// Immutable copy of pre-submit pending queue lengths and current accounted bytes.
    pub fn pending_resource_counts(&self) -> PendingResourceCounts {
        self.resource_ledger
            .lock()
            .map(|l| l.pending_counts())
            .unwrap_or(PendingResourceCounts {
                commands: 0,
                stagings: 0,
                buffers: 0,
                textures: 0,
                current_bytes: 0,
            })
    }

    /// Retained submission records (completed waits reap metadata).
    pub fn retained_submission_count(&self) -> usize {
        self.resource_ledger
            .lock()
            .map(|l| l.submissions.len())
            .unwrap_or(0)
    }

    fn device_max_buffer_size(&self) -> u64 {
        self.report()
            .map(|r| r.device_limits.max_buffer_size)
            .unwrap_or(u64::MAX)
    }

    fn ensure_staging_fits_device(&self, size: u64) -> Result<(), DomainError> {
        let max = self.device_max_buffer_size();
        if size > max {
            return Err(range_err(format!(
                "staging buffer size {size} exceeds device max_buffer_size {max}"
            )));
        }
        Ok(())
    }

    pub fn create_buffer(&mut self, desc: BufferDesc) -> Result<BufferHandle, DomainError> {
        self.require_live()?;
        if desc.size == 0 {
            return Err(range_err("buffer size must be > 0"));
        }
        if !desc.usage.copy_src
            && !desc.usage.copy_dst
            && !desc.usage.storage
            && !desc.usage.uniform
        {
            return Err(inp(
                "buffer usage must include copy_src, copy_dst, storage, and/or uniform",
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

        let scope_inject = self.take_scope_inject();
        let mut led = self.resource_ledger.lock().expect("ledger");
        if let Some(inj) = scope_inject {
            let current = led.tracker.current_bytes();
            let budget = led.budget_limit;
            let (requested, current, budget) = match &inj {
                crate::diagnostics::ScopeErrorInject::OutOfMemory {
                    requested_bytes,
                    current_bytes,
                    budget_bytes,
                } => (
                    requested_bytes.or(Some(desc.size)),
                    current_bytes.or(Some(current)),
                    budget_bytes.or(budget),
                ),
                crate::diagnostics::ScopeErrorInject::Validation => {
                    (Some(desc.size), Some(current), budget)
                }
            };
            drop(led);
            return Err(self.fail_gpu(
                inj.into_domain_error(crate::diagnostics::ScopeClass::Resource),
                crate::diagnostics::GpuOperation::Create,
                Some(&desc.label),
                requested,
                current,
                budget,
            ));
        }
        if let Err(e) = led.reserve(desc.size) {
            let current = led.tracker.current_bytes();
            let budget = led.budget_limit;
            drop(led);
            return Err(self.fail_gpu(
                e,
                crate::diagnostics::GpuOperation::Create,
                Some(&desc.label),
                Some(desc.size),
                Some(current),
                budget,
            ));
        }
        let id = NEXT_RESOURCE_ID.fetch_add(1, Ordering::SeqCst);
        let gpu = match self.backend_device_queue() {
            Some((device, _)) => {
                match wgpu_scoped(
                    &device,
                    crate::diagnostics::ScopeClass::Resource,
                    None,
                    || {
                        device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some(desc.label.as_str()),
                            size: desc.size,
                            usage: desc.usage.to_wgpu(),
                            mapped_at_creation: false,
                        })
                    },
                ) {
                    Ok(buf) => GpuBuffer::Wgpu(buf),
                    Err(e) => {
                        let current = led.tracker.current_bytes();
                        let budget = led.budget_limit;
                        let _ = led.tracker.free(desc.size);
                        drop(led);
                        return Err(self.fail_gpu(
                            e,
                            crate::diagnostics::GpuOperation::Create,
                            Some(&desc.label),
                            Some(desc.size),
                            Some(current),
                            budget,
                        ));
                    }
                }
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
                pending_hold: 0,
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
        let max_dim = self
            .report()
            .map(|r| r.device_limits.max_texture_dimension_2d)
            .unwrap_or(8192);
        if desc.width > max_dim || desc.height > max_dim {
            return Err(range_err(format!(
                "texture dimension {}x{} exceeds device max_texture_dimension_2d {max_dim}",
                desc.width, desc.height
            )));
        }
        let accounted = rgba8_padded_bytes(desc.width, desc.height)?;
        let bpr = rgba8_bytes_per_row(desc.width)?;
        // Note: max_buffer_size does not limit the texture itself; padded
        // staging is checked on upload/readback before create_buffer.

        let scope_inject = self.take_scope_inject();
        let mut led = self.resource_ledger.lock().expect("ledger");
        if let Some(inj) = scope_inject {
            let current = led.tracker.current_bytes();
            let budget = led.budget_limit;
            drop(led);
            return Err(self.fail_gpu(
                inj.into_domain_error(crate::diagnostics::ScopeClass::Resource),
                crate::diagnostics::GpuOperation::Create,
                Some(&desc.label),
                Some(accounted),
                Some(current),
                budget,
            ));
        }
        if let Err(e) = led.reserve(accounted) {
            let current = led.tracker.current_bytes();
            let budget = led.budget_limit;
            drop(led);
            return Err(self.fail_gpu(
                e,
                crate::diagnostics::GpuOperation::Create,
                Some(&desc.label),
                Some(accounted),
                Some(current),
                budget,
            ));
        }
        let id = NEXT_RESOURCE_ID.fetch_add(1, Ordering::SeqCst);
        let gpu = match self.backend_device_queue() {
            Some((device, _)) => {
                match wgpu_scoped(
                    &device,
                    crate::diagnostics::ScopeClass::Resource,
                    None,
                    || {
                        device.create_texture(&wgpu::TextureDescriptor {
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
                        })
                    },
                ) {
                    Ok(tex) => GpuTexture::Wgpu(tex),
                    Err(e) => {
                        let current = led.tracker.current_bytes();
                        let budget = led.budget_limit;
                        let _ = led.tracker.free(accounted);
                        drop(led);
                        return Err(self.fail_gpu(
                            e,
                            crate::diagnostics::GpuOperation::Create,
                            Some(&desc.label),
                            Some(accounted),
                            Some(current),
                            budget,
                        ));
                    }
                }
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
                pending_hold: 0,
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
        let size = data.len() as u64;
        check_copy_buffer_align(offset, size)?;
        self.ensure_staging_fits_device(size)?;
        let mut led = self.resource_ledger.lock().expect("ledger");
        led.ensure_live(handle.context_id(), handle.generation())?;
        let entry = led
            .buffers
            .get(&handle.id())
            .ok_or_else(|| inp("unknown buffer handle", "buffer"))?;
        if !entry.usage.copy_dst {
            return Err(inp("buffer missing copy_dst for upload", "usage"));
        }
        let label = entry.label.clone();
        let end = offset
            .checked_add(size)
            .ok_or_else(|| range_err("upload offset+len overflow"))?;
        if end > entry.size {
            return Err(range_err("upload range exceeds buffer size"));
        }
        if let Err(e) = led.reserve(size) {
            let current = led.tracker.current_bytes();
            let budget = led.budget_limit;
            drop(led);
            return Err(self.fail_gpu(
                e,
                crate::diagnostics::GpuOperation::Upload,
                Some(&label),
                Some(size),
                Some(current),
                budget,
            ));
        }
        let staging_id = NEXT_RESOURCE_ID.fetch_add(1, Ordering::SeqCst);

        match self.backend_device_queue() {
            Some((device, queue)) => {
                let staging = match wgpu_scoped(
                    &device,
                    crate::diagnostics::ScopeClass::Resource,
                    None,
                    || {
                        device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some("spirula:gpu:buffer:upload-staging"),
                            size,
                            usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::MAP_WRITE,
                            mapped_at_creation: true,
                        })
                    },
                ) {
                    Ok(b) => b,
                    Err(e) => {
                        let current = led.tracker.current_bytes();
                        let budget = led.budget_limit;
                        let _ = led.tracker.free(size);
                        drop(led);
                        return Err(self.fail_gpu(
                            e,
                            crate::diagnostics::GpuOperation::Upload,
                            Some(&label),
                            Some(size),
                            Some(current),
                            budget,
                        ));
                    }
                };
                {
                    let mut view = match staging.slice(..).get_mapped_range_mut() {
                        Ok(v) => v,
                        Err(e) => {
                            let current = led.tracker.current_bytes();
                            let budget = led.budget_limit;
                            let _ = led.tracker.free(size);
                            drop(led);
                            return Err(self.fail_gpu(
                                submit_err(format!("map upload staging failed: {e}")),
                                crate::diagnostics::GpuOperation::Upload,
                                Some(&label),
                                Some(size),
                                Some(current),
                                budget,
                            ));
                        }
                    };
                    view.copy_from_slice(data);
                }
                staging.unmap();

                let dst = match led.buffers.get(&handle.id()).and_then(|e| e.gpu.as_ref()) {
                    Some(GpuBuffer::Wgpu(b)) => b,
                    _ => {
                        let current = led.tracker.current_bytes();
                        let budget = led.budget_limit;
                        let _ = led.tracker.free(size);
                        drop(led);
                        return Err(self.fail_gpu(
                            gpu_lost("buffer gpu storage missing"),
                            crate::diagnostics::GpuOperation::Upload,
                            Some(&label),
                            Some(size),
                            Some(current),
                            budget,
                        ));
                    }
                };
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("spirula:gpu:encoder:upload-buffer"),
                });
                encoder.copy_buffer_to_buffer(&staging, 0, dst, offset, size);
                led.pending_commands.push(PendingGpuCommand {
                    command: encoder.finish(),
                    staging_id: Some(staging_id),
                });
                // Keep staging alive until submit/wait — store it.
                led.stagings.insert(
                    staging_id,
                    StagingEntry {
                        accounted: size,
                        kind: StagingKind::Upload,
                        gpu: Some(GpuBuffer::Wgpu(staging)),
                        mapped_ready: false,
                        abandon_when_complete: false,
                        pending_hold_on: Some(PendingHoldTarget::Buffer(handle.id())),
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
                        abandon_when_complete: false,
                        pending_hold_on: Some(PendingHoldTarget::Buffer(handle.id())),
                    },
                );
            }
        }
        led.pending_staging.push(staging_id);
        led.note_pending_buffer(handle.id());
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
        let (width, height, usage_ok, label) = {
            let e = led
                .textures
                .get(&handle.id())
                .ok_or_else(|| inp("unknown texture handle", "texture"))?;
            (e.width, e.height, e.usage.copy_dst, e.label.clone())
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
        self.ensure_staging_fits_device(padded)?;
        if let Err(e) = led.reserve(padded) {
            let current = led.tracker.current_bytes();
            let budget = led.budget_limit;
            drop(led);
            return Err(self.fail_gpu(
                e,
                crate::diagnostics::GpuOperation::Upload,
                Some(&label),
                Some(padded),
                Some(current),
                budget,
            ));
        }
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
                let staging = match wgpu_scoped(
                    &device,
                    crate::diagnostics::ScopeClass::Resource,
                    None,
                    || {
                        device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some("spirula:gpu:buffer:tex-upload-staging"),
                            size: padded,
                            usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::MAP_WRITE,
                            mapped_at_creation: true,
                        })
                    },
                ) {
                    Ok(b) => b,
                    Err(e) => {
                        let current = led.tracker.current_bytes();
                        let budget = led.budget_limit;
                        let _ = led.tracker.free(padded);
                        drop(led);
                        return Err(self.fail_gpu(
                            e,
                            crate::diagnostics::GpuOperation::Upload,
                            Some(&label),
                            Some(padded),
                            Some(current),
                            budget,
                        ));
                    }
                };
                {
                    let mut view = match staging.slice(..).get_mapped_range_mut() {
                        Ok(v) => v,
                        Err(e) => {
                            let current = led.tracker.current_bytes();
                            let budget = led.budget_limit;
                            let _ = led.tracker.free(padded);
                            drop(led);
                            return Err(self.fail_gpu(
                                submit_err(format!("map texture upload staging failed: {e}")),
                                crate::diagnostics::GpuOperation::Upload,
                                Some(&label),
                                Some(padded),
                                Some(current),
                                budget,
                            ));
                        }
                    };
                    view.copy_from_slice(&padded_bytes);
                }
                staging.unmap();

                let tex = match led.textures.get(&handle.id()).and_then(|e| e.gpu.as_ref()) {
                    Some(GpuTexture::Wgpu(t)) => t,
                    _ => {
                        let current = led.tracker.current_bytes();
                        let budget = led.budget_limit;
                        let _ = led.tracker.free(padded);
                        drop(led);
                        return Err(self.fail_gpu(
                            gpu_lost("texture gpu storage missing"),
                            crate::diagnostics::GpuOperation::Upload,
                            Some(&label),
                            Some(padded),
                            Some(current),
                            budget,
                        ));
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
                led.pending_commands.push(PendingGpuCommand {
                    command: encoder.finish(),
                    staging_id: Some(staging_id),
                });
                led.stagings.insert(
                    staging_id,
                    StagingEntry {
                        accounted: padded,
                        kind: StagingKind::Upload,
                        gpu: Some(GpuBuffer::Wgpu(staging)),
                        mapped_ready: false,
                        abandon_when_complete: false,
                        pending_hold_on: Some(PendingHoldTarget::Texture(handle.id())),
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
                        abandon_when_complete: false,
                        pending_hold_on: Some(PendingHoldTarget::Texture(handle.id())),
                    },
                );
            }
        }
        led.pending_staging.push(staging_id);
        led.note_pending_texture(handle.id());
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
        check_copy_buffer_align(offset, size)?;
        self.ensure_staging_fits_device(size)?;
        let mut led = self.resource_ledger.lock().expect("ledger");
        led.ensure_live(handle.context_id(), handle.generation())?;
        let entry = led
            .buffers
            .get(&handle.id())
            .ok_or_else(|| inp("unknown buffer handle", "buffer"))?;
        if !entry.usage.copy_src {
            return Err(inp("buffer missing copy_src for readback", "usage"));
        }
        let label = entry.label.clone();
        let end = offset
            .checked_add(size)
            .ok_or_else(|| range_err("readback offset+size overflow"))?;
        if end > entry.size {
            return Err(range_err("readback range exceeds buffer size"));
        }
        if let Err(e) = led.reserve(size) {
            let current = led.tracker.current_bytes();
            let budget = led.budget_limit;
            drop(led);
            return Err(self.fail_gpu(
                e,
                crate::diagnostics::GpuOperation::Readback,
                Some(&label),
                Some(size),
                Some(current),
                budget,
            ));
        }
        let staging_id = NEXT_RESOURCE_ID.fetch_add(1, Ordering::SeqCst);

        match self.backend_device_queue() {
            Some((device, _)) => {
                let staging = match wgpu_scoped(
                    &device,
                    crate::diagnostics::ScopeClass::Resource,
                    None,
                    || {
                        device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some("spirula:gpu:buffer:readback-staging"),
                            size,
                            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                            mapped_at_creation: false,
                        })
                    },
                ) {
                    Ok(b) => b,
                    Err(e) => {
                        let current = led.tracker.current_bytes();
                        let budget = led.budget_limit;
                        let _ = led.tracker.free(size);
                        drop(led);
                        return Err(self.fail_gpu(
                            e,
                            crate::diagnostics::GpuOperation::Readback,
                            Some(&label),
                            Some(size),
                            Some(current),
                            budget,
                        ));
                    }
                };
                let src = match led.buffers.get(&handle.id()).and_then(|e| e.gpu.as_ref()) {
                    Some(GpuBuffer::Wgpu(b)) => b,
                    _ => {
                        let current = led.tracker.current_bytes();
                        let budget = led.budget_limit;
                        let _ = led.tracker.free(size);
                        drop(led);
                        return Err(self.fail_gpu(
                            gpu_lost("buffer gpu storage missing"),
                            crate::diagnostics::GpuOperation::Readback,
                            Some(&label),
                            Some(size),
                            Some(current),
                            budget,
                        ));
                    }
                };
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("spirula:gpu:encoder:readback-buffer"),
                });
                encoder.copy_buffer_to_buffer(src, offset, &staging, 0, size);
                led.pending_commands.push(PendingGpuCommand {
                    command: encoder.finish(),
                    staging_id: Some(staging_id),
                });
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
                        abandon_when_complete: false,
                        pending_hold_on: Some(PendingHoldTarget::Buffer(handle.id())),
                    },
                );
            }
            None => {
                let data = match led.buffers.get(&handle.id()).and_then(|e| e.gpu.as_ref()) {
                    Some(GpuBuffer::Synthetic(v)) => {
                        v[offset as usize..(offset + size) as usize].to_vec()
                    }
                    _ => {
                        let current = led.tracker.current_bytes();
                        let budget = led.budget_limit;
                        let _ = led.tracker.free(size);
                        drop(led);
                        return Err(self.fail_gpu(
                            gpu_lost("buffer gpu storage missing"),
                            crate::diagnostics::GpuOperation::Readback,
                            Some(&label),
                            Some(size),
                            Some(current),
                            budget,
                        ));
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
                        abandon_when_complete: false,
                        pending_hold_on: Some(PendingHoldTarget::Buffer(handle.id())),
                    },
                );
            }
        }
        led.pending_staging.push(staging_id);
        led.note_pending_buffer(handle.id());
        Ok(ReadbackTicket {
            inner: Arc::new(ReadbackTicketInner {
                context_id: self.id(),
                generation: self.generation(),
                staging_id,
                logical_len: size,
                texture_pitch: None,
                ledger: self.ledger_arc(),
                consumed: AtomicBool::new(false),
            }),
        })
    }

    pub fn readback_texture_rgba8(
        &mut self,
        handle: &TextureHandle,
    ) -> Result<ReadbackTicket, DomainError> {
        self.require_live()?;
        let mut led = self.resource_ledger.lock().expect("ledger");
        led.ensure_live(handle.context_id(), handle.generation())?;
        let (width, height, usage_ok, label) = {
            let e = led
                .textures
                .get(&handle.id())
                .ok_or_else(|| inp("unknown texture handle", "texture"))?;
            (e.width, e.height, e.usage.copy_src, e.label.clone())
        };
        if !usage_ok {
            return Err(inp("texture missing copy_src for readback", "usage"));
        }
        let bpr = rgba8_bytes_per_row(width)?;
        let padded = rgba8_padded_bytes(width, height)?;
        let logical = rgba8_logical_bytes(width, height)?;
        self.ensure_staging_fits_device(padded)?;
        if let Err(e) = led.reserve(padded) {
            let current = led.tracker.current_bytes();
            let budget = led.budget_limit;
            drop(led);
            return Err(self.fail_gpu(
                e,
                crate::diagnostics::GpuOperation::Readback,
                Some(&label),
                Some(padded),
                Some(current),
                budget,
            ));
        }
        let staging_id = NEXT_RESOURCE_ID.fetch_add(1, Ordering::SeqCst);

        match self.backend_device_queue() {
            Some((device, _)) => {
                let staging = match wgpu_scoped(
                    &device,
                    crate::diagnostics::ScopeClass::Resource,
                    None,
                    || {
                        device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some("spirula:gpu:buffer:tex-readback-staging"),
                            size: padded,
                            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                            mapped_at_creation: false,
                        })
                    },
                ) {
                    Ok(b) => b,
                    Err(e) => {
                        let current = led.tracker.current_bytes();
                        let budget = led.budget_limit;
                        let _ = led.tracker.free(padded);
                        drop(led);
                        return Err(self.fail_gpu(
                            e,
                            crate::diagnostics::GpuOperation::Readback,
                            Some(&label),
                            Some(padded),
                            Some(current),
                            budget,
                        ));
                    }
                };
                let tex = match led.textures.get(&handle.id()).and_then(|e| e.gpu.as_ref()) {
                    Some(GpuTexture::Wgpu(t)) => t,
                    _ => {
                        let current = led.tracker.current_bytes();
                        let budget = led.budget_limit;
                        let _ = led.tracker.free(padded);
                        drop(led);
                        return Err(self.fail_gpu(
                            gpu_lost("texture gpu storage missing"),
                            crate::diagnostics::GpuOperation::Readback,
                            Some(&label),
                            Some(padded),
                            Some(current),
                            budget,
                        ));
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
                led.pending_commands.push(PendingGpuCommand {
                    command: encoder.finish(),
                    staging_id: Some(staging_id),
                });
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
                        abandon_when_complete: false,
                        pending_hold_on: Some(PendingHoldTarget::Texture(handle.id())),
                    },
                );
            }
            None => {
                let data = match led.textures.get(&handle.id()).and_then(|e| e.gpu.as_ref()) {
                    Some(GpuTexture::Synthetic { data, .. }) => data.clone(),
                    _ => {
                        let current = led.tracker.current_bytes();
                        let budget = led.budget_limit;
                        let _ = led.tracker.free(padded);
                        drop(led);
                        return Err(self.fail_gpu(
                            gpu_lost("texture gpu storage missing"),
                            crate::diagnostics::GpuOperation::Readback,
                            Some(&label),
                            Some(padded),
                            Some(current),
                            budget,
                        ));
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
                        abandon_when_complete: false,
                        pending_hold_on: Some(PendingHoldTarget::Texture(handle.id())),
                    },
                );
            }
        }
        led.pending_staging.push(staging_id);
        led.note_pending_texture(handle.id());
        Ok(ReadbackTicket {
            inner: Arc::new(ReadbackTicketInner {
                context_id: self.id(),
                generation: self.generation(),
                staging_id,
                logical_len: logical,
                texture_pitch: Some((bpr, height)),
                ledger: self.ledger_arc(),
                consumed: AtomicBool::new(false),
            }),
        })
    }

    /// Submit all pending encoded copies. Does not wait.
    pub fn submit(&mut self) -> Result<SubmissionHandle, DomainError> {
        self.require_live()?;
        // Synthetic probes fail *before* mutating pending→inflight ownership.
        if self.take_submit_inject() {
            return Err(self.fail_gpu(
                submit_err("injected submit failure"),
                crate::diagnostics::GpuOperation::Submit,
                None,
                None,
                None,
                None,
            ));
        }
        if let Some(inj) = self.take_scope_inject() {
            return Err(self.fail_gpu(
                inj.into_domain_error(crate::diagnostics::ScopeClass::Submit),
                crate::diagnostics::GpuOperation::Submit,
                None,
                None,
                None,
                None,
            ));
        }
        let post_inject = self.take_post_submit_scope_inject();
        let post_oom_meta = post_inject
            .as_ref()
            .map(crate::diagnostics::ScopeErrorInject::oom_meta)
            .unwrap_or((None, None, None));

        let mut led = self.resource_ledger.lock().expect("ledger");
        let sub_id = NEXT_SUBMISSION_ID.fetch_add(1, Ordering::SeqCst);
        let staging_ids = std::mem::take(&mut led.pending_staging);
        let buffer_ids = std::mem::take(&mut led.pending_buffers);
        let texture_ids = std::mem::take(&mut led.pending_textures);
        for sid in &staging_ids {
            if let Some(st) = led.stagings.get_mut(sid) {
                st.pending_hold_on = None;
            }
        }
        // Encoded → submitted: release pending_hold, take inflight.
        for id in &buffer_ids {
            if let Some(b) = led.buffers.get_mut(id) {
                b.pending_hold = b.pending_hold.saturating_sub(1);
                b.inflight = b.inflight.saturating_add(1);
            }
        }
        for id in &texture_ids {
            if let Some(t) = led.textures.get_mut(id) {
                t.pending_hold = t.pending_hold.saturating_sub(1);
                t.inflight = t.inflight.saturating_add(1);
            }
        }

        let (wgpu_index, scope_err) = match self.backend_device_queue() {
            Some((device, queue)) => {
                let cmds = std::mem::take(&mut led.pending_commands);
                if cmds.is_empty() && staging_ids.is_empty() {
                    // Allow empty submit as no-op token for symmetry.
                }
                let buffers: Vec<wgpu::CommandBuffer> =
                    cmds.into_iter().map(|c| c.command).collect();
                // Host error scopes around submit (runtime-contracts §6.3).
                // queue.submit accepts work before scope pop — keep the index so
                // the returned handle can wait/reap even when scopes report.
                let oom_scope = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
                let validation_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
                let index = queue.submit(buffers);
                let validation = pollster::block_on(validation_scope.pop());
                let oom = pollster::block_on(oom_scope.pop());
                let mut err = validation.or(oom).map(|e| {
                    crate::diagnostics::map_wgpu_scope_error(
                        e,
                        crate::diagnostics::ScopeClass::Submit,
                    )
                });
                if err.is_none()
                    && let Some(inj) = post_inject
                {
                    err = Some(inj.into_domain_error(crate::diagnostics::ScopeClass::Submit));
                }
                (Some(index), err)
            }
            None => {
                led.pending_commands.clear();
                // Synthetic: no host queue; post-submit inject still latches a
                // waitable handle with canonical failure.
                let err = post_inject
                    .map(|inj| inj.into_domain_error(crate::diagnostics::ScopeClass::Submit));
                (None, err)
            }
        };

        let (req, cur, bud) = post_oom_meta;
        let latched_failure = scope_err.map(|err| {
            crate::diagnostics::enrich_gpu_error(
                err,
                crate::diagnostics::GpuOperation::Submit,
                None,
                req,
                cur,
                bud,
            )
        });

        led.submissions.insert(
            sub_id,
            SubmissionEntry {
                staging_ids,
                buffer_ids,
                texture_ids,
                wgpu_index,
                completed: false,
                scope_failure: latched_failure.as_ref().map(DomainError::without_source),
            },
        );
        drop(led);

        let handle = SubmissionHandle {
            context_id: self.id(),
            generation: self.generation(),
            id: sub_id,
        };

        if let Some(ref err) = latched_failure {
            // Diagnostic may be ring-evicted; SubmissionEntry keeps the failure.
            self.record_error_diagnostic(
                err,
                crate::diagnostics::GpuOperation::Submit,
                None,
                req,
                cur,
                bud,
            );
        }

        Ok(handle)
    }

    /// Wait for a submission. `timeout` `None` waits indefinitely (host).
    ///
    /// On timeout the submission stays incomplete: staging/resources and their
    /// accounted bytes are retained so a later `wait` (or context teardown)
    /// can finish safely. Timeout is not a free/reclaim signal.
    ///
    /// After GPU sync completes the submission is **reaped**. If submit latched
    /// a post-submit scope failure (`SP-GPU-003`/`SP-GPU-004`), wait returns
    /// that canonical Err **after** reclaim (readbacks are not marked
    /// `mapped_ready`). Device loss delivered by this wait's own `device.poll`
    /// is observed before reclaim/Ok and returns `SP-GPU-002` (resources freed
    /// only via Lost teardown).
    pub fn wait(
        &mut self,
        submission: SubmissionHandle,
        timeout: Option<Duration>,
    ) -> Result<(), DomainError> {
        self.poll_host_lost();
        if self.status() != ContextStatus::Live {
            return Err(handle_unavailable(self.status()));
        }
        let mut led = self.resource_ledger.lock().expect("ledger");
        led.ensure_live(submission.context_id, submission.generation)?;
        if led.inject_wait_timeout {
            led.inject_wait_timeout = false;
            // Release the ledger so unrelated handles may Drop during the
            // unlocked window (same as a real poll); this submission is kept
            // (including any latched scope_failure) for a later wait.
            drop(led);
            for _ in 0..64 {
                std::thread::yield_now();
            }
            return Err(self.fail_gpu(
                submit_err(
                    "submission wait timed out (caller deadline; in-flight staging retained)",
                ),
                crate::diagnostics::GpuOperation::Wait,
                None,
                None,
                None,
                None,
            ));
        }
        let entry = led
            .submissions
            .get_mut(&submission.id)
            .ok_or_else(|| inp("unknown submission handle", "submission"))?;
        if entry.completed {
            // Should not linger after reap; treat as unknown if somehow present.
            led.submissions.remove(&submission.id);
            return Ok(());
        }

        if let Some(index) = entry.wgpu_index.clone() {
            let device = match self.backend_device_queue() {
                Some((d, _)) => d,
                None => return Err(gpu_lost("device missing during wait")),
            };
            // Drop ledger lock across poll (lost callback may need the mutex;
            // unrelated BufferHandle/TextureHandle/ReadbackTicket Drop is legal).
            drop(led);
            let poll = device.poll(wgpu::PollType::Wait {
                submission_index: Some(index),
                timeout,
            });
            // Host Destroyed/lost callbacks fire during poll — Lost wins over
            // latched scope failure / Ok.
            self.poll_host_lost();
            if self.status() != ContextStatus::Live {
                return Err(handle_unavailable(self.status()));
            }
            let mut led = self.resource_ledger.lock().expect("ledger");
            match poll {
                Ok(_) => {}
                Err(wgpu::PollError::Timeout) => {
                    drop(led);
                    return Err(self.fail_gpu(
                        submit_err(
                            "submission wait timed out (caller deadline; in-flight staging retained)",
                        ),
                        crate::diagnostics::GpuOperation::Wait,
                        None,
                        None,
                        None,
                        None,
                    ));
                }
                Err(other) => {
                    drop(led);
                    return Err(self.fail_gpu(
                        submit_err(format!("submission wait failed: {other}")),
                        crate::diagnostics::GpuOperation::Wait,
                        None,
                        None,
                        None,
                        None,
                    ));
                }
            }
            led.ensure_live(submission.context_id, submission.generation)?;
            let entry = led
                .submissions
                .get_mut(&submission.id)
                .ok_or_else(|| inp("unknown submission handle", "submission"))?;
            if entry.completed {
                led.submissions.remove(&submission.id);
                return Ok(());
            }

            let staging_ids = std::mem::take(&mut entry.staging_ids);
            let buffer_ids = std::mem::take(&mut entry.buffer_ids);
            let texture_ids = std::mem::take(&mut entry.texture_ids);
            let scope_failure = entry.scope_failure.take();
            led.submissions.remove(&submission.id);
            let failed = scope_failure.is_some();
            self.finish_wait_locked(&mut led, staging_ids, buffer_ids, texture_ids, failed)?;
            drop(led);
            return match scope_failure {
                Some(err) => Err(err),
                None => Ok(()),
            };
        }

        // Synthetic / no wgpu index: complete immediately.
        let staging_ids = std::mem::take(&mut entry.staging_ids);
        let buffer_ids = std::mem::take(&mut entry.buffer_ids);
        let texture_ids = std::mem::take(&mut entry.texture_ids);
        let scope_failure = entry.scope_failure.take();
        led.submissions.remove(&submission.id);
        let failed = scope_failure.is_some();
        self.finish_wait_locked(&mut led, staging_ids, buffer_ids, texture_ids, failed)?;
        drop(led);
        match scope_failure {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    fn finish_wait_locked(
        &self,
        led: &mut ResourceLedger,
        staging_ids: Vec<u64>,
        buffer_ids: Vec<u64>,
        texture_ids: Vec<u64>,
        submission_failed: bool,
    ) -> Result<(), DomainError> {
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
                        // Failed submissions must not become successful maps.
                        if submission_failed || st.abandon_when_complete {
                            let accounted = st.accounted;
                            st.gpu = None;
                            let _ = led.tracker.free(accounted);
                            led.stagings.remove(&sid);
                        } else {
                            st.mapped_ready = true;
                        }
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
                led.buffers.get(id).is_some_and(|b| {
                    b.destroy_pending
                        && b.inflight == 0
                        && b.pending_hold == 0
                        && b.handle_refs == 0
                })
            })
            .collect();
        for id in reclaim_b {
            led.reclaim_buffer(id);
        }
        let reclaim_t: Vec<u64> = texture_ids
            .into_iter()
            .filter(|id| {
                led.textures.get(id).is_some_and(|t| {
                    t.destroy_pending
                        && t.inflight == 0
                        && t.pending_hold == 0
                        && t.handle_refs == 0
                })
            })
            .collect();
        for id in reclaim_t {
            led.reclaim_texture(id);
        }
        Ok(())
    }

    /// Map a completed readback ticket into tightly packed host bytes.
    ///
    /// Borrows the ticket: not-ready / deadline `Err` keeps ownership so Drop or
    /// a later map can still reclaim. Requires a prior successful `wait`
    /// (`mapped_ready`). Staging is freed only after a successful map, ticket
    /// abandon (Drop), definitive post-GPU map failure that retires the staging,
    /// or context teardown.
    pub fn map_readback(&mut self, ticket: &ReadbackTicket) -> Result<Vec<u8>, DomainError> {
        self.require_live()?;
        if ticket.inner.consumed.load(Ordering::SeqCst) {
            return Err(inp(
                "readback ticket already consumed by a successful map",
                "readback",
            ));
        }
        let staging_id = ticket.inner.staging_id;
        let logical_len = ticket.inner.logical_len;
        let texture_pitch = ticket.inner.texture_pitch;
        let mut led = self.resource_ledger.lock().expect("ledger");
        led.ensure_live(ticket.inner.context_id, ticket.inner.generation)?;
        let ready = led
            .stagings
            .get(&staging_id)
            .map(|s| s.mapped_ready)
            .ok_or_else(|| inp("unknown readback ticket", "readback"))?;
        if !ready {
            return Err(inp(
                "readback staging not ready; wait for submission first (GPU work not confirmed complete)",
                "readback",
            ));
        }
        // Consume inject flag up front; host path still issues map_async then cancels.
        let force_deadline = led.inject_map_deadline;
        if force_deadline {
            led.inject_map_deadline = false;
        }

        let st = led
            .stagings
            .get_mut(&staging_id)
            .ok_or_else(|| inp("unknown readback ticket", "readback"))?;
        let padded = match st.gpu.take() {
            Some(GpuBuffer::Wgpu(buf)) => {
                let slice = buf.slice(..);
                let (tx, rx) = std::sync::mpsc::channel();
                slice.map_async(wgpu::MapMode::Read, move |r| {
                    let _ = tx.send(r);
                });
                drop(led);
                let device = self
                    .backend_device_queue()
                    .ok_or_else(|| gpu_lost("device missing during map"))?
                    .0;

                let map_result = if force_deadline {
                    Err("map_async timed out".to_string())
                } else {
                    let start = Instant::now();
                    loop {
                        let _ = device.poll(wgpu::PollType::Poll);
                        match rx.try_recv() {
                            Ok(Ok(())) => break Ok(()),
                            Ok(Err(e)) => break Err(format!("map_async failed: {e}")),
                            Err(std::sync::mpsc::TryRecvError::Empty) => {
                                if start.elapsed() > Duration::from_secs(30) {
                                    break Err("map_async timed out".into());
                                }
                                std::thread::yield_now();
                            }
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                break Err("map_async channel disconnected".into());
                            }
                        }
                    }
                };
                // Poll may have delivered device-lost; observe before choosing map Err/Ok.
                self.poll_host_lost();
                if self.status() != ContextStatus::Live {
                    // Restore staging for teardown/drop reclaim; do not invent map success.
                    let mut led = self.resource_ledger.lock().expect("ledger");
                    if let Some(st) = led.stagings.get_mut(&staging_id) {
                        st.gpu = Some(GpuBuffer::Wgpu(buf));
                    }
                    drop(led);
                    return Err(handle_unavailable(self.status()));
                }
                match map_result {
                    Ok(()) => {
                        let data = match slice.get_mapped_range() {
                            Ok(view) => view.to_vec(),
                            Err(e) => {
                                // Mapped but range failed: unmap to Idle before restore/retry.
                                buf.unmap();
                                let mut led = self.resource_ledger.lock().expect("ledger");
                                if let Some(st) = led.stagings.get_mut(&staging_id) {
                                    st.gpu = Some(GpuBuffer::Wgpu(buf));
                                }
                                drop(led);
                                return Err(self.fail_gpu(
                                    submit_err(format!("get_mapped_range failed: {e}")),
                                    crate::diagnostics::GpuOperation::Map,
                                    None,
                                    None,
                                    None,
                                    None,
                                ));
                            }
                        };
                        buf.unmap();
                        let mut led = self.resource_ledger.lock().expect("ledger");
                        if let Some(st) = led.stagings.remove(&staging_id) {
                            let _ = led.tracker.free(st.accounted);
                        }
                        ticket.inner.consumed.store(true, Ordering::SeqCst);
                        data
                    }
                    Err(msg) if msg.contains("timed out") => {
                        // Bounded cancel: unmap terminates pending map_async
                        // (Waiting→Idle + MapAborted). Do **not** wait on rx.
                        buf.unmap();
                        let _ = device.poll(wgpu::PollType::Poll);
                        let _ = rx.try_recv(); // MapAborted or empty — never block
                        self.poll_host_lost();
                        if self.status() != ContextStatus::Live {
                            let mut led = self.resource_ledger.lock().expect("ledger");
                            if let Some(st) = led.stagings.get_mut(&staging_id) {
                                st.gpu = Some(GpuBuffer::Wgpu(buf));
                            }
                            drop(led);
                            return Err(handle_unavailable(self.status()));
                        }
                        let mut led = self.resource_ledger.lock().expect("ledger");
                        if let Some(st) = led.stagings.get_mut(&staging_id) {
                            st.gpu = Some(GpuBuffer::Wgpu(buf));
                        }
                        drop(led);
                        return Err(self.fail_gpu(
                            submit_err(format!(
                                "{msg} (staging retained idle for retry; pending map cancelled via unmap)"
                            )),
                            crate::diagnostics::GpuOperation::Map,
                            None,
                            None,
                            None,
                            None,
                        ));
                    }
                    Err(msg) if msg.contains("disconnected") => {
                        let mut led = self.resource_ledger.lock().expect("ledger");
                        if let Some(st) = led.stagings.get_mut(&staging_id) {
                            st.gpu = Some(GpuBuffer::Wgpu(buf));
                        }
                        drop(led);
                        return Err(self.fail_gpu(
                            submit_err(format!(
                                "{msg} (staging retained; GPU copy already complete)"
                            )),
                            crate::diagnostics::GpuOperation::Map,
                            None,
                            None,
                            None,
                            None,
                        ));
                    }
                    Err(msg) => {
                        // Definitive map failure after GPU copy complete: retire staging.
                        let mut led = self.resource_ledger.lock().expect("ledger");
                        if let Some(st) = led.stagings.remove(&staging_id) {
                            let _ = led.tracker.free(st.accounted);
                        }
                        ticket.inner.consumed.store(true, Ordering::SeqCst);
                        // buf drops without unmap if never mapped — ok for failed map_async
                        drop(buf);
                        drop(led);
                        return Err(self.fail_gpu(
                            submit_err(msg),
                            crate::diagnostics::GpuOperation::Map,
                            None,
                            None,
                            None,
                            None,
                        ));
                    }
                }
            }
            Some(GpuBuffer::Synthetic(data)) => {
                if force_deadline {
                    // No map_async on synthetic; restore and return deadline Err.
                    st.gpu = Some(GpuBuffer::Synthetic(data));
                    drop(led);
                    return Err(self.fail_gpu(
                        submit_err("map_readback deadline (staging retained for retry)"),
                        crate::diagnostics::GpuOperation::Map,
                        None,
                        None,
                        None,
                        None,
                    ));
                }
                let accounted = st.accounted;
                led.stagings.remove(&staging_id);
                let _ = led.tracker.free(accounted);
                ticket.inner.consumed.store(true, Ordering::SeqCst);
                data
            }
            None => return Err(gpu_lost("readback staging missing")),
        };

        if let Some((bpr, height)) = texture_pitch {
            let row_logical = (logical_len / u64::from(height)) as usize;
            let mut out = Vec::with_capacity(logical_len as usize);
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
