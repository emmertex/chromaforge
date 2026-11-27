//! Builds the C shim against LibRaw, when there is one:
//!
//! - `LIBRAW_DIR=<LibRaw source checkout>` (<https://github.com/LibRaw/LibRaw>): LibRaw's sources are compiled and
//!   linked in statically (any desktop platform with a C++ compiler);
//! - `LIBRAW_SYSTEM=1`: the system's LibRaw (`libraw_r`, e.g. `libraw-dev` or Homebrew `libraw`) is linked; its
//!   headers are looked for in `LIBRAW_INCLUDE_DIR` (default: the compiler's own paths), the library in
//!   `LIBRAW_LIB_DIR`.
//!
//! With neither the crate builds as a stub (`cfg(lcc_libraw)` unset) that answers "not available". `LIBRAW_REQUIRED=1`
//! (release builds that promise LibRaw) makes that an error instead.

use std::path::{Path, PathBuf};

fn main() -> Result<(), String> {
    for v in ["LIBRAW_DIR", "LIBRAW_SYSTEM", "LIBRAW_INCLUDE_DIR", "LIBRAW_LIB_DIR", "LIBRAW_REQUIRED"] {
        println!("cargo:rerun-if-env-changed={v}");
    }
    println!("cargo:rerun-if-changed=shim/shim.cpp");
    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let unsupported_target = target_arch == "wasm32" || target_os == "android" || target_os == "ios";
    let dir = std::env::var_os("LIBRAW_DIR").map(PathBuf::from).filter(|d| !d.as_os_str().is_empty());
    let system = std::env::var("LIBRAW_SYSTEM").is_ok_and(|v| v == "1");
    let required = std::env::var("LIBRAW_REQUIRED").is_ok_and(|v| v == "1");
    if unsupported_target || (dir.is_none() && !system) {
        if required {
            return Err(format!(
                "LIBRAW_REQUIRED=1 but {}",
                if unsupported_target { "LibRaw is not built for this target" } else { "neither LIBRAW_DIR nor LIBRAW_SYSTEM=1 is set" }
            ));
        }
        return Ok(());
    }
    let mut build = cc::Build::new();
    build.cpp(true).file("shim/shim.cpp").warnings(false);
    if target_os == "windows" {
        build.define("LIBRAW_NODLL", None).define("_CRT_SECURE_NO_WARNINGS", None);
    }
    match dir {
        Some(dir) => {
            if !dir.join("libraw/libraw.h").is_file() {
                return Err(format!("LIBRAW_DIR={} is not a LibRaw source checkout (no libraw/libraw.h)", dir.display()));
            }
            build.include(&dir);
            let mut sources = Vec::new();
            collect_cpp(&dir.join("src"), &mut sources).map_err(|e| format!("reading {}: {e}", dir.join("src").display()))?;
            sources.sort();
            if sources.is_empty() {
                return Err(format!("no LibRaw sources under {}", dir.join("src").display()));
            }
            build.files(&sources);
            println!("cargo:rerun-if-changed={}", dir.join("src").display());
        }
        None => {
            if let Some(inc) = std::env::var_os("LIBRAW_INCLUDE_DIR") {
                build.include(inc);
            }
            if let Some(lib) = std::env::var_os("LIBRAW_LIB_DIR") {
                println!("cargo:rustc-link-search=native={}", Path::new(&lib).display());
            }
            println!("cargo:rustc-link-lib=raw_r");
        }
    }
    build.try_compile("lcc_libraw_shim").map_err(|e| e.to_string())?;
    println!("cargo:rustc-cfg=lcc_libraw");
    Ok(())
}

/// Every `.cpp` under `dir`, recursively, except the `*_ph.cpp` placeholders LibRaw's own makefiles leave out.
fn collect_cpp(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_cpp(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "cpp") && !path.file_stem().is_some_and(|s| s.to_string_lossy().ends_with("_ph")) {
            out.push(path);
        }
    }
    Ok(())
}
