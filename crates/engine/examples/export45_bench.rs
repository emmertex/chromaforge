//! The 45 MP export benchmark of issue #1377 (`cargo xtask bench`'s `export45` probe): a batch of 45 MP raws exported
//! as the Export dialog runs them (JPEG quality 90, sRGB, photos side by side), at a 2048 px long edge and at full
//! size, on the CPU and (when there is an adapter) the GPU. For each it prints, per photo:
//!
//! - wall time and process CPU time,
//! - peak memory (the process's peak resident set; macOS and Linux),
//! - GPU busy: the share of the wall time the GPU had work in flight (`lcc_gpu::busy`, an upper bound on the time
//!   the device itself worked),
//! - per-stage time (`lcc_pipeline::meter`): each stage's time summed over every lane, per photo, so the stages of
//!   photos side by side add up (stage-seconds, not wall time). `export decode` is the whole source load (file read,
//!   raw decode, both repair passes, starting colour, binning or demosaic, colour finish), the `raw …` rows split it.
//!
//! `cargo run --release -p lcc-engine --example export45_bench -- [--photos N] [--lanes N] [--sizes 2048,full]
//! [--cpu-only] [--dir DIR | FILES…]`
//!
//! Without files it writes `N` (default 6) synthetic 45 MP (8256 × 5504) Bayer DNGs, lossless-JPEG compressed, with
//! fine detail, noise and some hot photosites, to `target/bench/export45/` (kept for the next run). Those decode far
//! faster than a Nikon HE\* NEF (no JPEG XS): pass real raws (`--dir` or files) to measure a camera's decode.
//! Each scenario runs in a child process of its own, so peak memory is that scenario's; one photo is exported first,
//! untimed, to start the GPU and compile its kernels.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::time::Instant;

use lcc_engine::catalog::PhotoId;
use lcc_engine::export::{Destination, ExportOptions, Resize, export_batch};

/// Nikon Z 9 sensor size (45.7 MP).
const W: usize = 8256;
const H: usize = 5504;

/// Process CPU time (user + system) and peak resident set in bytes, from `getrusage`. The only `unsafe` is this
/// libc read, in a dev-only example (as `render_bench`'s clock).
#[allow(unsafe_code)]
fn usage() -> (f64, u64) {
    #[cfg(unix)]
    {
        // SAFETY: an all-zero `rusage` is a valid value, and `getrusage` only writes into the struct we pass.
        let mut u: libc::rusage = unsafe { std::mem::zeroed() };
        // SAFETY: as above; RUSAGE_SELF is always a valid target.
        unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) };
        let ms = |t: libc::timeval| t.tv_sec as f64 * 1e3 + t.tv_usec as f64 / 1e3;
        // ru_maxrss: kilobytes on Linux, bytes on macOS
        let peak = if cfg!(target_os = "macos") { u.ru_maxrss as u64 } else { (u.ru_maxrss as u64).saturating_mul(1024) };
        (ms(u.ru_utime) + ms(u.ru_stime), peak)
    }
    #[cfg(not(unix))]
    {
        (0.0, 0)
    }
}

/// A deterministic hash of a photosite: the synthetic scene's noise.
fn hash(x: u32, y: u32, seed: u32) -> u32 {
    let mut h = x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA77) ^ seed.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^ (h >> 15)
}

/// One synthetic 45 MP raw: smooth colour fields, fine sinusoidal detail and edges, shot-like noise and about one
/// hot photosite in 100,000, as an RGGB mosaic.
fn synthetic_dng(seed: u32) -> Vec<u8> {
    let (black, white) = (1008.0f32, 15520.0f32);
    let mut data = vec![0u16; W * H];
    for (y, row) in data.chunks_mut(W).enumerate() {
        let fy = y as f32 / H as f32;
        for (x, v) in row.iter_mut().enumerate() {
            let fx = x as f32 / W as f32;
            let colour = (y & 1) * 2 + (x & 1); // 0 R, 1 G, 2 G, 3 B
            let base = match colour {
                0 => 0.25 + 0.35 * fx,
                3 => 0.20 + 0.30 * fy,
                _ => 0.30 + 0.25 * (1.0 - fx * fy),
            };
            let detail = 0.08 * ((x as f32 * 0.37 + seed as f32).sin() * (y as f32 * 0.21).cos());
            let edge = if ((x / 397) + (y / 263) + seed as usize).is_multiple_of(3) { 0.15 } else { 0.0 };
            let h = hash(x as u32, y as u32, seed);
            let noise = ((h & 0xFFFF) as f32 / 65535.0 - 0.5) * 0.02;
            let level = (base + detail + edge + noise).clamp(0.0, 1.0);
            *v = if h >> 15 == 0x1_2345 { white as u16 } else { (black + level * (white - black)) as u16 };
        }
    }
    let raw = lcc_raw::RawImage {
        format: lcc_raw::RawFormat::Dng,
        width: W,
        height: H,
        cpp: 1,
        data: lcc_raw::RawData::U16(data),
        cfa: lcc_raw::Cfa::bayer("RGGB"),
        bits: 14,
        black: lcc_raw::BlackLevel::uniform(black),
        white: vec![white],
        active_area: lcc_raw::Rect::new(0, 0, W, H),
        crop: lcc_raw::Rect::new(0, 0, W, H),
        orientation: lcc_raw::Orientation::Normal,
        color: lcc_raw::ColorData {
            illuminant: [21, 0],
            color_matrix: [Some(lcc_raw::Mat3([[0.8, -0.2, -0.05], [-0.45, 1.3, 0.15], [-0.08, 0.22, 0.62]])), None],
            as_shot_neutral: Some([0.5, 1.0, 0.7]),
            ..Default::default()
        },
        wb_multipliers: None,
        linearized: false,
        pixel_aspect: 1.0,
        opcodes: Default::default(),
        metadata: lcc_raw::Metadata { make: Some("Synthetic".into()), model: Some("45 MP bench".into()), ..Default::default() },
        lens_metadata: Default::default(),
    };
    let opts = lcc_raw::DngWriteOptions { compression: lcc_raw::DngCompression::Lj92 { tile: 256 }, ..Default::default() };
    lcc_raw::write_dng(&raw, &opts).expect("synthetic DNG")
}

/// The synthetic photos, written once into `dir`.
fn synthetic_files(dir: &Path, n: usize) -> Vec<String> {
    std::fs::create_dir_all(dir).expect("bench dir");
    (0..n)
        .map(|i| {
            let p = dir.join(format!("SYNTH45_{i:03}.dng"));
            if !p.exists() {
                eprintln!("writing {}", p.display());
                let tmp = p.with_extension("tmp");
                std::fs::write(&tmp, synthetic_dng(i as u32)).expect("write DNG");
                std::fs::rename(&tmp, &p).expect("rename DNG");
            }
            p.to_string_lossy().into_owned()
        })
        .collect()
}

/// The raw files `--dir` picks up.
const RAW_EXTENSIONS: &[&str] =
    &["nef", "nrw", "cr2", "cr3", "crw", "arw", "srf", "sr2", "dng", "raf", "orf", "rw2", "pef", "srw", "3fr", "iiq", "erf", "x3f"];

fn raws_in(dir: &Path, n: usize) -> Vec<String> {
    let mut files: Vec<String> = std::fs::read_dir(dir)
        .expect("--dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            let ext = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            p.is_file() && RAW_EXTENSIONS.contains(&ext.as_str())
        })
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    files.sort();
    files.truncate(n);
    files
}

/// One scenario, in this (child) process: prints one `RESULT` line of JSON.
fn child(files: &[String], edge: Option<u32>, gpu: bool, lanes: Option<usize>) {
    // as the app and the CLI do: freed pages go back to the system after each decode
    lcc_engine::memory::set_release_hook(|| {
        let _ = lcc_sysmem::release_free_memory();
    });
    if let Some(n) = lanes {
        lcc_engine::export::set_export_lanes(Some(n));
    }
    lcc_engine::gpu::set_enabled(gpu);
    let adapter = if gpu { lcc_engine::gpu::adapter_name() } else { None };
    if gpu && adapter.is_none() {
        println!("RESULT {}", serde_json::json!({ "adapter": null }));
        return;
    }
    let mut session = lcc_engine::Session::new().with_fs();
    let r = session.execute("library.import", &serde_json::json!({ "paths": files })).expect("import");
    let ids: Vec<PhotoId> = r["imported"].as_array().expect("ids").iter().filter_map(|v| v.as_u64()).map(PhotoId).collect();
    assert!(!ids.is_empty(), "nothing imported from {files:?}");
    let o = ExportOptions { hdr: false, quality: 90, resize: edge.map(Resize::long_edge), ..ExportOptions::default() };
    let dest = Destination::default();
    // warm-up, untimed: the GPU's device and kernels
    export_batch(&mut session, &ids[..1], &o, &dest, &mut |_, _| Ok(()), &|_| false).expect("warm-up export");
    session.media.forget(ids[0]);
    let _ = lcc_engine::pipeline::meter::take();
    lcc_engine::pipeline::meter::set_on(true);
    if gpu {
        lcc_engine::gpu::busy::set_metering(true);
    }
    let (cpu0, _) = usage();
    let t = Instant::now();
    let out = export_batch(&mut session, &ids, &o, &dest, &mut |_, _| Ok(()), &|_| false).expect("export");
    let wall = t.elapsed().as_secs_f64() * 1e3;
    let (cpu1, peak) = usage();
    let busy = if gpu { lcc_engine::gpu::busy::busy_time().as_secs_f64() * 1e3 } else { 0.0 };
    lcc_engine::pipeline::meter::set_on(false);
    let errors: Vec<String> = out.iter().filter_map(|f| f.get("error").map(|e| e.to_string())).collect();
    let fell_back = out.iter().filter(|f| f.get("gpuFallback").is_some()).count();
    let n = ids.len() as f64;
    let stages: serde_json::Map<String, serde_json::Value> =
        lcc_engine::pipeline::meter::take().into_iter().map(|(k, s)| (k, serde_json::json!(s.total.as_secs_f64() * 1e3 / n))).collect();
    let result = serde_json::json!({
        "adapter": adapter,
        "photos": ids.len(),
        "lanes": lcc_engine::export::export_parallelism(ids.len()),
        "wall_ms": wall / n,
        "cpu_ms": (cpu1 - cpu0) / n,
        "peak_mb": peak as f64 / (1u64 << 20) as f64,
        "busy_pct": if wall > 0.0 { busy * 100.0 / wall } else { 0.0 },
        "stages": stages,
        "errors": errors,
        "fell_back": fell_back,
    });
    println!("RESULT {result}");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |f: &str| args.iter().any(|a| a == f);
    let value = |f: &str| args.iter().position(|a| a == f).and_then(|i| args.get(i + 1)).cloned();
    let with_value = ["--photos", "--lanes", "--sizes", "--dir", "--child-edge", "--child-gpu"];
    let files: Vec<String> = args
        .iter()
        .enumerate()
        .filter(|(i, a)| !a.starts_with("--") && !i.checked_sub(1).and_then(|j| args.get(j)).is_some_and(|p| with_value.contains(&p.as_str())))
        .map(|(_, a)| a.clone())
        .collect();
    let lanes = value("--lanes").and_then(|v| v.parse::<usize>().ok());
    if let Some(edge) = value("--child-edge") {
        let gpu = value("--child-gpu").as_deref() == Some("1");
        child(&files, edge.parse::<u32>().ok().filter(|e| *e > 0), gpu, lanes);
        return;
    }
    let photos = value("--photos").and_then(|v| v.parse::<usize>().ok()).unwrap_or(6).max(1);
    let synthetic = value("--dir").is_none() && files.is_empty();
    let files = if let Some(dir) = value("--dir") {
        raws_in(Path::new(&dir), photos)
    } else if !files.is_empty() {
        files
    } else {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        synthetic_files(&root.join("target/bench/export45"), photos)
    };
    if files.is_empty() {
        eprintln!("export45_bench: no raws to export");
        std::process::exit(2);
    }
    let sizes: Vec<(String, u32)> = value("--sizes")
        .unwrap_or_else(|| "2048,full".into())
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| (s.to_string(), if s == "full" { 0 } else { s.parse::<u32>().expect("--sizes: a long edge in px, or `full`") }))
        .collect();
    let exe = std::env::current_exe().expect("own executable");
    let what = if synthetic { "synthetic " } else { "" };
    println!("export45: {} {what}photos, JPEG quality 90, sRGB", files.len());
    for (name, edge) in &sizes {
        let label = if *edge == 0 { "export 45 MP full size".to_string() } else { format!("export 45 MP to {name} px") };
        let mut cols: Vec<(&str, serde_json::Value)> = Vec::new();
        for gpu in if flag("--cpu-only") { vec![false] } else { vec![false, true] } {
            let mut c = std::process::Command::new(&exe);
            c.args(["--child-edge", &edge.to_string(), "--child-gpu", if gpu { "1" } else { "0" }]).args(&files);
            if let Some(n) = lanes {
                c.args(["--lanes", &n.to_string()]);
            }
            let out = c.output().expect("run the scenario");
            if !out.status.success() {
                eprintln!("{label} ({}): failed\n{}", if gpu { "gpu" } else { "cpu" }, String::from_utf8_lossy(&out.stderr));
                std::process::exit(1);
            }
            let stdout = String::from_utf8_lossy(&out.stdout);
            let Some(r) = stdout.lines().find_map(|l| l.strip_prefix("RESULT ")).and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok())
            else {
                eprintln!("{label}: no result\n{stdout}");
                std::process::exit(1);
            };
            if r["adapter"].is_null() && gpu {
                eprintln!("{label}: no GPU adapter, CPU only");
                continue;
            }
            for e in r["errors"].as_array().into_iter().flatten() {
                eprintln!("  error: {e}");
            }
            if r["fell_back"].as_u64().unwrap_or(0) > 0 {
                eprintln!("  {} photos fell back to the CPU", r["fell_back"]);
            }
            cols.push((if gpu { "gpu" } else { "cpu" }, r));
        }
        if let Some((_, r)) = cols.first() {
            println!(
                "{label}: {} photos, {} lanes{}",
                r["photos"],
                r["lanes"],
                cols.iter().find_map(|(_, r)| r["adapter"].as_str().map(|a| format!(", GPU {a}"))).unwrap_or_default()
            );
        }
        let main_line: Vec<String> = cols
            .iter()
            .map(|(b, r)| {
                let f = |k: &str| r[k].as_f64().unwrap_or(0.0);
                let busy = if *b == "gpu" { format!(", {:.1} % busy", f("busy_pct")) } else { String::new() };
                format!("{b}: {:.1} ms wall, {:.1} ms cpu, {:.0} MB peak{busy}", f("wall_ms"), f("cpu_ms"), f("peak_mb"))
            })
            .collect();
        println!("{label}, per photo:  {}", main_line.join("  |  "));
        let mut names: Vec<String> = cols.iter().flat_map(|(_, r)| r["stages"].as_object().into_iter().flat_map(|m| m.keys().cloned())).collect();
        names.sort();
        names.dedup();
        for stage in names {
            let parts: Vec<String> =
                cols.iter().filter_map(|(b, r)| r["stages"].get(&stage).and_then(|v| v.as_f64()).map(|ms| format!("{b}: {ms:.1} ms wall"))).collect();
            println!("{label}, stage {stage}:  {}", parts.join("  |  "));
        }
    }
}
