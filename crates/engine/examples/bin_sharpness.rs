//! The visual check for #1377 step 2 (source size for small exports): how sharp, and how close to a full-size develop,
//! a 45 MP raw comes out at a given long edge when it is binned first instead of demosaiced whole, and what each way
//! costs. For each output size it compares, against the reference (an AHD demosaic of the whole sensor, Lanczos-3 to
//! the output):
//!
//! - `today`: what an export does now: binned to the preview level when it fits there (k from `bin_factor(2560)`,
//!   then a box fit to 2560 px), else the whole sensor demosaiced;
//! - `bin k`: binned by `k`, then one Lanczos-3 step straight to the output, for every `k` that leaves at least the
//!   output's size.
//!
//! It prints SSIM against the reference (sRGB 8-bit), the detail kept (the mean absolute Laplacian of the luma, as a
//! share of the reference's) and the source time (develop + fit). Scenes: a zone plate (fine detail at every
//! frequency, the worst case for a box filter) and three demo-library scenes, mosaicked to an RGGB 8256 × 5504 raw.
//! Writes a crop of each result as JPEG to `target/bench/bin-sharpness/` to look at side by side.
//!
//! `cargo run --release -p lcc-engine --example bin_sharpness -- [--sizes 2048,2560,3000,3840]`
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Instant;

use lcc_raster::Rgb32f;
use lcc_raster::resample::{Filter, fit, resize};

const W: usize = 8256;
const H: usize = 5504;

fn zone_plate() -> Rgb32f {
    let (cx, cy) = (W as f32 / 2.0, H as f32 / 2.0);
    let k = std::f32::consts::PI / (2.0 * W as f32); // frequency rises to ~Nyquist/2 at the corners
    Rgb32f::from_fn(W, H, |x, y| {
        let (dx, dy) = (x as f32 - cx, y as f32 - cy);
        let v = 0.5 + 0.4 * (k * (dx * dx + dy * dy)).cos();
        [v * 0.9, v, v * 0.8]
    })
}

/// An RGGB 14-bit raw of linear camera RGB `img` (`W × H`).
fn raw_of(img: &Rgb32f) -> lcc_raw::RawImage {
    let (black, white) = (1008.0f32, 15520.0f32);
    let data: Vec<u16> = (0..W * H)
        .map(|i| {
            let (x, y) = (i % W, i / W);
            let p = img.data[i];
            let v = match (y & 1, x & 1) {
                (0, 0) => p[0],
                (1, 1) => p[2],
                _ => p[1],
            };
            (black + v.clamp(0.0, 1.0) * (white - black)) as u16
        })
        .collect();
    lcc_raw::RawImage {
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
        color: Default::default(),
        wb_multipliers: None,
        linearized: false,
        pixel_aspect: 1.0,
        opcodes: Default::default(),
        metadata: Default::default(),
        lens_metadata: Default::default(),
    }
}

fn luma(img: &Rgb32f) -> Vec<f32> {
    img.data.iter().map(|p| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]).collect()
}

/// Mean absolute 4-neighbour Laplacian of the luma: how much fine detail the image holds.
fn detail(img: &Rgb32f) -> f64 {
    let l = luma(img);
    let (w, h) = (img.width, img.height);
    let mut s = 0.0f64;
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = y * w + x;
            s += (4.0 * l[i] - l[i - 1] - l[i + 1] - l[i - w] - l[i + w]).abs() as f64;
        }
    }
    s / ((w - 2) * (h - 2)) as f64
}

fn save_crop(img: &Rgb32f, path: &std::path::Path) {
    let (cw, ch) = (512.min(img.width), 512.min(img.height));
    let (x0, y0) = (img.width - cw, img.height - ch); // the corner: the zone plate's finest rings
    let crop = Rgb32f::from_fn(cw, ch, |x, y| img.data[(y0 + y) * img.width + x0 + x]);
    let rgba = crop.to_srgb8();
    let bytes = lcc_codecs::encode_jpeg(&lcc_codecs::EncodeImage::rgba8(&rgba), 97, Default::default(), &Default::default()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let sizes: Vec<usize> = args
        .iter()
        .position(|a| a == "--sizes")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.split(',').filter_map(|v| v.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![2048, 2560, 3000, 3840]);
    let out_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/bench/bin-sharpness");
    std::fs::create_dir_all(&out_dir).unwrap();
    let mut scenes: Vec<(String, Rgb32f)> = vec![("zone plate".into(), zone_plate())];
    let lib = lcc_scenes::demo_library();
    for s in lib.iter().filter(|s| s.width > s.height).take(3) {
        let img = s.render(W, H);
        scenes.push((s.name.clone(), img));
    }
    println!("scene | long edge | way | SSIM vs full demosaic | detail kept | source ms");
    for (name, rgb) in &scenes {
        let raw = raw_of(rgb);
        let t = Instant::now();
        let full = raw.develop(lcc_raw::Method::Ahd).unwrap();
        let full_ms = t.elapsed().as_secs_f64() * 1e3;
        let slug: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase();
        for &edge in &sizes {
            let t = Instant::now();
            let reference = fit(&full, edge, edge, Filter::Lanczos3);
            let ref_ms = full_ms + t.elapsed().as_secs_f64() * 1e3;
            let ref8 = reference.to_srgb8();
            let ref_detail = detail(&reference);
            save_crop(&reference, &out_dir.join(format!("{slug}-{edge}-reference.jpg")));
            println!("{name} | {edge} | full demosaic | 1.0000 | 100 % | {ref_ms:.0}");
            let mut ways: Vec<(String, Rgb32f, f64)> = Vec::new();
            // today: the preview level's bin factor and its box fit, or the whole sensor
            if edge <= 2560 {
                let need = 2560 * 9 / 10;
                if let Some(k) = [8usize, 6, 4, 3, 2].into_iter().find(|&k| raw.can_bin(k) && W / k >= need) {
                    let t = Instant::now();
                    let b = raw.develop_binned_masked(k, 1.0).unwrap().unwrap().0;
                    let b = if b.width.max(b.height) > 2560 { fit(&b, 2560, 2560, Filter::Box) } else { b };
                    let o = resize(&b, reference.width, reference.height, Filter::Lanczos3);
                    ways.push((format!("today (bin {k}, box to 2560)"), o, t.elapsed().as_secs_f64() * 1e3));
                }
            }
            for k in [2usize, 4, 6, 8] {
                if !raw.can_bin(k) || W / k < edge {
                    continue;
                }
                let t = Instant::now();
                let b = raw.develop_binned_masked(k, 1.0).unwrap().unwrap().0;
                let o = resize(&b, reference.width, reference.height, Filter::Lanczos3);
                ways.push((format!("bin {k} ({} px), Lanczos", W / k), o, t.elapsed().as_secs_f64() * 1e3));
            }
            for (way, o, ms) in ways {
                let s = lcc_fidelity::ssim(&o.to_srgb8(), &ref8).unwrap_or(f64::NAN);
                let d = detail(&o) / ref_detail * 100.0;
                let file: String = way.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
                save_crop(&o, &out_dir.join(format!("{slug}-{edge}-{file}.jpg")));
                println!("{name} | {edge} | {way} | {s:.4} | {d:.0} % | {ms:.0}");
            }
        }
    }
    println!("crops: {}", out_dir.display());
}
