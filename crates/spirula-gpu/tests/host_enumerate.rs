//! Host headless enumerate — always runs; empty is not a measured GPU success.

use spirula_gpu::{
    CapabilityRequest, FieldAvailability, WGPU_VERSION, enumerate_host_adapters, select_auto,
};

#[test]
fn host_enumerate_records_adapters_or_precise_empty() {
    let adapters = enumerate_host_adapters().expect("enumerate must not panic");
    eprintln!("wgpu_version={WGPU_VERSION}");
    eprintln!("host_adapter_count={}", adapters.len());
    for a in &adapters {
        assert!(
            !a.identity.name.trim().is_empty(),
            "adapter name must be non-empty"
        );
        eprintln!(
            "host_adapter index={} name={:?} backend={} class={} driver={:?}",
            a.identity.index,
            a.identity.name,
            a.identity.backend.as_str(),
            a.identity.class.as_str(),
            a.identity.driver,
        );
        assert!(
            matches!(
                a.identity.dedicated_vram_bytes,
                FieldAvailability::Unavailable { .. }
            ),
            "VRAM must stay honestly unavailable on wgpu AdapterInfo"
        );
        assert!(
            matches!(a.identity.uuid, FieldAvailability::Unavailable { .. }),
            "UUID must stay honestly unavailable"
        );
    }

    match select_auto(&adapters, &CapabilityRequest::diagnose_default()) {
        Ok(sel) => {
            eprintln!(
                "host_selected name={:?} backend={}",
                sel.adapter.identity.name,
                sel.adapter.identity.backend.as_str()
            );
        }
        Err(err) => {
            assert_eq!(err.code().as_str(), "SP-GPU-001");
            eprintln!(
                "host_select_unavailable code={} message={}",
                err.code().as_str(),
                err.message()
            );
            assert!(
                adapters.is_empty() || err.message().contains("required"),
                "non-empty adapters failing diagnose_default is unexpected: {}",
                err.message()
            );
        }
    }
}
