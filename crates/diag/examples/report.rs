//! `cargo run -p lcc-diag --example report -- [diag dir]`: print the report of every process that left files there.
//! Without an argument: `<PREFIX>_DIAG_DIR`, else `diag/` in the default settings folder (portable installs and
//! `<PREFIX>_CONFIG_DIR` need the folder given).
use std::io::Write as _;

fn main() -> std::process::ExitCode {
    let dir = std::env::args_os()
        .nth(1)
        .or_else(|| lcc_brand::env_os("DIAG_DIR").filter(|d| !d.is_empty()))
        .map(std::path::PathBuf::from)
        .or_else(|| lcc_brand::settings_dir().map(|d| d.join("diag")));
    let Some(dir) = dir else {
        eprintln!("usage: report <diag dir>");
        return std::process::ExitCode::FAILURE;
    };
    // a closed stdout (`report | head`) ends the report quietly instead of panicking
    let _ = std::io::stdout().write_all(lcc_diag::report(&dir).as_bytes());
    std::process::ExitCode::SUCCESS
}
