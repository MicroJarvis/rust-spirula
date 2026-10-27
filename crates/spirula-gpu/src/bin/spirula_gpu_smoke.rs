//! P2-F06 headless GPU smoke entry (compute + honest surface gate).

use spirula_core::{EXIT_SUCCESS, EXIT_USAGE};
use spirula_gpu::{
    CapabilityRequest, WGPU_VERSION, format_smoke_report, run_compute_smoke_repeat,
    run_surface_smoke, smoke_exit_code,
};
use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return ExitCode::from(EXIT_SUCCESS);
    }

    let compute = args.iter().any(|a| a == "--compute");
    let surface = args.iter().any(|a| a == "--surface");
    let all = args.iter().any(|a| a == "--all");
    let repeat = match parse_repeat(&args) {
        Ok(n) => n.unwrap_or(1),
        Err(()) => {
            eprintln!(
                "error: usage: spirula-gpu-smoke [--compute | --surface | --all] [--repeat N] [--help]"
            );
            return ExitCode::from(EXIT_USAGE);
        }
    };

    if args_have_unknown(&args) {
        eprintln!(
            "error: usage: spirula-gpu-smoke [--compute | --surface | --all] [--repeat N] [--help]"
        );
        return ExitCode::from(EXIT_USAGE);
    }

    let run_compute = all || compute || (!surface && !all && !compute);
    let run_surface = all || surface;

    println!("spirula-gpu-smoke");
    println!("wgpu_version={WGPU_VERSION}");
    let request = CapabilityRequest::diagnose_default();
    let mut worst = EXIT_SUCCESS;

    if run_compute {
        for i in 0..repeat {
            let report = run_compute_smoke_repeat(&request, i);
            print!("{}", format_smoke_report(&report));
            worst = worst.max(smoke_exit_code(&report));
        }
    }
    if run_surface {
        let report = run_surface_smoke(&request);
        print!("{}", format_smoke_report(&report));
        worst = worst.max(smoke_exit_code(&report));
    }

    ExitCode::from(worst)
}

fn parse_repeat(args: &[String]) -> Result<Option<u32>, ()> {
    let mut found = None;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if let Some(v) = a.strip_prefix("--repeat=") {
            let n = v.parse().ok().filter(|n| *n >= 1).ok_or(())?;
            found = Some(n);
            i += 1;
            continue;
        }
        if a == "--repeat" {
            let n = args
                .get(i + 1)
                .and_then(|v| v.parse().ok())
                .filter(|n| *n >= 1)
                .ok_or(())?;
            found = Some(n);
            i += 2;
            continue;
        }
        i += 1;
    }
    Ok(found)
}

fn args_have_unknown(args: &[String]) -> bool {
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if matches!(a, "--compute" | "--surface" | "--all" | "--help" | "-h")
            || a.starts_with("--repeat=")
        {
            i += 1;
            continue;
        }
        if a == "--repeat" {
            i += 2; // value consumed even if invalid (parse_repeat already checked)
            continue;
        }
        return true;
    }
    false
}

fn print_help() {
    println!(
        "spirula-gpu-smoke — headless compute / honest surface smoke (P2-F06)\n\
         \n\
         Usage:\n\
           spirula-gpu-smoke\n\
           spirula-gpu-smoke --compute [--repeat N]\n\
           spirula-gpu-smoke --surface\n\
           spirula-gpu-smoke --all\n\
           spirula-gpu-smoke --help\n\
         \n\
         Default: --compute once.\n\
         Exit: 0 = pass|skip|unsupported; 1 = fail; 2 = usage.\n\
         Surface: set SPIRULA_GPU_SURFACE=1 only with a safe present target;\n\
         this binary will not forge a window.\n\
         wgpu pin: {WGPU_VERSION}\n\
         Not a session CLI. Not application/train/UI readiness. Not P2-F07 packaging."
    );
}
