//! Headless GPU adapter diagnose entry for P2-F01.
//!
//! Not a shared-session CLI command. Prints adapter table + auto-selection.

use spirula_core::{EXIT_FAILURE, EXIT_SUCCESS, EXIT_USAGE};
use spirula_gpu::{
    CapabilityRequest, FieldAvailability, WGPU_VERSION, diagnose_exit_code,
    enumerate_host_adapters, select_auto,
};
use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return ExitCode::from(EXIT_SUCCESS);
    }
    if !args.is_empty() {
        eprintln!("error: unknown argument (usage: spirula-gpu-diagnose [--help])");
        return ExitCode::from(EXIT_USAGE);
    }

    match run() {
        Ok(()) => ExitCode::from(EXIT_SUCCESS),
        Err(code) => ExitCode::from(code),
    }
}

fn print_help() {
    println!(
        "spirula-gpu-diagnose — enumerate wgpu adapters and auto-select (P2-F01)\n\
         \n\
         Usage:\n\
           spirula-gpu-diagnose\n\
           spirula-gpu-diagnose --help\n\
         \n\
         Exit codes: 0 success, 1 no eligible adapter (SP-GPU-001), 2 usage.\n\
         wgpu pin: {WGPU_VERSION}\n\
         Does not create a device/queue (P2-F02) and is not a session CLI command."
    );
}

fn run() -> Result<(), u8> {
    println!("spirula-gpu-diagnose");
    println!("wgpu_version={WGPU_VERSION}");
    let adapters = enumerate_host_adapters().map_err(|e| diagnose_exit_code(&e))?;
    println!("adapter_count={}", adapters.len());
    if adapters.is_empty() {
        println!("note: no adapters enumerated on this host (not a measured GPU success)");
    }
    for a in &adapters {
        let vram = match &a.identity.dedicated_vram_bytes {
            FieldAvailability::Available(b) => format!("{b}"),
            FieldAvailability::Unavailable { reason } => format!("unavailable({reason})"),
        };
        let uuid = match &a.identity.uuid {
            FieldAvailability::Available(u) => u.clone(),
            FieldAvailability::Unavailable { reason } => format!("unavailable({reason})"),
        };
        println!(
            "adapter[{}] name={:?} backend={} class={} vendor={} device={} driver={:?} driver_info={:?} vram={} uuid={} \
             features={{subgroup={},f16={},timestamp={},f32_atomic={}}} \
             limits={{max_buffer_size={},max_storage_buffers_per_stage={},max_compute_workgroup_storage={}}}",
            a.identity.index,
            a.identity.name,
            a.identity.backend.as_str(),
            a.identity.class.as_str(),
            a.identity.vendor,
            a.identity.device,
            a.identity.driver,
            a.identity.driver_info,
            vram,
            uuid,
            a.features.subgroup,
            a.features.shader_f16,
            a.features.timestamp_query,
            a.features.float32_atomic_add,
            a.limits.max_buffer_size,
            a.limits.max_storage_buffers_per_shader_stage,
            a.limits.max_compute_workgroup_storage_size,
        );
    }

    let request = CapabilityRequest::diagnose_default();
    match select_auto(&adapters, &request) {
        Ok(sel) => {
            println!(
                "selected index={} name={:?} backend={} class={}",
                sel.adapter.identity.index,
                sel.adapter.identity.name,
                sel.adapter.identity.backend.as_str(),
                sel.adapter.identity.class.as_str(),
            );
            println!(
                "negotiated subgroup={:?} shader_f16={:?} timestamp={:?} float_atomic={:?}",
                sel.negotiated.subgroup,
                sel.negotiated.shader_f16,
                sel.negotiated.timestamp,
                sel.negotiated.float_atomic,
            );
            Ok(())
        }
        Err(err) => {
            let line =
                if env::var_os("SPIRULA_ERROR_CODES").is_some_and(|v| v == "1" || v == "true") {
                    err.cli_stderr_line(true)
                } else {
                    err.cli_stderr_line(false)
                };
            eprintln!("{line}");
            Err(diagnose_exit_code(&err).max(EXIT_FAILURE))
        }
    }
}
