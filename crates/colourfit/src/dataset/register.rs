//! Registration of our developed raw against Lightroom's export: isotropic scale about the centre plus a
//! translation, found by a coarse-to-fine search maximising the normalised cross-correlation of high-pass
//! log-luminance. Both grids share the same pixel size (the export is reduced to our width).
//!
//! Sources: own design (exhaustive NCC search; normalised cross-correlation as in Lewis 1995, "Fast normalized
//! cross-correlation").
use super::grid::{Grid, bilinear, box_blur};
use rayon::prelude::*;

/// Maps a pixel of ours to the export: `p_e = c_e + scale · (p_o − c_o) + t` (pixel units of the shared grid).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub scale: f32,
    pub tx: f32,
    pub ty: f32,
    /// Normalised cross-correlation at the optimum (−1..1).
    pub score: f32,
}

impl Transform {
    /// Export position of our point `(x, y)`, for grids of size `ours` and `theirs`, scaled by `k` (a grid `k`
    /// times the registration grid's resolution).
    pub fn map(&self, x: f32, y: f32, ours: (usize, usize), theirs: (usize, usize), k: f32) -> (f32, f32) {
        let (co_x, co_y) = (ours.0 as f32 / 2.0, ours.1 as f32 / 2.0);
        let (ce_x, ce_y) = (theirs.0 as f32 / 2.0, theirs.1 as f32 / 2.0);
        (ce_x + self.scale * (x - co_x) + self.tx * k, ce_y + self.scale * (y - co_y) + self.ty * k)
    }
}

/// High-pass log luminance (log minus its local mean), the registration feature.
pub fn feature(lum: &Grid<f32>) -> Grid<f32> {
    let lg = lum.map(|v| (v.max(0.0) + 1e-3).ln());
    let r = (lum.w.max(lum.h) / 64).max(2);
    let bl = box_blur(&lg, r);
    Grid { w: lg.w, h: lg.h, data: lg.data.iter().zip(&bl.data).map(|(a, b)| a - b).collect() }
}

/// 2× box reduction of a scalar grid.
fn half(g: &Grid<f32>) -> Grid<f32> {
    let (w, h) = ((g.w / 2).max(1), (g.h / 2).max(1));
    let mut o = Grid::filled(w, h, 0.0);
    for y in 0..h {
        for x in 0..w {
            let s = g.at(2 * x, 2 * y) + g.at(2 * x + 1, 2 * y) + g.at(2 * x, 2 * y + 1) + g.at(2 * x + 1, 2 * y + 1);
            o.set(x, y, s / 4.0);
        }
    }
    o
}

/// Our sample points (inner 80 %, every `stride`), with zero-mean values.
fn points(a: &Grid<f32>, stride: usize) -> Vec<(f32, f32, f32)> {
    let (mx, my) = (a.w / 10, a.h / 10);
    let mut pts = Vec::new();
    for y in (my..a.h.saturating_sub(my)).step_by(stride.max(1)) {
        for x in (mx..a.w.saturating_sub(mx)).step_by(stride.max(1)) {
            pts.push((x as f32 + 0.5, y as f32 + 0.5, a.at(x, y)));
        }
    }
    let n = pts.len().max(1) as f32;
    let mean = pts.iter().map(|p| p.2).sum::<f32>() / n;
    for p in &mut pts {
        p.2 -= mean;
    }
    pts
}

fn ncc(pts: &[(f32, f32, f32)], ours: (usize, usize), b: &Grid<f32>, t: &Transform) -> f32 {
    let (mut sa, mut sab, mut saa, mut sb, mut sbb, mut n) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0usize);
    for &(x, y, va) in pts {
        let (u, v) = t.map(x, y, ours, (b.w, b.h), 1.0);
        let Some(vb) = bilinear(b, u, v) else { continue };
        let (va, vb) = (va as f64, vb as f64);
        sa += va;
        sab += va * vb;
        saa += va * va;
        sb += vb;
        sbb += vb * vb;
        n += 1;
    }
    // need most of the points inside the export
    if n < pts.len() / 2 || n < 16 {
        return -1.0;
    }
    let nf = n as f64;
    let cov = sab - sa * sb / nf;
    let (va, vb) = (saa - sa * sa / nf, sbb - sb * sb / nf);
    let d = (va * vb).max(0.0).sqrt();
    if d > 1e-12 && d.is_finite() { (cov / d) as f32 } else { -1.0 }
}

fn search(pts: &[(f32, f32, f32)], ours: (usize, usize), b: &Grid<f32>, scales: &[f32], shifts: &[(f32, f32)]) -> Transform {
    let cands: Vec<(f32, f32, f32)> = scales.iter().flat_map(|&s| shifts.iter().map(move |&(x, y)| (s, x, y))).collect();
    cands
        .par_iter()
        .map(|&(scale, tx, ty)| {
            let mut t = Transform { scale, tx, ty, score: -1.0 };
            t.score = ncc(pts, ours, b, &t);
            t
        })
        .reduce(|| Transform { scale: 1.0, tx: 0.0, ty: 0.0, score: -2.0 }, |a, b| if b.score > a.score { b } else { a })
}

fn grid2(range: f32, step: f32) -> Vec<(f32, f32)> {
    let n = (range / step).round().clamp(0.0, 200.0) as i32;
    let mut v = Vec::new();
    for j in -n..=n {
        for i in -n..=n {
            v.push((i as f32 * step, j as f32 * step));
        }
    }
    v
}

fn steps(c: f32, half: f32, step: f32) -> Vec<f32> {
    let n = (half / step).round().clamp(0.0, 200.0) as i32;
    (-n..=n).map(|i| c + i as f32 * step).collect()
}

/// Register feature grids `a` (ours) and `b` (export, same pixel size). `max_shift` and `max_scale` are
/// fractions of the width / of 1.
pub fn register(a: &Grid<f32>, b: &Grid<f32>, max_shift: f32, max_scale: f32) -> Transform {
    // coarse, at half resolution
    let (ah, bh) = (half(a), half(b));
    let pts = points(&ah, 1);
    let range = (max_shift * ah.w as f32).max(1.0);
    let coarse = search(&pts, (ah.w, ah.h), &bh, &steps(1.0, max_scale, 0.005), &grid2(range, 1.0));
    // fine, at full resolution around the coarse optimum
    let pts = points(a, 1);
    let shifts: Vec<(f32, f32)> = grid2(2.0, 0.5).into_iter().map(|(x, y)| (coarse.tx * 2.0 + x, coarse.ty * 2.0 + y)).collect();
    search(&pts, (a.w, a.h), b, &steps(coarse.scale, 0.005, 0.001), &shifts)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A textured scene sampled at `(x, y)` in scene units.
    fn scene(x: f32, y: f32) -> f32 {
        let v = (x * 0.21).sin() * (y * 0.17).cos() + (x * 0.05 + y * 0.09).sin() + ((x * 0.6).sin() * (y * 0.45).sin()) * 0.5;
        (v + 3.0) * 0.1
    }

    #[test]
    fn recovers_shift_and_scale() {
        let (w, h) = (200usize, 140usize);
        let a = Grid { w, h, data: (0..w * h).map(|i| scene((i % w) as f32 + 0.5, (i / w) as f32 + 0.5)).collect() };
        // export: our point p shows at c + 1.02 (p − c) + (3, −2)
        let (s, tx, ty) = (1.02f32, 3.0f32, -2.0f32);
        let b = Grid {
            w,
            h,
            data: (0..w * h)
                .map(|i| {
                    let (u, v) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
                    let x = (u - w as f32 / 2.0 - tx) / s + w as f32 / 2.0;
                    let y = (v - h as f32 / 2.0 - ty) / s + h as f32 / 2.0;
                    scene(x, y)
                })
                .collect(),
        };
        let t = register(&feature(&a), &feature(&b), 0.05, 0.04);
        assert!((t.scale - s).abs() < 0.004, "{t:?}");
        assert!((t.tx - tx).abs() < 0.75 && (t.ty - ty).abs() < 0.75, "{t:?}");
        assert!(t.score > 0.8, "{t:?}");
    }

    #[test]
    fn unrelated_images_score_low() {
        let (w, h) = (160usize, 120usize);
        let a = Grid { w, h, data: (0..w * h).map(|i| scene((i % w) as f32, (i / w) as f32)).collect() };
        let mut seed = 12345u32;
        let b = Grid {
            w,
            h,
            data: (0..w * h)
                .map(|_| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (seed >> 8) as f32 / (1u32 << 24) as f32
                })
                .collect(),
        };
        let t = register(&feature(&a), &feature(&b), 0.04, 0.03);
        assert!(t.score < 0.3, "{t:?}");
    }
}
