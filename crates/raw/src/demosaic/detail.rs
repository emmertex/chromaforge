//! Enhance ▸ Raw Details demosaic for Bayer mosaics: a slower, edge-aware pass for the enhanced DNG.
//!
//! Own design built from published ideas:
//! 1. Green everywhere: horizontal and vertical Hamilton–Adams estimates (neighbour mean plus a same-colour
//!    Laplacian correction; Hamilton & Adams, US 5,629,734, 1997), blended with weights from the inverse squared
//!    directional gradient summed over a 3×3 window instead of a hard choice, as in Pekkucuksen & Altunbasak's
//!    gradient-based threshold-free demosaicing (ICASSP 2010).
//! 2. Red and blue as residuals against the full green (the colour-difference / residual idea of Kiku et al.,
//!    "Residual interpolation for color image demosaicking", ICIP 2013): the difference `R − G` (`B − G`) known at its
//!    own sites is spread first along diagonals to the opposite-colour sites, then along the axes to the green sites,
//!    each neighbour weighted by how little green changes in its direction, so differences do not bleed across edges.
//!
//! A constant-colour mosaic is reproduced exactly and known samples are kept.

use super::Mosaic;
use crate::Rgb32f;
use lcc_raster::par_rows;

const EPS: f32 = 1e-6;

/// A full-size plane read with mirror reflection (keeps Bayer parity).
struct Plane<'a> {
    w: usize,
    h: usize,
    data: &'a [f32],
}

impl Plane<'_> {
    #[inline]
    fn get(&self, x: isize, y: isize) -> f32 {
        self.data.get(super::reflect(y, self.h) * self.w + super::reflect(x, self.w)).copied().unwrap_or(0.0)
    }
}

/// Finite or 0: a NaN sample (a damaged float raw) must not spread through the weights.
#[inline]
fn finite(v: f32) -> f32 {
    if v.is_finite() { v } else { 0.0 }
}

pub(crate) fn detail(m: &Mosaic) -> Rgb32f {
    let (w, h) = (m.w, m.h);
    let raw: Vec<f32> = m.data.iter().map(|&v| finite(v)).collect();
    let mo = Mosaic { w, h, data: &raw, cfa: m.cfa };
    // directional gradients at every pixel (any row: ±1 is another colour, ±2 the same)
    let mut dh = vec![0.0f32; w * h];
    let mut dv = vec![0.0f32; w * h];
    par_rows(&mut dh, w, |y, row| {
        let y = y as isize;
        for (x, d) in row.iter_mut().enumerate() {
            let x = x as isize;
            *d = (mo.get(x - 1, y) - mo.get(x + 1, y)).abs() + (2.0 * mo.get(x, y) - mo.get(x - 2, y) - mo.get(x + 2, y)).abs();
        }
    });
    par_rows(&mut dv, w, |y, row| {
        let y = y as isize;
        for (x, d) in row.iter_mut().enumerate() {
            let x = x as isize;
            *d = (mo.get(x, y - 1) - mo.get(x, y + 1)).abs() + (2.0 * mo.get(x, y) - mo.get(x, y - 2) - mo.get(x, y + 2)).abs();
        }
    });
    let (ph, pv) = (Plane { w, h, data: &dh }, Plane { w, h, data: &dv });
    let window = |p: &Plane, x: isize, y: isize| -> f32 {
        let mut s = 0.0;
        for dy in -1..=1 {
            for dx in -1..=1 {
                s += p.get(x + dx, y + dy);
            }
        }
        s
    };
    // 1. green
    let mut green = vec![0.0f32; w * h];
    par_rows(&mut green, w, |y, row| {
        let yi = y as isize;
        for (x, g) in row.iter_mut().enumerate() {
            let xi = x as isize;
            let c = mo.get(xi, yi);
            if mo.color(xi, yi) == 1 {
                *g = c;
                continue;
            }
            let gh = (mo.get(xi - 1, yi) + mo.get(xi + 1, yi)) * 0.5 + (2.0 * c - mo.get(xi - 2, yi) - mo.get(xi + 2, yi)) * 0.25;
            let gv = (mo.get(xi, yi - 1) + mo.get(xi, yi + 1)) * 0.5 + (2.0 * c - mo.get(xi, yi - 2) - mo.get(xi, yi + 2)) * 0.25;
            let wh = 1.0 / (window(&ph, xi, yi) + EPS).powi(2);
            let wv = 1.0 / (window(&pv, xi, yi) + EPS).powi(2);
            let v = (wh * gh + wv * gv) / (wh + wv);
            // keep the Laplacian correction from overshooting past the four green neighbours by more than their spread
            let n = [mo.get(xi - 1, yi), mo.get(xi + 1, yi), mo.get(xi, yi - 1), mo.get(xi, yi + 1)];
            let (lo, hi) = n.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), &v| (a.min(v), b.max(v)));
            let spread = hi - lo;
            *g = finite(super::clamp_between(v, lo - spread, hi + spread));
        }
    });
    let gp = Plane { w, h, data: &green };
    // 2. residuals for red (0) and blue (2)
    let mut diff = [vec![0.0f32; w * h], vec![0.0f32; w * h]];
    for (k, colour) in [0u8, 2u8].into_iter().enumerate() {
        // known sites
        let mut known = vec![0.0f32; w * h];
        par_rows(&mut known, w, |y, row| {
            for (x, d) in row.iter_mut().enumerate() {
                let (xi, yi) = (x as isize, y as isize);
                if mo.color(xi, yi) == colour {
                    *d = mo.get(xi, yi) - gp.get(xi, yi);
                }
            }
        });
        // diagonals → the opposite colour's sites
        let kp = Plane { w, h, data: &known };
        let mut diag = known.clone();
        par_rows(&mut diag, w, |y, row| {
            for (x, d) in row.iter_mut().enumerate() {
                let (xi, yi) = (x as isize, y as isize);
                let own = mo.color(xi, yi);
                if own == colour || own == 1 {
                    continue;
                }
                let g0 = gp.get(xi, yi);
                let (mut s, mut ws) = (0.0f32, 0.0f32);
                for (dx, dy) in [(-1isize, -1isize), (1, -1), (-1, 1), (1, 1)] {
                    let grad = (g0 - gp.get(xi + dx, yi + dy)).abs() + (gp.get(xi + dx, yi + dy) - gp.get(xi + 2 * dx, yi + 2 * dy)).abs();
                    let wt = 1.0 / (grad + 1e-4);
                    s += wt * kp.get(xi + dx, yi + dy);
                    ws += wt;
                }
                *d = s / ws;
            }
        });
        // axes → the green sites
        let dp = Plane { w, h, data: &diag };
        let mut full = diag.clone();
        par_rows(&mut full, w, |y, row| {
            for (x, d) in row.iter_mut().enumerate() {
                let (xi, yi) = (x as isize, y as isize);
                if mo.color(xi, yi) != 1 {
                    continue;
                }
                let g0 = gp.get(xi, yi);
                let (mut s, mut ws) = (0.0f32, 0.0f32);
                for (dx, dy) in [(-1isize, 0isize), (1, 0), (0, -1), (0, 1)] {
                    let grad = (g0 - gp.get(xi + dx, yi + dy)).abs() + (gp.get(xi + dx, yi + dy) - gp.get(xi + 2 * dx, yi + 2 * dy)).abs();
                    let wt = 1.0 / (grad + 1e-4);
                    s += wt * dp.get(xi + dx, yi + dy);
                    ws += wt;
                }
                *d = s / ws;
            }
        });
        if let Some(slot) = diff.get_mut(k) {
            *slot = full;
        }
    }
    let mut out = Rgb32f::new(w, h);
    let [dr, db] = &diff;
    par_rows(&mut out.data, w, |y, row| {
        for (x, px) in row.iter_mut().enumerate() {
            let i = y * w + x;
            let g = green.get(i).copied().unwrap_or(0.0);
            let mut p = [g + dr.get(i).copied().unwrap_or(0.0), g, g + db.get(i).copied().unwrap_or(0.0)];
            // the sample itself is kept exactly
            let own = m.cfa.color_at(x, y) as usize;
            if let (Some(slot), Some(v)) = (p.get_mut(own), raw.get(i)) {
                *slot = *v;
            }
            *px = p;
        }
    });
    out
}
