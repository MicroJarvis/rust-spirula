//! `spirula` process entry. Thin wrapper over `spirula_cli`.

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    spirula_cli::main_from_args(args)
}
