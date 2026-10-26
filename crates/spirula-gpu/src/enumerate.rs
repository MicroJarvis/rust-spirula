//! Real wgpu adapter enumeration → narrow [`AdapterCapabilities`] reports.

use crate::adapter::{
    AdapterCapabilities, AdapterClass, AdapterIdentity, AdapterLimits, BackendKind, FeatureBits,
    FieldAvailability,
};
use spirula_core::{DomainError, ErrorCode};

/// wgpu crate version recorded in diagnostics (must match Cargo.toml pin).
pub const WGPU_VERSION: &str = "30.0.1";

/// Enumerate host adapters through wgpu. Empty is success-with-zero, not an
/// error — callers decide whether that becomes `SP-GPU-001` on select.
pub fn enumerate_host_adapters() -> Result<Vec<AdapterCapabilities>, DomainError> {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = wgpu::Backends::all();
    let instance = wgpu::Instance::new(desc);
    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    let mut out = Vec::with_capacity(adapters.len());
    for (index, adapter) in adapters.into_iter().enumerate() {
        out.push(map_adapter(index, &adapter));
    }
    Ok(out)
}

fn map_adapter(index: usize, adapter: &wgpu::Adapter) -> AdapterCapabilities {
    let info = adapter.get_info();
    let features = adapter.features();
    let limits = adapter.limits();
    AdapterCapabilities {
        identity: AdapterIdentity {
            index,
            name: info.name,
            backend: map_backend(info.backend),
            class: map_class(info.device_type),
            vendor: info.vendor,
            device: info.device,
            driver: info.driver,
            driver_info: info.driver_info,
            dedicated_vram_bytes: FieldAvailability::Unavailable {
                reason: "wgpu exposes no portable dedicated-VRAM query on AdapterInfo",
            },
            uuid: FieldAvailability::Unavailable {
                reason: "wgpu exposes no portable adapter UUID",
            },
        },
        features: FeatureBits {
            subgroup: features.contains(wgpu::Features::SUBGROUP),
            shader_f16: features.contains(wgpu::Features::SHADER_F16),
            timestamp_query: features.contains(wgpu::Features::TIMESTAMP_QUERY),
            float32_atomic_add: features.contains(wgpu::Features::SHADER_FLOAT32_ATOMIC),
        },
        limits: AdapterLimits {
            max_buffer_size: limits.max_buffer_size,
            max_storage_buffers_per_shader_stage: limits.max_storage_buffers_per_shader_stage,
            max_compute_workgroup_storage_size: limits.max_compute_workgroup_storage_size,
        },
    }
}

fn map_backend(backend: wgpu::Backend) -> BackendKind {
    match backend {
        wgpu::Backend::Vulkan => BackendKind::Vulkan,
        wgpu::Backend::Metal => BackendKind::Metal,
        wgpu::Backend::Dx12 => BackendKind::Dx12,
        wgpu::Backend::Gl => BackendKind::Gl,
        wgpu::Backend::BrowserWebGpu => BackendKind::BrowserWebGpu,
        wgpu::Backend::Noop => BackendKind::Noop,
    }
}

fn map_class(device_type: wgpu::DeviceType) -> AdapterClass {
    match device_type {
        wgpu::DeviceType::DiscreteGpu => AdapterClass::DiscreteGpu,
        wgpu::DeviceType::IntegratedGpu => AdapterClass::IntegratedGpu,
        wgpu::DeviceType::Cpu => AdapterClass::Cpu,
        wgpu::DeviceType::Other | wgpu::DeviceType::VirtualGpu => AdapterClass::Other,
    }
}

/// Map a domain error to process exit for the diagnose binary.
pub fn diagnose_exit_code(err: &DomainError) -> u8 {
    match err.code() {
        ErrorCode::InvalidArgument | ErrorCode::UnknownCommand | ErrorCode::UnimplementedOption => {
            2
        }
        _ => 1,
    }
}
