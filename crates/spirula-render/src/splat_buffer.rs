//! P4-F02 host splat packing and narrow GPU buffer ownership.
//!
//! The layout is deliberately explicit and independent of `wgpu`: one record
//! is four 16-byte lanes containing mean, quaternion, log scale, and
//! opacity/DC. Projection, bind groups, and kernels consume this contract in
//! later features.

use std::time::Duration;

use spirula_core::{ContextKey, DomainError, ErrorCode, SplatSet};
use spirula_gpu::{
    BufferDesc, BufferHandle, BufferUsage, ContextGeneration, ContextId, GpuContext, ResourceLabel,
    SubmissionHandle,
};

pub const SPLAT_GPU_LAYOUT_VERSION: u32 = 1;
pub const SPLAT_GPU_STRIDE_BYTES: u64 = 64;
const LANE_BYTES: u64 = 16;

fn error(code: ErrorCode, field: &str, message: impl Into<String>) -> DomainError {
    DomainError::try_new(code, message)
        .expect("splat resource error")
        .with_context(ContextKey::Field, field)
        .expect("splat resource field")
}

/// std430-compatible degree-0 Gaussian record offsets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SplatGpuLayout {
    pub version: u32,
    pub stride_bytes: u64,
    pub mean_offset: u64,
    pub rotation_offset: u64,
    pub scale_log_offset: u64,
    pub opacity_dc_offset: u64,
}

impl Default for SplatGpuLayout {
    fn default() -> Self {
        Self {
            version: SPLAT_GPU_LAYOUT_VERSION,
            stride_bytes: SPLAT_GPU_STRIDE_BYTES,
            mean_offset: 0,
            rotation_offset: LANE_BYTES,
            scale_log_offset: LANE_BYTES * 2,
            opacity_dc_offset: LANE_BYTES * 3,
        }
    }
}

impl SplatGpuLayout {
    pub fn validate(self) -> Result<(), DomainError> {
        if self.version != SPLAT_GPU_LAYOUT_VERSION {
            return Err(error(
                ErrorCode::UnsupportedVersion,
                "layout_version",
                "unsupported splat GPU layout version",
            ));
        }
        if self.stride_bytes != SPLAT_GPU_STRIDE_BYTES
            || !self.stride_bytes.is_multiple_of(LANE_BYTES)
            || self.mean_offset != 0
            || self.rotation_offset != LANE_BYTES
            || self.scale_log_offset != LANE_BYTES * 2
            || self.opacity_dc_offset != LANE_BYTES * 3
        {
            return Err(error(
                ErrorCode::ValueOutOfRange,
                "layout",
                "splat layout offsets or stride are not aligned",
            ));
        }
        Ok(())
    }
}

/// Packed little-endian host bytes ready for a storage buffer upload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackedSplatData {
    pub layout: SplatGpuLayout,
    pub splat_count: usize,
    pub bytes: Vec<u8>,
}

impl PackedSplatData {
    pub fn from_scene(scene: &SplatSet) -> Result<Self, DomainError> {
        let layout = SplatGpuLayout::default();
        layout.validate()?;
        if scene.is_empty() {
            return Err(error(
                ErrorCode::InvalidArgument,
                "scene",
                "cannot pack an empty splat scene",
            ));
        }
        let byte_len = scene
            .len()
            .checked_mul(usize::try_from(layout.stride_bytes).map_err(|_| {
                error(
                    ErrorCode::ValueOutOfRange,
                    "splat_count",
                    "splat byte count exceeds usize",
                )
            })?)
            .ok_or_else(|| {
                error(
                    ErrorCode::ValueOutOfRange,
                    "splat_count",
                    "byte count overflow",
                )
            })?;
        let mut bytes = vec![0_u8; byte_len];
        for (index, splat) in scene.splats().iter().enumerate() {
            let base = index * layout.stride_bytes as usize;
            put_vec4(
                &mut bytes,
                base + layout.mean_offset as usize,
                splat.mean().to_array(),
                0.0,
            )?;
            let q = splat.rotation().wxyz();
            put_vec4(
                &mut bytes,
                base + layout.rotation_offset as usize,
                [q[0], q[1], q[2]],
                q[3],
            )?;
            put_vec4(
                &mut bytes,
                base + layout.scale_log_offset as usize,
                splat.scale_log().to_array(),
                0.0,
            )?;
            let dc = splat.dc().to_array();
            put_vec4(
                &mut bytes,
                base + layout.opacity_dc_offset as usize,
                [splat.opacity_logit(), dc[0], dc[1]],
                dc[2],
            )?;
        }
        Ok(Self {
            layout,
            splat_count: scene.len(),
            bytes,
        })
    }

    pub fn byte_len(&self) -> u64 {
        self.bytes.len() as u64
    }

    /// Peak accounted bytes during upload: resident buffer plus upload staging.
    pub fn upload_peak_bytes(&self) -> Result<u64, DomainError> {
        self.byte_len().checked_mul(2).ok_or_else(|| {
            error(
                ErrorCode::ValueOutOfRange,
                "upload_bytes",
                "upload peak byte count overflow",
            )
        })
    }
}

fn put_vec4(
    bytes: &mut [u8],
    offset: usize,
    xyz: [f64; 3],
    fourth: f64,
) -> Result<(), DomainError> {
    let values = [xyz[0], xyz[1], xyz[2], fourth];
    for (index, value) in values.into_iter().enumerate() {
        let value = value as f32;
        if !value.is_finite() {
            return Err(error(
                ErrorCode::ValueOutOfRange,
                "splat_value",
                "splat value cannot be represented as finite f32",
            ));
        }
        bytes[offset + index * 4..offset + (index + 1) * 4].copy_from_slice(&value.to_le_bytes());
    }
    Ok(())
}

/// GPU storage-buffer owner. The contained handle is the narrow public GPU
/// boundary; no `wgpu` type crosses `spirula-render`.
pub struct GpuSplatBuffer {
    buffer: BufferHandle,
    layout: SplatGpuLayout,
    splat_count: usize,
    byte_len: u64,
    context_id: ContextId,
    generation: ContextGeneration,
    pending_upload: Option<SubmissionHandle>,
}

impl std::fmt::Debug for GpuSplatBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuSplatBuffer")
            .field("splat_count", &self.splat_count)
            .field("byte_len", &self.byte_len)
            .field("context_id", &self.context_id)
            .field("generation", &self.generation)
            .field("pending_upload", &self.pending_upload)
            .finish()
    }
}

impl GpuSplatBuffer {
    pub fn upload(context: &mut GpuContext, packed: &PackedSplatData) -> Result<Self, DomainError> {
        packed.layout.validate()?;
        if packed.splat_count == 0 || packed.bytes.is_empty() {
            return Err(error(
                ErrorCode::InvalidArgument,
                "scene",
                "cannot upload an empty splat scene",
            ));
        }
        let peak = packed.upload_peak_bytes()?;
        let accounting = context.resource_accounting();
        if let Some(limit) = accounting.budget_limit {
            let required = accounting.current_bytes.checked_add(peak).ok_or_else(|| {
                error(
                    ErrorCode::ValueOutOfRange,
                    "upload_bytes",
                    "budget overflow",
                )
            })?;
            if required > limit {
                return Err(error(
                    ErrorCode::ValueOutOfRange,
                    "upload_bytes",
                    format!("splat upload peak {required} exceeds GPU budget {limit}"),
                ));
            }
        }
        let label = ResourceLabel::parse("spirula:render:buffer:splat")?;
        let buffer = context.create_buffer(BufferDesc {
            label,
            size: packed.byte_len(),
            usage: BufferUsage::STORAGE_COPY,
        })?;
        if let Err(err) = context.upload_buffer(&buffer, 0, &packed.bytes) {
            context.abandon_unsubmitted_pending();
            return Err(err);
        }
        let submission = match context.submit() {
            Ok(submission) => submission,
            Err(err) => {
                context.abandon_unsubmitted_pending();
                return Err(err);
            }
        };
        Ok(Self {
            context_id: context.id(),
            generation: context.generation(),
            buffer,
            layout: packed.layout,
            splat_count: packed.splat_count,
            byte_len: packed.byte_len(),
            pending_upload: Some(submission),
        })
    }

    pub fn buffer(&self) -> &BufferHandle {
        &self.buffer
    }

    pub const fn layout(&self) -> SplatGpuLayout {
        self.layout
    }

    pub const fn splat_count(&self) -> usize {
        self.splat_count
    }

    pub const fn byte_len(&self) -> u64 {
        self.byte_len
    }

    pub fn is_upload_pending(&self) -> bool {
        self.pending_upload.is_some()
    }

    pub fn wait_ready(
        &mut self,
        context: &mut GpuContext,
        timeout: Option<Duration>,
    ) -> Result<(), DomainError> {
        self.validate_current(context)?;
        let Some(submission) = self.pending_upload else {
            return Ok(());
        };
        context.wait(submission, timeout)?;
        self.pending_upload = None;
        Ok(())
    }

    pub fn validate_current(&self, context: &mut GpuContext) -> Result<(), DomainError> {
        if context.id() != self.context_id {
            return Err(error(
                ErrorCode::InvalidArgument,
                "context_id",
                "splat buffer belongs to a different context",
            ));
        }
        if context.generation() != self.generation {
            return Err(DomainError::try_new(
                ErrorCode::DeviceLost,
                "splat buffer generation is stale",
            )
            .expect("stale generation"));
        }
        context.device_handle().map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spirula_core::{GaussianSplat, Quat, Vec3};
    use spirula_gpu::{
        AdapterCapabilities, AdapterClass, AdapterIdentity, AdapterLimits, BackendKind,
        CapabilityRequest, FeatureBits, FieldAvailability,
    };

    fn scene() -> SplatSet {
        SplatSet::try_from_splats(vec![
            GaussianSplat::try_new(
                Vec3::try_new(1.0, 2.0, 3.0).unwrap(),
                Quat::identity(),
                Vec3::try_new(-1.0, -2.0, -3.0).unwrap(),
                0.25,
                Vec3::try_new(0.1, 0.2, 0.3).unwrap(),
            )
            .unwrap(),
        ])
        .unwrap()
    }

    fn adapter() -> AdapterCapabilities {
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
                max_compute_workgroup_storage_size: 16_384,
            },
        }
    }

    fn open_context() -> GpuContext {
        GpuContext::open_synthetic(&CapabilityRequest::diagnose_default(), adapter(), false)
            .expect("synthetic context")
    }

    #[test]
    fn packs_explicit_64_byte_record_and_releases_accounting() {
        let packed = PackedSplatData::from_scene(&scene()).unwrap();
        assert_eq!(packed.layout, SplatGpuLayout::default());
        assert_eq!(packed.splat_count, 1);
        assert_eq!(packed.bytes.len(), 64);
        assert_eq!(packed.upload_peak_bytes().unwrap(), 128);

        let mut context = open_context();
        context.set_resource_budget(Some(128));
        let mut gpu = GpuSplatBuffer::upload(&mut context, &packed).unwrap();
        assert!(gpu.is_upload_pending());
        gpu.wait_ready(&mut context, None).unwrap();
        assert!(!gpu.is_upload_pending());
        assert_eq!(context.resource_accounting().current_bytes, 64);
        drop(gpu);
        assert_eq!(context.resource_accounting().current_bytes, 0);
    }

    #[test]
    fn budget_preflight_and_f32_overflow_are_typed() {
        let packed = PackedSplatData::from_scene(&scene()).unwrap();
        let mut context = open_context();
        context.set_resource_budget(Some(127));
        let err = GpuSplatBuffer::upload(&mut context, &packed).unwrap_err();
        assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
        assert_eq!(context.resource_accounting().current_bytes, 0);

        let huge = SplatSet::try_from_splats(vec![
            GaussianSplat::try_new(
                Vec3::try_new(1e300, 0.0, 0.0).unwrap(),
                Quat::identity(),
                Vec3::try_new(0.0, 0.0, 0.0).unwrap(),
                0.0,
                Vec3::try_new(0.0, 0.0, 0.0).unwrap(),
            )
            .unwrap(),
        ])
        .unwrap();
        let err = PackedSplatData::from_scene(&huge).unwrap_err();
        assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
    }

    #[test]
    fn stale_generation_and_cross_context_are_rejected() {
        let packed = PackedSplatData::from_scene(&scene()).unwrap();
        let mut context = open_context();
        let mut other = open_context();
        let mut gpu = GpuSplatBuffer::upload(&mut context, &packed).unwrap();
        gpu.wait_ready(&mut context, None).unwrap();
        let err = gpu.validate_current(&mut other).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidArgument);
        context
            .reopen_synthetic(&CapabilityRequest::diagnose_default(), adapter(), false)
            .unwrap();
        let err = gpu.validate_current(&mut context).unwrap_err();
        assert_eq!(err.code(), ErrorCode::DeviceLost);
    }
}
