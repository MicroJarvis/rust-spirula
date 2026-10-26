//! Real wgpu adapter enumeration → narrow [`AdapterCapabilities`] reports.

use crate::adapter::{
    AdapterCapabilities, AdapterClass, AdapterIdentity, AdapterLimits, BackendKind, FeatureBits,
    FieldAvailability,
};
use spirula_core::{DomainError, ErrorCode};

/// wgpu crate version recorded in diagnostics (must match Cargo.toml pin).
pub const WGPU_VERSION: &str = "30.0.1";

/// Create a headless instance with all backends enabled.
pub(crate) fn new_instance() -> wgpu::Instance {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = wgpu::Backends::all();
    wgpu::Instance::new(desc)
}

/// Enumerate adapters on an existing instance (paired reports + live adapters).
pub(crate) fn enumerate_on_instance(
    instance: &wgpu::Instance,
) -> (Vec<AdapterCapabilities>, Vec<wgpu::Adapter>) {
    let live = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    let mut reports = Vec::with_capacity(live.len());
    for (index, adapter) in live.iter().enumerate() {
        reports.push(map_adapter(index, adapter));
    }
    (reports, live)
}

/// Enumerate host adapters through wgpu. Empty is success-with-zero, not an
/// error — callers decide whether that becomes `SP-GPU-001` on select.
pub fn enumerate_host_adapters() -> Result<Vec<AdapterCapabilities>, DomainError> {
    let instance = new_instance();
    let (reports, _) = enumerate_on_instance(&instance);
    Ok(reports)
}

pub(crate) fn map_adapter(index: usize, adapter: &wgpu::Adapter) -> AdapterCapabilities {
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

/// Match a P2-F01 selection identity to a live adapter on the **same** instance.
pub(crate) fn find_live_adapter<'a>(
    live: &'a [wgpu::Adapter],
    selected: &AdapterCapabilities,
) -> Option<&'a wgpu::Adapter> {
    live.iter().find(|adapter| {
        let info = adapter.get_info();
        info.name == selected.identity.name
            && map_backend(info.backend) == selected.identity.backend
            && info.vendor == selected.identity.vendor
            && info.device == selected.identity.device
    })
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
