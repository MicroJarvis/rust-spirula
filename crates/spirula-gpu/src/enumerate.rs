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

/// Resolve the snapshot-local slot for a selection made against `reports` from
/// the **same** [`enumerate_on_instance`] call.
///
/// Uses `selected.identity.index` as the pairing key into that snapshot only.
/// Name/backend/vendor/device are **not** unique (two identical cards can share
/// them); matching by those fields can rebind the second selected adapter to
/// the first. External or previous-snapshot indices are rejected because the
/// slot must still equal `selected` (including features/limits).
pub(crate) fn resolve_snapshot_slot(
    reports: &[AdapterCapabilities],
    selected: &AdapterCapabilities,
) -> Option<usize> {
    let index = selected.identity.index;
    let report = reports.get(index)?;
    if !same_snapshot_entry(report, selected) {
        return None;
    }
    Some(index)
}

fn same_snapshot_entry(report: &AdapterCapabilities, selected: &AdapterCapabilities) -> bool {
    report.identity.index == selected.identity.index
        && report.identity.name == selected.identity.name
        && report.identity.backend == selected.identity.backend
        && report.identity.vendor == selected.identity.vendor
        && report.identity.device == selected.identity.device
        && report.features == selected.features
        && report.limits == selected.limits
}

/// Live adapter paired with `selected` in the same enumerate snapshot.
pub(crate) fn paired_live_adapter<'a>(
    reports: &[AdapterCapabilities],
    live: &'a [wgpu::Adapter],
    selected: &AdapterCapabilities,
) -> Option<&'a wgpu::Adapter> {
    if reports.len() != live.len() {
        return None;
    }
    let index = resolve_snapshot_slot(reports, selected)?;
    live.get(index)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{
        AdapterClass, CapabilityId, CapabilityRequest, FeatureBits, LimitFloor, select_auto,
    };

    fn twin(index: usize, features: FeatureBits, limits: AdapterLimits) -> AdapterCapabilities {
        AdapterCapabilities {
            identity: AdapterIdentity {
                index,
                name: "Twin GPU".into(),
                backend: BackendKind::Vulkan,
                class: AdapterClass::DiscreteGpu,
                vendor: 0x1002,
                device: 0x73bf,
                driver: "test".into(),
                driver_info: String::new(),
                dedicated_vram_bytes: FieldAvailability::Unavailable {
                    reason: "synthetic",
                },
                uuid: FieldAvailability::Unavailable {
                    reason: "synthetic",
                },
            },
            features,
            limits,
        }
    }

    fn limits() -> AdapterLimits {
        AdapterLimits {
            max_buffer_size: 1 << 30,
            max_storage_buffers_per_shader_stage: 8,
            max_compute_workgroup_storage_size: 16_384,
        }
    }

    #[test]
    fn snapshot_index_pairs_second_twin_not_first_by_name() {
        // Identical name/backend/vendor/device; only index 1 meets required subgroup.
        let weak = FeatureBits {
            subgroup: false,
            shader_f16: false,
            timestamp_query: false,
            float32_atomic_add: false,
        };
        let strong = FeatureBits {
            subgroup: true,
            shader_f16: false,
            timestamp_query: false,
            float32_atomic_add: false,
        };
        let reports = vec![twin(0, weak, limits()), twin(1, strong, limits())];
        let mut request = CapabilityRequest::diagnose_default();
        request.required_features = vec![CapabilityId::Subgroup];
        request.optional_features.clear();
        request.required_min_limits = LimitFloor::default();

        let selected = select_auto(&reports, &request).expect("second twin eligible");
        assert_eq!(selected.adapter.identity.index, 1);
        assert_eq!(
            resolve_snapshot_slot(&reports, &selected.adapter),
            Some(1),
            "must pair by snapshot index, not first name match"
        );
    }

    #[test]
    fn external_or_stale_index_rejected_even_with_matching_name_fields() {
        let features = FeatureBits {
            subgroup: true,
            shader_f16: false,
            timestamp_query: false,
            float32_atomic_add: false,
        };
        let reports = vec![twin(0, features, limits()), twin(1, features, limits())];
        // Forged selection: claims index 0 but carries index-1 features would still
        // match slot 0 features here; instead claim index 9 (outside snapshot).
        let mut forged = twin(9, features, limits());
        forged.identity.name = "Twin GPU".into();
        assert_eq!(resolve_snapshot_slot(&reports, &forged), None);

        // Wrong slot: index points at 0 but payload equals report[1] (features same);
        // change features so same_snapshot_entry fails when index is lied about.
        let mut mismatched = reports[1].clone();
        mismatched.identity.index = 0; // pretend to be slot 0 while keeping slot-1 identity payload
        // name/vendor/device still match slot 0, but index field on selected is 0 while
        // we compare to reports[0] — features equal so this would pass features check.
        // Make features differ from reports[0] to prove we do not accept a rebinding.
        mismatched.features.shader_f16 = true;
        assert_ne!(mismatched.features, reports[0].features);
        assert_eq!(
            resolve_snapshot_slot(&reports, &mismatched),
            None,
            "index 0 slot must equal selected payload; no silent rebind"
        );
    }
}
