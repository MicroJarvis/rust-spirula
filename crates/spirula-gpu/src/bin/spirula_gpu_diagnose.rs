//! Headless GPU diagnose: enumerate (P2-F01), device (P2-F02), resources (P2-F03),
//! compute (P2-F04).

use spirula_core::{EXIT_FAILURE, EXIT_SUCCESS, EXIT_USAGE};
use spirula_gpu::{
    AffineU32Params, BufferDesc, BufferUsage, CapabilityRequest, FieldAvailability, GpuContext,
    OpenFailureStage, ResourceLabel, Texture2dDesc, TextureFormat, TextureUsage, WGPU_VERSION,
    cpu_affine_u32, diagnose_device_open, diagnose_exit_code, enumerate_host_adapters,
    format_device_report, select_auto,
};
use std::env;
use std::process::ExitCode;
use std::time::{Duration, Instant};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return ExitCode::from(EXIT_SUCCESS);
    }

    let device = args.iter().any(|a| a == "--device");
    let resources = args.iter().any(|a| a == "--resources");
    let compute = args.iter().any(|a| a == "--compute");
    let unknown: Vec<&String> = args
        .iter()
        .filter(|a| !matches!(a.as_str(), "--device" | "--resources" | "--compute"))
        .collect();
    let mode_count = usize::from(device) + usize::from(resources) + usize::from(compute);
    if !unknown.is_empty() || mode_count > 1 {
        eprintln!(
            "error: usage: spirula-gpu-diagnose [--device | --resources | --compute] [--help]"
        );
        return ExitCode::from(EXIT_USAGE);
    }

    let result = if compute {
        run_compute()
    } else if resources {
        run_resources()
    } else if device {
        run_device()
    } else {
        run_enumerate()
    };
    match result {
        Ok(()) => ExitCode::from(EXIT_SUCCESS),
        Err(code) => ExitCode::from(code),
    }
}

fn print_help() {
    println!(
        "spirula-gpu-diagnose — enumerate / device / resource / compute (P2-F01–F04)\n\
         \n\
         Usage:\n\
           spirula-gpu-diagnose\n\
           spirula-gpu-diagnose --device\n\
           spirula-gpu-diagnose --resources\n\
           spirula-gpu-diagnose --compute\n\
           spirula-gpu-diagnose --help\n\
         \n\
         Exit codes: 0 success, 1 adapter/device unavailable, 2 usage.\n\
         wgpu pin: {WGPU_VERSION}\n\
         Not a session CLI command. Does not claim application/train/UI readiness."
    );
}

fn run_enumerate() -> Result<(), u8> {
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

fn run_device() -> Result<(), u8> {
    println!("spirula-gpu-diagnose --device");
    println!("wgpu_version={WGPU_VERSION}");
    let request = CapabilityRequest::diagnose_default();
    match diagnose_device_open(&request) {
        Ok(report) => {
            print!("{}", format_device_report(&report));
            println!("note: device open/close succeeded; not a compute/train/UI readiness claim");
            Ok(())
        }
        Err(failure) => print_open_failure(failure),
    }
}

fn run_resources() -> Result<(), u8> {
    println!("spirula-gpu-diagnose --resources");
    println!("wgpu_version={WGPU_VERSION}");
    let request = CapabilityRequest::diagnose_default();
    let mut ctx = match GpuContext::open(&request) {
        Ok(c) => c,
        Err(failure) => return print_open_failure(failure),
    };
    let backend = ctx
        .report()
        .map(|r| r.backend.as_str().to_owned())
        .unwrap_or_else(|| "unknown".into());
    let name = ctx
        .report()
        .map(|r| r.adapter.identity.name.clone())
        .unwrap_or_default();
    println!("device name={name:?} backend={backend}");

    let t_upload = Instant::now();
    let buf = ctx
        .create_buffer(BufferDesc {
            label: ResourceLabel::parse("spirula:gpu:buffer:diagnose").unwrap(),
            size: 32,
            usage: BufferUsage::COPY_SRC_DST,
        })
        .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;
    let pattern: Vec<u8> = (0..32).collect();
    ctx.upload_buffer(&buf, 0, &pattern)
        .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;
    let sub = ctx
        .submit()
        .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;
    ctx.wait(sub, Some(Duration::from_secs(5)))
        .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;
    let upload_ns = t_upload.elapsed().as_nanos();

    let t_rb = Instant::now();
    let ticket = ctx
        .readback_buffer(&buf, 0, 32)
        .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;
    let sub2 = ctx
        .submit()
        .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;
    ctx.wait(sub2, Some(Duration::from_secs(5)))
        .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;
    let out = ctx
        .map_readback(&ticket)
        .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;
    let readback_ns = t_rb.elapsed().as_nanos();
    if out != pattern {
        eprintln!("error: buffer round-trip mismatch");
        return Err(EXIT_FAILURE);
    }

    let tex = ctx
        .create_texture_2d(Texture2dDesc {
            label: ResourceLabel::parse("spirula:gpu:texture:diagnose").unwrap(),
            width: 3,
            height: 1,
            format: TextureFormat::Rgba8Unorm,
            usage: TextureUsage::COPY_SRC_DST,
        })
        .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;
    let pixels: Vec<u8> = (0..12).collect();
    ctx.upload_texture_rgba8(&tex, &pixels)
        .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;
    let s = ctx
        .submit()
        .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;
    ctx.wait(s, Some(Duration::from_secs(5)))
        .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;
    let ticket = ctx
        .readback_texture_rgba8(&tex)
        .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;
    let s2 = ctx
        .submit()
        .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;
    ctx.wait(s2, Some(Duration::from_secs(5)))
        .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;
    let out = ctx
        .map_readback(&ticket)
        .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;
    if out != pixels {
        eprintln!("error: texture round-trip mismatch (width=3 non-256 row)");
        return Err(EXIT_FAILURE);
    }

    let acc = ctx.resource_accounting();
    println!(
        "accounted source={} allocated={} current={} peak={} budget={:?}",
        acc.source, acc.allocated_bytes, acc.current_bytes, acc.peak_bytes, acc.budget_limit
    );
    println!("phase upload_wait_ns={upload_ns} readback_map_ns={readback_ns}");
    drop(buf);
    drop(tex);
    println!(
        "after_drop current={}",
        ctx.resource_accounting().current_bytes
    );
    ctx.close();
    println!("note: resource round-trip ok; not compute/train/UI readiness");
    Ok(())
}

fn run_compute() -> Result<(), u8> {
    println!("spirula-gpu-diagnose --compute");
    println!("wgpu_version={WGPU_VERSION}");
    let request = CapabilityRequest::diagnose_default();
    let mut ctx = match GpuContext::open(&request) {
        Ok(c) => c,
        Err(failure) => return print_open_failure(failure),
    };
    let report = ctx.report().cloned();
    let backend = report
        .as_ref()
        .map(|r| r.backend.as_str().to_owned())
        .unwrap_or_else(|| "unknown".into());
    let name = report
        .as_ref()
        .map(|r| r.adapter.identity.name.clone())
        .unwrap_or_default();
    let class = report
        .as_ref()
        .map(|r| r.adapter.identity.class.as_str().to_owned())
        .unwrap_or_else(|| "unknown".into());
    let limits = report.as_ref().map(|r| &r.device_limits);
    println!("device name={name:?} backend={backend} class={class}");
    if let Some(lim) = limits {
        println!(
            "device_limits max_buffer_size={} max_storage_buffers_per_shader_stage={} \
             max_compute_workgroup_storage_size={} max_compute_workgroups_per_dimension={} \
             max_texture_dimension_2d={}",
            lim.max_buffer_size,
            lim.max_storage_buffers_per_shader_stage,
            lim.max_compute_workgroup_storage_size,
            lim.max_compute_workgroups_per_dimension,
            lim.max_texture_dimension_2d
        );
    }

    let count = 100u32;
    let input: Vec<u32> = (0..count)
        .map(|i| i.wrapping_mul(3).wrapping_add(1))
        .collect();
    let scale = 7u32;
    let bias = 11u32;
    let mode = 2u32;

    let params = ctx
        .create_buffer(BufferDesc {
            label: ResourceLabel::parse("spirula:gpu:buffer:compute-params").unwrap(),
            size: 16,
            usage: BufferUsage::UNIFORM_UPLOAD,
        })
        .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;
    let in_buf = ctx
        .create_buffer(BufferDesc {
            label: ResourceLabel::parse("spirula:gpu:buffer:compute-in").unwrap(),
            size: u64::from(count) * 4,
            usage: BufferUsage::STORAGE_COPY,
        })
        .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;
    let out_buf = ctx
        .create_buffer(BufferDesc {
            label: ResourceLabel::parse("spirula:gpu:buffer:compute-out").unwrap(),
            size: u64::from(count) * 4,
            usage: BufferUsage::STORAGE_COPY,
        })
        .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;

    let mut in_bytes = Vec::with_capacity(input.len() * 4);
    for w in &input {
        in_bytes.extend_from_slice(&w.to_le_bytes());
    }
    ctx.upload_buffer(
        &params,
        0,
        &AffineU32Params::new(scale, bias, count).as_bytes(),
    )
    .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;
    ctx.upload_buffer(&in_buf, 0, &in_bytes)
        .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;
    let s = ctx
        .submit()
        .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;
    ctx.wait(s, Some(Duration::from_secs(5)))
        .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;

    let t_cold = Instant::now();
    let pipe = ctx
        .obtain_affine_u32_pipeline(mode)
        .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;
    let cold_ns = t_cold.elapsed().as_nanos();
    let t_warm = Instant::now();
    let pipe2 = ctx
        .obtain_affine_u32_pipeline(mode)
        .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;
    let warm_ns = t_warm.elapsed().as_nanos();
    if pipe.id() != pipe2.id() {
        eprintln!("error: warm pipeline obtain missed cache");
        return Err(EXIT_FAILURE);
    }
    let cache = ctx.pipeline_cache_stats();
    println!(
        "pipeline_cache hits={} misses={} entries={}",
        cache.hits, cache.misses, cache.entries
    );

    let t_compute = Instant::now();
    ctx.dispatch_affine_u32(&pipe, &params, &in_buf, &out_buf, count)
        .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;
    let s = ctx
        .submit()
        .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;
    ctx.wait(s, Some(Duration::from_secs(5)))
        .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;
    let ticket = ctx
        .readback_buffer(&out_buf, 0, u64::from(count) * 4)
        .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;
    let s = ctx
        .submit()
        .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;
    ctx.wait(s, Some(Duration::from_secs(5)))
        .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;
    let out = ctx
        .map_readback(&ticket)
        .map_err(|e| diagnose_exit_code(&e).max(EXIT_FAILURE))?;
    let compute_ns = t_compute.elapsed().as_nanos();
    let got: Vec<u32> = out
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect();
    let expect = cpu_affine_u32(&input, scale, bias, mode);
    if got != expect {
        eprintln!("error: affine compute mismatch vs CPU reference");
        return Err(EXIT_FAILURE);
    }

    let acc = ctx.resource_accounting();
    println!(
        "accounted source={} allocated={} current={} peak={} budget={:?}",
        acc.source, acc.allocated_bytes, acc.current_bytes, acc.peak_bytes, acc.budget_limit
    );
    println!(
        "phase cold_pipeline_ns={cold_ns} warm_pipeline_ns={warm_ns} compute_dispatch_readback_ns={compute_ns}"
    );
    println!(
        "proof count={count} (non-workgroup-multiple) mode={mode} scale={scale} bias={bias} match=cpu_exact"
    );
    drop(params);
    drop(in_buf);
    drop(out_buf);
    println!(
        "after_drop current={}",
        ctx.resource_accounting().current_bytes
    );
    ctx.close();
    println!(
        "note: affine compute round-trip ok; not application/train/UI readiness; \
         full GPU events=P2-F05 surface smoke=P2-F06 cache perf opt=P9-F03"
    );
    Ok(())
}

fn print_open_failure(failure: spirula_gpu::OpenError) -> Result<(), u8> {
    let line = if env::var_os("SPIRULA_ERROR_CODES").is_some_and(|v| v == "1" || v == "true") {
        failure.error.cli_stderr_line(true)
    } else {
        failure.error.cli_stderr_line(false)
    };
    eprintln!("{line}");
    eprintln!(
        "open_failure_stage={:?}{}",
        failure.stage,
        match failure.stage {
            OpenFailureStage::EnumerateEmpty => " (no adapter — not a false pass)",
            _ => "",
        }
    );
    Err(diagnose_exit_code(&failure.error).max(EXIT_FAILURE))
}
