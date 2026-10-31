//! Bounded host-only training image loader (P3-F08).
//!
//! The loader deliberately stops at decoded RGB8 host bytes. It owns a bounded
//! task queue and decode workers; no application, training, UI, or GPU types
//! cross this module. A frame's expected `width * height * 3` bytes are
//! reserved before enqueue and released after decode, while packed cache bytes
//! are accounted independently.

use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::Value;
use spirula_core::{CancelToken, ContextKey, DomainError, ErrorCode};

use crate::colmap::{LoadedFrame, ParsedDataset};
use crate::decode_rgb8;

const STATE_WAIT: Duration = Duration::from_millis(10);
const SAMPLER_VERSION: u32 = 1;

fn err(code: ErrorCode, field: &str, message: impl Into<String>) -> DomainError {
    DomainError::try_new(code, message)
        .expect("bounded loader error message")
        .with_context(ContextKey::Field, field)
        .expect("bounded loader field")
}

fn canceled(cancel: Option<&CancelToken>) -> Result<(), DomainError> {
    if let Some(cancel) = cancel {
        cancel.check()?;
    }
    Ok(())
}

/// Configuration for [`BoundedDatasetLoader`]. All limits are hard caps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundedLoaderConfig {
    /// Maximum packed RGB8 bytes retained in the cache.
    pub cache_byte_cap: u64,
    /// Maximum bytes reserved by queued and currently decoding frames.
    pub in_flight_decode_byte_cap: u64,
    /// Maximum number of decode tasks accepted but not yet picked by a worker.
    pub prefetch_queue_cap: usize,
    /// Number of owned decode workers.
    pub worker_count: usize,
    /// Seed used by the deterministic train sampler.
    pub seed: u64,
}

impl Default for BoundedLoaderConfig {
    fn default() -> Self {
        Self {
            cache_byte_cap: 64 * 1024 * 1024,
            in_flight_decode_byte_cap: 128 * 1024 * 1024,
            prefetch_queue_cap: 2,
            worker_count: 2,
            seed: 0,
        }
    }
}

impl BoundedLoaderConfig {
    fn validate(&self) -> Result<(), DomainError> {
        if self.cache_byte_cap == 0 {
            return Err(err(
                ErrorCode::ValueOutOfRange,
                "cache_byte_cap",
                "must be greater than zero",
            ));
        }
        if self.in_flight_decode_byte_cap == 0 {
            return Err(err(
                ErrorCode::ValueOutOfRange,
                "in_flight_decode_byte_cap",
                "must be greater than zero",
            ));
        }
        if self.prefetch_queue_cap == 0 {
            return Err(err(
                ErrorCode::ValueOutOfRange,
                "prefetch_queue_cap",
                "must be greater than zero",
            ));
        }
        if self.worker_count == 0 {
            return Err(err(
                ErrorCode::ValueOutOfRange,
                "worker_count",
                "must be greater than zero",
            ));
        }
        Ok(())
    }
}

/// Serializable deterministic sampler state. `frame_indices` is the complete
/// ordered epoch, so a state can be rebuilt without relying on worker queues.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SamplerState {
    pub version: u32,
    pub seed: u64,
    pub epoch: u64,
    pub cursor: usize,
    pub frame_indices: Vec<usize>,
}

impl SamplerState {
    /// Build epoch zero from train indices and deterministically shuffle them.
    pub fn new(seed: u64, train_indices: &[usize]) -> Result<Self, DomainError> {
        if train_indices.is_empty() {
            return Err(err(
                ErrorCode::ValueOutOfRange,
                "train",
                "train split is empty",
            ));
        }
        let mut state = Self {
            version: SAMPLER_VERSION,
            seed,
            epoch: 0,
            cursor: 0,
            frame_indices: train_indices.to_vec(),
        };
        state.shuffle();
        Ok(state)
    }

    pub fn from_dataset(dataset: &ParsedDataset, seed: u64) -> Result<Self, DomainError> {
        Self::new(seed, &dataset.split.train)
    }

    pub fn remaining(&self) -> usize {
        self.frame_indices.len().saturating_sub(self.cursor)
    }

    pub fn peek(&self) -> Option<usize> {
        self.frame_indices.get(self.cursor).copied()
    }

    /// Advance only after the corresponding frame has been successfully
    /// delivered. Failed decode/cache operations therefore never move cursor.
    pub fn advance(&mut self) -> Option<usize> {
        let index = self.peek()?;
        self.cursor += 1;
        Some(index)
    }

    pub fn next_index(&mut self) -> Option<usize> {
        self.advance()
    }

    /// Start the next epoch and reset its cursor.
    pub fn next_epoch(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.cursor = 0;
        self.shuffle();
    }

    /// Set an epoch and derive the same order as any other process with the
    /// same seed and source frame list.
    pub fn set_epoch(&mut self, epoch: u64) {
        self.epoch = epoch;
        self.cursor = 0;
        self.shuffle();
    }

    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "version": self.version,
            "seed": self.seed,
            "epoch": self.epoch,
            "cursor": self.cursor,
            "frame_indices": self.frame_indices,
        })
    }

    pub fn from_json(value: &Value) -> Result<Self, DomainError> {
        let object = value.as_object().ok_or_else(|| {
            err(
                ErrorCode::MalformedRecord,
                "sampler",
                "sampler state must be an object",
            )
        })?;
        let version = object
            .get("version")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                err(
                    ErrorCode::MalformedRecord,
                    "version",
                    "sampler state version is missing",
                )
            })?;
        if version != u64::from(SAMPLER_VERSION) {
            return Err(err(
                ErrorCode::UnsupportedVersion,
                "version",
                format!("sampler state version {version} is unsupported"),
            ));
        }
        let seed = object.get("seed").and_then(Value::as_u64).ok_or_else(|| {
            err(
                ErrorCode::MalformedRecord,
                "seed",
                "sampler seed is missing",
            )
        })?;
        let epoch = object.get("epoch").and_then(Value::as_u64).ok_or_else(|| {
            err(
                ErrorCode::MalformedRecord,
                "epoch",
                "sampler epoch is missing",
            )
        })?;
        let cursor = object
            .get("cursor")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                err(
                    ErrorCode::MalformedRecord,
                    "cursor",
                    "sampler cursor is missing",
                )
            })?;
        let cursor = usize::try_from(cursor).map_err(|_| {
            err(
                ErrorCode::ValueOutOfRange,
                "cursor",
                "sampler cursor exceeds usize",
            )
        })?;
        let indices = object
            .get("frame_indices")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                err(
                    ErrorCode::MalformedRecord,
                    "frame_indices",
                    "sampler indices are missing",
                )
            })?;
        let mut frame_indices = Vec::with_capacity(indices.len());
        for value in indices {
            let index = value.as_u64().ok_or_else(|| {
                err(
                    ErrorCode::MalformedRecord,
                    "frame_indices",
                    "sampler index is not an integer",
                )
            })?;
            frame_indices.push(usize::try_from(index).map_err(|_| {
                err(
                    ErrorCode::ValueOutOfRange,
                    "frame_indices",
                    "sampler index exceeds usize",
                )
            })?);
        }
        if frame_indices.is_empty() || cursor > frame_indices.len() {
            return Err(err(
                ErrorCode::ValueOutOfRange,
                "cursor",
                "sampler state has an empty order or cursor beyond its end",
            ));
        }
        Ok(Self {
            version: version as u32,
            seed,
            epoch,
            cursor,
            frame_indices,
        })
    }

    fn shuffle(&mut self) {
        // Canonicalize the source order before each epoch so set_epoch(e) and
        // next_epoch() produce the same order from any reconstructed state.
        self.frame_indices.sort_unstable();
        let mut rng = self.seed ^ self.epoch.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        for i in (1..self.frame_indices.len()).rev() {
            rng = splitmix64(rng);
            let j = (rng % (i as u64 + 1)) as usize;
            self.frame_indices.swap(i, j);
        }
    }
}

fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Decoded train image returned by [`BoundedDatasetLoader`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedRgbFrame {
    pub frame_index: usize,
    pub image_id: i32,
    pub width: u32,
    pub height: u32,
    pub path: PathBuf,
    pub rgb: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LoaderMetrics {
    pub cache_bytes: u64,
    pub in_flight_bytes: u64,
    pub queue_count: usize,
    pub decode_worker_count: usize,
    pub cache_entries: usize,
    pub cache_hits: u64,
    pub cache_misses: u64,
}

struct CacheItem {
    bytes: Arc<Vec<u8>>,
}

struct LoaderState {
    cache: HashMap<usize, CacheItem>,
    lru: VecDeque<usize>,
    cache_bytes: u64,
    in_flight_bytes: u64,
    queue_count: usize,
    pending: HashSet<usize>,
    failures: VecDeque<(usize, DomainError)>,
    reported_failures: HashSet<usize>,
    cache_hits: u64,
    cache_misses: u64,
    live_workers: usize,
}

impl LoaderState {
    fn new(worker_count: usize) -> Self {
        Self {
            cache: HashMap::new(),
            lru: VecDeque::new(),
            cache_bytes: 0,
            in_flight_bytes: 0,
            queue_count: 0,
            pending: HashSet::new(),
            failures: VecDeque::new(),
            reported_failures: HashSet::new(),
            cache_hits: 0,
            cache_misses: 0,
            live_workers: worker_count,
        }
    }

    fn touch(&mut self, index: usize) {
        self.lru.retain(|candidate| *candidate != index);
        self.lru.push_back(index);
    }
}

struct Task {
    index: usize,
    reserved_bytes: u64,
}

struct Inner {
    dataset: Arc<ParsedDataset>,
    config: BoundedLoaderConfig,
    state: Mutex<LoaderState>,
    changed: Condvar,
    queue: Mutex<VecDeque<Task>>,
    queue_changed: Condvar,
    stopping: AtomicBool,
    cancel: CancelToken,
}

/// Host-only bounded RGB8 decoder and packed-byte cache.
pub struct BoundedDatasetLoader {
    inner: Arc<Inner>,
    workers: Vec<JoinHandle<()>>,
}

impl std::fmt::Debug for BoundedDatasetLoader {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BoundedDatasetLoader")
            .field("metrics", &self.metrics())
            .finish()
    }
}

impl BoundedDatasetLoader {
    pub fn new(dataset: ParsedDataset, config: BoundedLoaderConfig) -> Result<Self, DomainError> {
        config.validate()?;
        if dataset.split.train.is_empty() {
            return Err(err(
                ErrorCode::ValueOutOfRange,
                "train",
                "train split is empty",
            ));
        }
        for &index in &dataset.split.train {
            let frame = dataset.frames.get(index).ok_or_else(|| {
                err(
                    ErrorCode::MalformedRecord,
                    "train",
                    format!("train index {index} is out of range"),
                )
            })?;
            let bytes = expected_bytes(frame)?;
            if bytes > config.cache_byte_cap {
                return Err(err(
                    ErrorCode::ValueOutOfRange,
                    "cache_byte_cap",
                    format!(
                        "train frame {index} needs {bytes} bytes, over cache cap {}",
                        config.cache_byte_cap
                    ),
                ));
            }
            if bytes > config.in_flight_decode_byte_cap {
                return Err(err(
                    ErrorCode::ValueOutOfRange,
                    "in_flight_decode_byte_cap",
                    format!(
                        "train frame {index} needs {bytes} bytes, over in-flight cap {}",
                        config.in_flight_decode_byte_cap
                    ),
                ));
            }
        }
        let inner = Arc::new(Inner {
            dataset: Arc::new(dataset),
            config: config.clone(),
            state: Mutex::new(LoaderState::new(config.worker_count)),
            changed: Condvar::new(),
            queue: Mutex::new(VecDeque::new()),
            queue_changed: Condvar::new(),
            stopping: AtomicBool::new(false),
            cancel: CancelToken::new(),
        });
        let mut workers = Vec::with_capacity(config.worker_count);
        for _ in 0..config.worker_count {
            let worker_inner = Arc::clone(&inner);
            workers.push(thread::spawn(move || worker_loop(worker_inner)));
        }
        Ok(Self { inner, workers })
    }

    pub fn from_dataset(
        dataset: ParsedDataset,
        config: BoundedLoaderConfig,
    ) -> Result<Self, DomainError> {
        Self::new(dataset, config)
    }

    pub fn dataset(&self) -> &ParsedDataset {
        &self.inner.dataset
    }

    pub fn new_sampler(&self) -> Result<SamplerState, DomainError> {
        SamplerState::from_dataset(&self.inner.dataset, self.inner.config.seed)
    }

    pub fn cancel_token(&self) -> CancelToken {
        self.inner.cancel.clone()
    }

    pub fn metrics(&self) -> LoaderMetrics {
        let state = lock_state(&self.inner.state);
        LoaderMetrics {
            cache_bytes: state.cache_bytes,
            in_flight_bytes: state.in_flight_bytes,
            queue_count: state.queue_count,
            decode_worker_count: state.live_workers,
            cache_entries: state.cache.len(),
            cache_hits: state.cache_hits,
            cache_misses: state.cache_misses,
        }
    }

    /// Queue up to `count` upcoming sampler indices. Cursor is unchanged;
    /// successful delivery through [`Self::next`] advances it.
    pub fn prefetch(
        &self,
        sampler: &SamplerState,
        count: usize,
        cancel: Option<&CancelToken>,
    ) -> Result<usize, DomainError> {
        canceled(cancel)?;
        let mut accepted = 0;
        for offset in 0..count {
            canceled(cancel)?;
            let Some(&index) =
                sampler
                    .frame_indices
                    .get(sampler.cursor.checked_add(offset).ok_or_else(|| {
                        err(
                            ErrorCode::ValueOutOfRange,
                            "cursor",
                            "sampler cursor overflow",
                        )
                    })?)
            else {
                break;
            };
            match self.schedule(index, cancel) {
                Ok(true) => accepted += 1,
                Ok(false) => {}
                Err(error) => {
                    if accepted > 0 && is_backpressure(&error) {
                        break;
                    }
                    return Err(error);
                }
            }
        }
        Ok(accepted)
    }

    /// Wait until all queued and in-flight decode work is accounted as done.
    pub fn drain_prefetch(&self, cancel: Option<&CancelToken>) -> Result<(), DomainError> {
        loop {
            canceled(cancel)?;
            let mut state = lock_state(&self.inner.state);
            if state.queue_count == 0 && state.in_flight_bytes == 0 {
                if let Some((_, error)) = state.failures.pop_front() {
                    return Err(error);
                }
                return Ok(());
            }
            state = wait_state(&self.inner.changed, state);
            drop(state);
        }
    }

    /// Rebuild queued work from a sampler state after draining old work.
    pub fn rebuild_prefetch(
        &self,
        sampler: &SamplerState,
        count: usize,
        cancel: Option<&CancelToken>,
    ) -> Result<usize, DomainError> {
        self.drain_prefetch(cancel)?;
        self.prefetch(sampler, count, cancel)
    }

    pub fn next(
        &self,
        sampler: &mut SamplerState,
        cancel: Option<&CancelToken>,
    ) -> Result<LoadedRgbFrame, DomainError> {
        canceled(cancel)?;
        let index = sampler.peek().ok_or_else(|| {
            err(
                ErrorCode::InvalidState,
                "cursor",
                "sampler epoch is exhausted",
            )
        })?;
        let frame = self.get(index, cancel)?;
        sampler.advance();
        Ok(frame)
    }

    pub fn next_batch(
        &self,
        sampler: &mut SamplerState,
        batch_size: usize,
        cancel: Option<&CancelToken>,
    ) -> Result<Vec<LoadedRgbFrame>, DomainError> {
        if batch_size == 0 {
            return Err(err(
                ErrorCode::ValueOutOfRange,
                "batch_size",
                "must be greater than zero",
            ));
        }
        let mut batch = Vec::with_capacity(batch_size);
        for _ in 0..batch_size {
            canceled(cancel)?;
            if sampler.peek().is_none() {
                break;
            }
            batch.push(self.next(sampler, cancel)?);
        }
        if batch.is_empty() {
            return Err(err(
                ErrorCode::InvalidState,
                "cursor",
                "sampler epoch is exhausted",
            ));
        }
        Ok(batch)
    }

    /// Retry a failed decode once the source has been repaired.
    pub fn retry_failed(&self, index: usize) -> Result<bool, DomainError> {
        let mut state = lock_state(&self.inner.state);
        let before = state.failures.len();
        state.failures.retain(|(candidate, _)| *candidate != index);
        let reported = state.reported_failures.remove(&index);
        Ok(reported || before != state.failures.len())
    }

    fn schedule(&self, index: usize, cancel: Option<&CancelToken>) -> Result<bool, DomainError> {
        canceled(cancel)?;
        let frame = self.inner.dataset.frames.get(index).ok_or_else(|| {
            err(
                ErrorCode::ValueOutOfRange,
                "frame_index",
                format!("frame index {index} is out of range"),
            )
        })?;
        let bytes = expected_bytes(frame)?;
        let mut state = lock_state(&self.inner.state);
        if state.cache.contains_key(&index) || state.pending.contains(&index) {
            return Ok(false);
        }
        if state.queue_count >= self.inner.config.prefetch_queue_cap {
            return Err(err(
                ErrorCode::ValueOutOfRange,
                "prefetch_queue_cap",
                "prefetch queue is full",
            ));
        }
        if state.in_flight_bytes.saturating_add(bytes) > self.inner.config.in_flight_decode_byte_cap
        {
            return Err(err(
                ErrorCode::ValueOutOfRange,
                "in_flight_decode_byte_cap",
                "in-flight decode byte cap exceeded",
            ));
        }
        let task = Task {
            index,
            reserved_bytes: bytes,
        };
        state.queue_count += 1;
        state.in_flight_bytes += bytes;
        state.pending.insert(index);
        drop(state);
        if self.inner.stopping.load(Ordering::Acquire) {
            self.release_failed_task(&task);
            return Err(err(
                ErrorCode::InvalidState,
                "loader",
                "loader is shutting down",
            ));
        }
        {
            let mut queue = lock_state(&self.inner.queue);
            if queue.len() >= self.inner.config.prefetch_queue_cap {
                drop(queue);
                self.release_failed_task(&task);
                return Err(err(
                    ErrorCode::ValueOutOfRange,
                    "prefetch_queue_cap",
                    "prefetch queue is full",
                ));
            }
            queue.push_back(task);
        }
        self.inner.queue_changed.notify_one();
        Ok(true)
    }

    fn release_failed_task(&self, task: &Task) {
        let mut state = lock_state(&self.inner.state);
        state.queue_count = state.queue_count.saturating_sub(1);
        state.in_flight_bytes = state.in_flight_bytes.saturating_sub(task.reserved_bytes);
        state.pending.remove(&task.index);
        self.inner.changed.notify_all();
    }

    fn get(
        &self,
        index: usize,
        cancel: Option<&CancelToken>,
    ) -> Result<LoadedRgbFrame, DomainError> {
        if self.inner.dataset.frames.get(index).is_none() {
            return Err(err(
                ErrorCode::ValueOutOfRange,
                "frame_index",
                format!("frame index {index} is out of range"),
            ));
        }
        loop {
            canceled(cancel)?;
            self.inner.cancel.check()?;
            {
                let mut state = lock_state(&self.inner.state);
                if let Some(item) = state.cache.remove(&index) {
                    let rgb = (*item.bytes).clone();
                    state.cache.insert(index, item);
                    state.touch(index);
                    state.cache_hits += 1;
                    return make_frame(&self.inner.dataset.frames[index], index, rgb);
                }
                if let Some(position) = state
                    .failures
                    .iter()
                    .position(|(candidate, _)| *candidate == index)
                {
                    let (_, error) = state.failures.remove(position).expect("failure position");
                    state.reported_failures.insert(index);
                    return Err(error);
                }
                state.cache_misses += 1;
                if state.live_workers == 0 && !state.pending.contains(&index) {
                    return Err(err(
                        ErrorCode::Internal,
                        "worker",
                        "all decode workers stopped",
                    ));
                }
            }
            match self.schedule(index, cancel) {
                Ok(_) => {}
                Err(error) if is_backpressure(&error) => {}
                Err(error) => return Err(error),
            }
            let state = lock_state(&self.inner.state);
            drop(wait_state_timeout(&self.inner.changed, state));
        }
    }
}

impl Drop for BoundedDatasetLoader {
    fn drop(&mut self) {
        self.inner.stopping.store(true, Ordering::Release);
        self.inner
            .cancel
            .request(spirula_core::CancelReason::Shutdown);
        self.inner.queue_changed.notify_all();
        self.inner.changed.notify_all();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
        let mut state = lock_state(&self.inner.state);
        state.cache.clear();
        state.lru.clear();
        state.cache_bytes = 0;
        state.in_flight_bytes = 0;
        state.queue_count = 0;
        state.pending.clear();
        self.inner.changed.notify_all();
    }
}

fn worker_loop(inner: Arc<Inner>) {
    loop {
        let task = loop {
            if inner.stopping.load(Ordering::Acquire) || inner.cancel.is_cancelled() {
                break None;
            }
            let mut queue = lock_state(&inner.queue);
            if let Some(task) = queue.pop_front() {
                break Some(task);
            }
            queue = inner
                .queue_changed
                .wait_timeout(queue, STATE_WAIT)
                .map(|(queue, _)| queue)
                .unwrap_or_else(|poisoned| poisoned.into_inner().0);
            drop(queue);
        };
        let Some(task) = task else { break };
        {
            let mut state = lock_state(&inner.state);
            state.queue_count = state.queue_count.saturating_sub(1);
            inner.changed.notify_all();
        }
        if inner.stopping.load(Ordering::Acquire) || inner.cancel.is_cancelled() {
            finish_task(&inner, task, None::<DomainError>);
            continue;
        }
        let frame = match inner.dataset.frames.get(task.index) {
            Some(frame) => frame,
            None => {
                finish_task(
                    &inner,
                    task,
                    Some(err(
                        ErrorCode::ValueOutOfRange,
                        "frame_index",
                        "frame index is out of range",
                    )),
                );
                continue;
            }
        };
        let result = catch_unwind(AssertUnwindSafe(|| decode_rgb8(&frame.path)));
        let result = match result {
            Ok(result) => result,
            Err(_) => Err(err(ErrorCode::Internal, "decode", "image decoder panicked")),
        };
        if inner.cancel.is_cancelled() || inner.stopping.load(Ordering::Acquire) {
            finish_task(
                &inner,
                task,
                Some(
                    inner
                        .cancel
                        .check()
                        .err()
                        .unwrap_or_else(|| err(ErrorCode::Canceled, "cancel", "loader stopped")),
                ),
            );
            continue;
        }
        match result {
            Ok((width, height, rgb)) => {
                let expected = expected_bytes(frame).unwrap_or(0);
                if width != frame.width || height != frame.height || rgb.len() as u64 != expected {
                    finish_task(
                        &inner,
                        task,
                        Some(err(
                            ErrorCode::MalformedRecord,
                            "image",
                            "decoded RGB8 size differs from frame metadata",
                        )),
                    );
                } else {
                    finish_task(&inner, task, Some(Decoded { rgb }));
                }
            }
            Err(error) => finish_task(&inner, task, Some(error)),
        }
    }
    let drained = {
        let mut queue = lock_state(&inner.queue);
        queue.drain(..).collect::<Vec<_>>()
    };
    if !drained.is_empty() {
        let mut state = lock_state(&inner.state);
        state.queue_count = state.queue_count.saturating_sub(drained.len());
        inner.changed.notify_all();
    }
    for task in drained {
        finish_task(&inner, task, None::<DomainError>);
    }
    let mut state = lock_state(&inner.state);
    state.live_workers = state.live_workers.saturating_sub(1);
    inner.changed.notify_all();
}

#[derive(Debug)]
struct Decoded {
    rgb: Vec<u8>,
}

fn finish_task(inner: &Arc<Inner>, task: Task, result: Option<impl IntoTaskResult>) {
    let result = result.map(IntoTaskResult::into_result);
    let mut state = lock_state(&inner.state);
    state.in_flight_bytes = state.in_flight_bytes.saturating_sub(task.reserved_bytes);
    state.pending.remove(&task.index);
    match result {
        Some(Ok(decoded)) => {
            let bytes = decoded.rgb.len() as u64;
            if bytes <= inner.config.cache_byte_cap {
                while state.cache_bytes.saturating_add(bytes) > inner.config.cache_byte_cap {
                    let Some(oldest) = state.lru.pop_front() else {
                        break;
                    };
                    if let Some(old) = state.cache.remove(&oldest) {
                        state.cache_bytes =
                            state.cache_bytes.saturating_sub(old.bytes.len() as u64);
                    }
                }
                let bytes = Arc::new(decoded.rgb);
                state.cache_bytes += bytes.len() as u64;
                state.cache.insert(task.index, CacheItem { bytes });
                state.touch(task.index);
            } else {
                state.failures.push_back((
                    task.index,
                    err(
                        ErrorCode::ValueOutOfRange,
                        "cache_byte_cap",
                        "decoded frame exceeds cache cap",
                    ),
                ));
            }
        }
        Some(Err(error)) => state.failures.push_back((task.index, error)),
        None => {}
    }
    inner.changed.notify_all();
}

trait IntoTaskResult {
    fn into_result(self) -> Result<Decoded, DomainError>;
}

impl IntoTaskResult for Decoded {
    fn into_result(self) -> Result<Decoded, DomainError> {
        Ok(self)
    }
}

impl IntoTaskResult for DomainError {
    fn into_result(self) -> Result<Decoded, DomainError> {
        Err(self)
    }
}

fn is_backpressure(error: &DomainError) -> bool {
    matches!(
        error.context_value(ContextKey::Field),
        Some("prefetch_queue_cap") | Some("in_flight_decode_byte_cap")
    )
}

fn expected_bytes(frame: &LoadedFrame) -> Result<u64, DomainError> {
    u64::from(frame.width)
        .checked_mul(u64::from(frame.height))
        .and_then(|n| n.checked_mul(3))
        .ok_or_else(|| {
            err(
                ErrorCode::ValueOutOfRange,
                "image_size",
                "RGB8 byte count overflow",
            )
        })
}

fn make_frame(
    frame: &LoadedFrame,
    index: usize,
    rgb: Vec<u8>,
) -> Result<LoadedRgbFrame, DomainError> {
    Ok(LoadedRgbFrame {
        frame_index: index,
        image_id: frame.image_id,
        width: frame.width,
        height: frame.height,
        path: frame.path.clone(),
        rgb,
    })
}

fn lock_state<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn wait_state<'a>(
    condvar: &'a Condvar,
    state: MutexGuard<'a, LoaderState>,
) -> MutexGuard<'a, LoaderState> {
    condvar
        .wait_timeout(state, STATE_WAIT)
        .map(|(state, _)| state)
        .unwrap_or_else(|poisoned| poisoned.into_inner().0)
}

fn wait_state_timeout<'a>(
    condvar: &'a Condvar,
    state: MutexGuard<'a, LoaderState>,
) -> MutexGuard<'a, LoaderState> {
    wait_state(condvar, state)
}
