//! `cargo run -p lcc-diag --example report -- <diag dir>`: print the report of every process that left files there.
fn main() {
    let dir = std::env::args_os().nth(1).map(std::path::PathBuf::from).or_else(|| lcc_brand::settings_dir().map(|d| d.join("diag")));
    match dir {
        Some(d) => print!("{}", lcc_diag::report(&d)),
        None => eprintln!("usage: report <diag dir>"),
    }
}
