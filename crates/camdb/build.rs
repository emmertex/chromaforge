//! Embeds every `data/cameras/*.toml` (one file per maker) as `(file name, text)` pairs in `$OUT_DIR/cameras.rs`,
//! and every `data/calibrations/*.json` (one file per camera model) in `$OUT_DIR/calibrations.rs`.
use std::fmt::Write as _;

fn embed(dir: &str, extension: &str, target: &str) {
    let dir = std::path::Path::new(dir);
    // Never watch a path that doesn't exist: cargo then counts the script as stale on every build, and this
    // crate and everything above it (engine, UI, MCP, CLI) recompiled on every cargo run.
    if dir.is_dir() {
        println!("cargo:rerun-if-changed={}", dir.display());
    } else {
        println!("cargo:rerun-if-changed=build.rs");
    }
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == extension)).collect())
        .unwrap_or_default();
    files.sort();
    let mut out = String::from("&[\n");
    for f in &files {
        println!("cargo:rerun-if-changed={}", f.display());
        let name = f.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        let abs = std::fs::canonicalize(f).unwrap_or_else(|_| f.clone());
        let _ = writeln!(out, "    ({name:?}, include_str!({:?})),", abs.display().to_string());
    }
    out.push(']');
    let target = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap_or_default()).join(target);
    if let Err(e) = std::fs::write(&target, out) {
        println!("cargo:warning=camdb: could not write {}: {e}", target.display());
    }
}

fn main() {
    embed("data/cameras", "toml", "cameras.rs");
    embed("data/calibrations", "json", "calibrations.rs");
}
