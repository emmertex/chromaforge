//! `cargo run --release -p lcc-raw --example defect_crops -- FILE OUT_DIR [x,y ...]`
//!
//! For each position (full-data coordinates; default: what [`detect`] corrects), writes a JPEG with the 24 × 24
//! pixel neighbourhood at 100 % (bilinear demosaic, as-shot white balance, a plain gamma), enlarged 10×: the
//! original on the left, corrected on the right (given positions are corrected as known pixels, with their cluster). A diagnostic for checking detections by eye.

use lcc_raw::defects::{Fix, Thresholds, correct, detect, grow};
use lcc_raw::{Method, RawImage};

const HALF: usize = 12;
const ZOOM: usize = 10;

fn crop(raw: &RawImage, cx: usize, cy: usize) -> Vec<[f32; 3]> {
    let mut window = raw.clone();
    // develop only a window around the point: shrink the active area to it (the CFA stays anchored at 0, 0)
    let a = raw.active_area;
    let x0 = cx.saturating_sub(HALF + 4).max(a.x);
    let y0 = cy.saturating_sub(HALF + 4).max(a.y);
    let x0 = x0 - (x0 % 6);
    let y0 = y0 - (y0 % 6);
    window.active_area = lcc_raw::Rect::new(x0, y0, (2 * HALF + 12).min(a.x + a.width - x0), (2 * HALF + 12).min(a.y + a.height - y0));
    window.crop = lcc_raw::Rect::default();
    let rgb = window.develop(Method::Bilinear).unwrap_or_else(|_| lcc_raw::Rgb32f::new(1, 1));
    let wb = raw.wb_multipliers.unwrap_or([2.0, 1.0, 1.6]);
    let (ox, oy) = (cx - x0, cy - y0);
    let mut out = Vec::new();
    for y in 0..2 * HALF {
        for x in 0..2 * HALF {
            let (sx, sy) = ((ox + x).saturating_sub(HALF), (oy + y).saturating_sub(HALF));
            let p = if sx < rgb.width && sy < rgb.height { rgb.get(sx, sy) } else { [0.0; 3] };
            out.push([p[0] * wb[0], p[1] * wb[1], p[2] * wb[2]]);
        }
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(path), Some(dir)) = (args.first(), args.get(1)) else {
        eprintln!("usage: defect_crops FILE OUT_DIR [x,y ...]");
        return;
    };
    let Ok(bytes) = std::fs::read(path) else { return };
    let Ok(raw) = lcc_raw::decode(&bytes) else { return };
    let found = detect(&raw, &Thresholds::CORRECT);
    let mut points: Vec<(usize, usize)> =
        args.iter().skip(2).filter_map(|p| p.split_once(',')).filter_map(|(x, y)| Some((x.parse().ok()?, y.parse().ok()?))).collect();
    if points.is_empty() {
        points = found.defects.iter().map(|d| (d.x as usize, d.y as usize)).collect();
    }
    let mut fixed = raw.clone();
    let mut fixes: Vec<Fix> = found.defects.iter().map(Fix::from).collect();
    let asked: Vec<Fix> = points.iter().map(|&(x, y)| Fix { x: x as u32, y: y as u32, s: 0 }).collect();
    // given positions are treated as a camera body's known pixels: their cluster mates go too
    fixes.extend(grow(&raw, &asked));
    fixes.extend(asked);
    fixes.sort_by_key(|f| (f.x, f.y));
    fixes.dedup();
    correct(&mut fixed, &fixes);
    let stem = std::path::Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or("raw").to_string();
    for (cx, cy) in points {
        if std::env::var_os("DEFECT_DUMP").is_some() {
            // the 7 × 7 samples around the point, before | after (stored values)
            for y in cy.saturating_sub(3)..=cy + 3 {
                let row = |r: &RawImage| {
                    (cx.saturating_sub(3)..=cx + 3).map(|x| format!("{:6}", r.data.get(y * r.width + x) as u32)).collect::<Vec<_>>().join("")
                };
                println!("{} |{}", row(&raw), row(&fixed));
            }
        }
        let (a, b) = (crop(&raw, cx, cy), crop(&fixed, cx, cy));
        // brightness: the window's 99th percentile to 0.9
        let mut lum: Vec<f32> = a.iter().map(|p| p[1]).collect();
        lum.sort_by(f32::total_cmp);
        let gain = 0.9 / lum.get(lum.len() * 98 / 100).copied().unwrap_or(1.0).max(1e-4);
        let side = 2 * HALF * ZOOM;
        let (w, h) = (2 * side + ZOOM, side);
        let mut px = vec![40u8; w * h * 3];
        for (k, img) in [a, b].iter().enumerate() {
            for y in 0..h {
                for x in 0..side {
                    let p = img[(y / ZOOM) * 2 * HALF + x / ZOOM];
                    let o = (y * w + k * (side + ZOOM) + x) * 3;
                    for c in 0..3 {
                        px[o + c] = ((p[c] * gain).clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0) as u8;
                    }
                }
            }
        }
        let out = format!("{dir}/{stem}_{cx}_{cy}.jpg");
        let mut buf = Vec::new();
        if jpeg_encoder::Encoder::new(&mut buf, 92).encode(&px, w as u16, h as u16, jpeg_encoder::ColorType::Rgb).is_ok() {
            let _ = std::fs::write(&out, buf);
            println!("{out}");
        }
    }
}
