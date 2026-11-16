//! Colour-pair sampling on registered grids: masks (clipped, near black, borders, busy texture), bin balance over
//! lightness × chroma × hue, and the per-photo cap.
//!
//! Sources: own design; CIE 1976 L*a*b* (CIE 15:2004).
use super::grid::{Grid, bilinear};
use super::register::Transform;
use crate::dataset::Sample;

/// Camera channel at or above this (white = 1) counts as clipped.
const CAMERA_CLIP: f32 = 0.98;
/// Export luminance at or above this (ProPhoto white Y = 1) counts as clipped.
const EXPORT_CLIP: f32 = 0.92;
const CAMERA_BLACK: f32 = 0.003;
const EXPORT_BLACK: f32 = 0.0015;
/// Local coefficient of variation above which a cell sits on an edge or in busy texture.
const MAX_COV: f32 = 0.12;
const BORDER: f32 = 0.05;

const L_BINS: usize = 5;
const HUE_BINS: usize = 12;
const CHROMA_EDGES: [f32; 3] = [8.0, 20.0, 40.0];

/// CIE L*a*b* (D50) of XYZ D50.
pub fn lab(xyz: [f32; 3]) -> [f32; 3] {
    const W: [f32; 3] = [0.9642, 1.0, 0.8249];
    let f = |t: f32| if t > 216.0 / 24389.0 { t.cbrt() } else { (24389.0 / 27.0 * t + 16.0) / 116.0 };
    let (fx, fy, fz) = (f(xyz[0] / W[0]), f(xyz[1] / W[1]), f(xyz[2] / W[2]));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

fn bin_of(xyz: [f32; 3]) -> usize {
    let [l, a, b] = lab(xyz);
    let lb = ((l / 100.0 * L_BINS as f32).floor().max(0.0) as usize).min(L_BINS - 1);
    let c = (a * a + b * b).sqrt();
    let per_l = 1 + (CHROMA_EDGES.len()) * HUE_BINS;
    let cb = CHROMA_EDGES.iter().filter(|e| c >= **e).count();
    if cb == 0 {
        return lb * per_l;
    }
    let h = b.atan2(a).rem_euclid(std::f32::consts::TAU);
    let hb = ((h / std::f32::consts::TAU * HUE_BINS as f32) as usize).min(HUE_BINS - 1);
    lb * per_l + 1 + (cb - 1) * HUE_BINS + hb
}

fn cov(vals: &[f32]) -> f32 {
    let n = vals.len().max(1) as f32;
    let m = vals.iter().sum::<f32>() / n;
    if m <= 1e-6 {
        return f32::INFINITY;
    }
    let var = vals.iter().map(|v| (v - m) * (v - m)).sum::<f32>() / n;
    var.sqrt() / m
}

/// Sample pairs. `cam` / `cam_max` are ours at the registration grid (camera RGB, and per-cell max channel);
/// `xyz` is the export at the same pixel size; `t` maps ours to the export. Sampling cells are 2 × 2 grid pixels.
pub fn sample(cam: &Grid<[f32; 3]>, cam_max: &Grid<f32>, xyz: &Grid<[f32; 3]>, t: &Transform, per_photo: usize) -> Vec<Sample> {
    let (pw, ph) = (cam.w / 2, cam.h / 2);
    let (bx, by) = ((pw as f32 * BORDER).ceil() as usize, (ph as f32 * BORDER).ceil() as usize);
    let ours = (cam.w, cam.h);
    let theirs = (xyz.w, xyz.h);
    let mut cands: Vec<(usize, Sample)> = Vec::new();
    for py in by..ph.saturating_sub(by) {
        'cell: for px in bx..pw.saturating_sub(bx) {
            let mut c_acc = [0.0f32; 3];
            let mut e_acc = [0.0f32; 3];
            let mut cmax = 0.0f32;
            let mut gs = [0.0f32; 16];
            let mut ys = [0.0f32; 16];
            let mut k = 0usize;
            // a 4 × 4 neighbourhood centred on the 2 × 2 cell, for the texture test
            for dy in 0..4usize {
                for dx in 0..4usize {
                    let (Some(x), Some(y)) = ((2 * px + dx).checked_sub(1), (2 * py + dy).checked_sub(1)) else { continue 'cell };
                    if x >= cam.w || y >= cam.h {
                        continue 'cell;
                    }
                    let c = cam.at(x, y);
                    let (u, v) = t.map(x as f32 + 0.5, y as f32 + 0.5, ours, theirs, 1.0);
                    let Some(e) = bilinear(xyz, u, v) else { continue 'cell };
                    if let (Some(g), Some(yy)) = (gs.get_mut(k), ys.get_mut(k)) {
                        *g = c[1];
                        *yy = e[1];
                    }
                    k += 1;
                    if (1..3).contains(&dx) && (1..3).contains(&dy) {
                        for i in 0..3 {
                            c_acc[i] += c[i] / 4.0;
                            e_acc[i] += e[i] / 4.0;
                        }
                        cmax = cmax.max(cam_max.at(x, y));
                    }
                }
            }
            let pos = [(2 * px + 1) as f32 / cam.w.max(1) as f32, (2 * py + 1) as f32 / cam.h.max(1) as f32];
            if !c_acc.iter().chain(&e_acc).all(|v| v.is_finite()) {
                continue;
            }
            if cmax >= CAMERA_CLIP || e_acc[1] >= EXPORT_CLIP {
                continue;
            }
            if c_acc.iter().cloned().fold(0.0, f32::max) < CAMERA_BLACK || e_acc[1] < EXPORT_BLACK {
                continue;
            }
            if cov(&gs) > MAX_COV || cov(&ys) > MAX_COV {
                continue;
            }
            cands.push((bin_of(e_acc), Sample { camera: c_acc, lightroom_xyz: e_acc, weight: 1.0, pos }));
        }
    }
    balance(cands, per_photo)
}

/// Cap each bin, weight bins by the square root of their population, keep at most `per_photo`.
fn balance(mut cands: Vec<(usize, Sample)>, per_photo: usize) -> Vec<Sample> {
    if cands.is_empty() || per_photo == 0 {
        return Vec::new();
    }
    cands.sort_by_key(|c| c.0);
    let mut bins: Vec<Vec<Sample>> = Vec::new();
    let mut last = usize::MAX;
    for (b, s) in cands {
        if b != last {
            bins.push(Vec::new());
            last = b;
        }
        if let Some(v) = bins.last_mut() {
            v.push(s);
        }
    }
    let cap = per_photo.div_ceil(bins.len().max(1)).max(1);
    let mut out = Vec::new();
    for bin in &bins {
        let n = bin.len();
        let take = n.min(cap);
        // the bin's total weight grows with the square root of what it had
        let w = (n as f32).sqrt() / take.max(1) as f32;
        for i in 0..take {
            // evenly spaced picks, deterministic
            if let Some(s) = bin.get(i * n / take.max(1)) {
                out.push(Sample { weight: w, ..*s });
            }
        }
    }
    if out.len() > per_photo {
        let n = out.len();
        out = (0..per_photo).filter_map(|i| out.get(i * n / per_photo).copied()).collect();
    }
    let mean = out.iter().map(|s| s.weight).sum::<f32>() / out.len().max(1) as f32;
    if mean > 0.0 && mean.is_finite() {
        for s in &mut out {
            s.weight /= mean;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_clipped_black_and_respects_cap() {
        let (w, h) = (80usize, 60usize);
        // left half: flat patches of distinct colours; one clipped region; one black region
        let cam = Grid {
            w,
            h,
            data: (0..w * h)
                .map(|i| {
                    let (x, y) = (i % w, i / w);
                    let patch = (x / 10 + (y / 10) * 8) as f32;
                    [0.1 + 0.01 * patch, 0.2 + 0.005 * patch, 0.15]
                })
                .collect(),
        };
        let mut cam_max = cam.map(|p| p[0].max(p[1]).max(p[2]));
        for y in 0..h {
            for x in 0..20 {
                cam_max.set(x, y, 1.0);
            }
        }
        let xyz = cam.map(|p| [p[0] * 0.9, p[1], p[2] * 0.8]);
        let t = Transform { scale: 1.0, tx: 0.0, ty: 0.0, score: 1.0 };
        let s = sample(&cam, &cam_max, &xyz, &t, 50);
        assert!(!s.is_empty() && s.len() <= 50);
        assert!(s.iter().all(|s| s.camera.iter().all(|v| *v < CAMERA_CLIP)));
        assert!(s.iter().all(|s| s.pos.iter().all(|v| (0.0..=1.0).contains(v))));
        let mean_w = s.iter().map(|s| s.weight).sum::<f32>() / s.len() as f32;
        assert!((mean_w - 1.0).abs() < 1e-3);
    }

    #[test]
    fn empty_and_tiny_inputs_give_nothing() {
        let g = Grid::filled(1, 1, [0.5f32; 3]);
        let m = Grid::filled(1, 1, 0.5f32);
        let t = Transform { scale: 1.0, tx: 0.0, ty: 0.0, score: 1.0 };
        assert!(sample(&g, &m, &g, &t, 100).is_empty());
        let g = Grid::filled(0, 0, [0.5f32; 3]);
        assert!(sample(&g, &Grid::filled(0, 0, 0.0), &g, &t, 100).is_empty());
    }
}
