//! The reconstruction: all 16 frames' raw samples, each at its measured position, onto the half-pixel node grid.
//!
//! Method: local kernel regression with data-adaptive anisotropic Gaussian kernels, after Takeda, Farsiu and Milanfar,
//! "Kernel regression for image processing and reconstruction", IEEE Trans. Image Processing 16(2), 2007 (steering
//! kernels, local polynomial fit) and Wronski et al., "Handheld multi-frame super-resolution", ACM Trans. Graphics 38(4),
//! 2019 (kernel shape from the local structure tensor, samples merged by normalised convolution). The fit is local
//! linear (a kernel centroid off the node causes no error) and ridge-regularised towards zero slope.
//!
//! * Green: the narrow kernel is used directly where it holds (nearly) two samples on the node, as in the plain
//!   placement; elsewhere the scale grows until the kernel holds a target number of effective samples.
//! * Red and blue: the colour ratios R/G and B/G (the reconstructed green interpolated at each red or blue sample) are
//!   smoothed with wider kernels and the reconstructed green is multiplied back (colour-plane correlation: Gunturk,
//!   Altunbasak and Mersereau, IEEE Trans. Image Processing 11(9), 2002). A sample that sits on a node passes through.
//! * All samples are used, including the blue samples of the rows the camera fills itself (raw rows 5 mod 12), as the
//!   plain merge does.
//!
//! A node `(X, Y)` lies at the sensor site `(X / 2, Y / 2)` of frame 0; a sample of frame `k` at site `(x, y)` lies at
//! the node `2 (x + d_k(x, y))` with `d_k` the frame's similarity model. The image is computed in tiles of 192 × 192
//! nodes with an 8-node margin, in parallel; every tile is computed by one thread in a fixed order, so the result does
//! not depend on the number of threads.

use super::gain::Gains;
use super::model::Similarity;
use super::samples::Samples;
use crate::pixelshift::Merged;
use crate::{MergeError, Result};
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// Reconstruction parameters (the frozen version 4 values).
#[derive(Debug, Clone, PartialEq)]
pub struct Params {
    /// Narrow kernel (sites): used where a sample already sits (almost) on the node.
    pub sigma_small: f64,
    /// Anisotropy: sigma along an edge is `sigma (1 + stretch coherence)`, across it `sigma / sqrt(that)`.
    pub stretch: f64,
    pub ridge: f64,
    pub black: f64,
    /// Noise variance of a sample is `noise_a` times its level above black.
    pub noise_a: f64,
    /// Target effective number of samples for green and for the colour ratios.
    pub n_g: f64,
    pub n_d: f64,
    pub g_scales: [f64; 4],
    pub d_scales: [f64; 3],
    pub ridge_g: f64,
    pub ridge_d: f64,
    /// The narrow-kernel green is used directly from this effective sample count (full weight 0.6 above it).
    pub g_lo: f64,
    /// Red and blue by colour ratio (true, the frozen setting) or by colour difference.
    pub ratio: bool,
}

impl Params {
    /// The frozen version 4 parameters for frames with this black level and noise.
    pub fn v4(black: f64, noise_a: f64) -> Params {
        Params {
            sigma_small: 0.08,
            stretch: 0.25,
            ridge: 0.05,
            black,
            noise_a,
            n_g: 2.25,
            n_d: 2.0,
            g_scales: [0.10, 0.13, 0.17, 0.22],
            d_scales: [0.2, 0.3, 0.45],
            ridge_g: 0.3,
            ridge_d: 0.3,
            g_lo: 1.2,
            ratio: true,
        }
    }
}

#[derive(Clone, Copy)]
struct Smp {
    x: f32,
    y: f32,
    v: f32,
}

const MARGIN: usize = 8;
const TILE: usize = 192;

/// What the tiles read: the samples (with the frames' gains applied), the frames' models and the geometry.
struct Input<'a> {
    src: &'a dyn Samples,
    gains: Option<&'a Gains>,
    models: &'a [Similarity],
    cx: f64,
    cy: f64,
    black: f64,
}

impl Input<'_> {
    #[inline]
    fn sample(&self, k: usize, x: i64, y: i64) -> Option<(u8, f32)> {
        let (c, v) = self.src.sample(k, x, y)?;
        match self.gains.and_then(|g| g.per_frame.get(k)) {
            Some(g) => Some((c, (self.black + (f64::from(v) - self.black) / g[usize::from(c) % 3]) as f32)),
            None => Some((c, v)),
        }
    }

    /// Displacement (sites) of frame `k` at the active-area point `(x, y)`.
    #[inline]
    fn disp(&self, k: usize, x: f64, y: f64) -> [f64; 2] {
        self.models.get(k).map_or([0.0, 0.0], |m| m.at(x - self.cx, y - self.cy))
    }
}

/// The node box `[x0, x0 + w) × [y0, y0 + h)` as RGB in samples of the frames (black not subtracted). Nodes no sample
/// reaches (outside the frames) are 0; for the same inputs the result is bit-identical on every run.
pub fn reconstruct_box(
    src: &dyn Samples,
    gains: Option<&Gains>,
    models: &[Similarity],
    x0: i64,
    y0: i64,
    w: usize,
    h: usize,
    prm: &Params,
) -> Vec<[f32; 3]> {
    let (aw, ah) = src.size();
    let input = Input { src, gains, models, cx: aw as f64 / 2.0, cy: ah as f64 / 2.0, black: prm.black };
    let tiles: Vec<(usize, usize)> = (0..h.div_ceil(TILE)).flat_map(|j| (0..w.div_ceil(TILE)).map(move |i| (i * TILE, j * TILE))).collect();
    let parts: Vec<(usize, usize, usize, usize, Vec<[f32; 3]>)> = tiles
        .par_iter()
        .map(|&(tx, ty)| {
            let (tw, th) = (TILE.min(w - tx), TILE.min(h - ty));
            (tx, ty, tw, th, tile(&input, x0 + tx as i64, y0 + ty as i64, tw, th, prm))
        })
        .collect();
    let mut out = vec![[0f32; 3]; w * h];
    for (tx, ty, tw, th, d) in parts {
        for j in 0..th {
            for i in 0..tw {
                let p = d[j * tw + i];
                out[(ty + j) * w + tx + i] = p.map(|v| if v.is_nan() { 0.0 } else { v });
            }
        }
    }
    out
}

/// The whole image: `2 × 2` nodes per site, three samples per node on the frames' own scale (rounded, limited to
/// `[0, white]`), the frames' crop in output pixels. Nodes outside the reach of every sample are filled from the nearest
/// node that has one (counted in `filled`). `progress` is called after every row of tiles with the fraction done and
/// returns false to cancel.
pub fn reconstruct(
    src: &dyn Samples,
    gains: Option<&Gains>,
    models: &[Similarity],
    crop: [usize; 4],
    prm: &Params,
    progress: &(dyn Fn(f32) -> bool + Sync),
) -> Result<Merged> {
    let (aw, ah) = src.size();
    let (w, h) = (aw.checked_mul(2), ah.checked_mul(2));
    let (Some(w), Some(h)) = (w, h) else { return Err(MergeError::Output("image too large".into())) };
    let n = w.checked_mul(h).and_then(|v| v.checked_mul(3)).ok_or_else(|| MergeError::Output("image too large".into()))?;
    let mut data: Vec<u16> = Vec::new();
    data.try_reserve_exact(n).map_err(|_| MergeError::Output("not enough memory for the merged image".into()))?;
    data.resize(n, 0);
    let input = Input { src, gains, models, cx: aw as f64 / 2.0, cy: ah as f64 / 2.0, black: prm.black };
    let white = src.white();
    let cancelled = AtomicBool::new(false);
    let mut holes: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let bands = h.div_ceil(TILE);
    for j in 0..bands {
        if !progress(j as f32 / bands as f32) {
            return Err(MergeError::Cancelled);
        }
        let ty = j * TILE;
        let th = TILE.min(h - ty);
        let parts: Vec<Option<(usize, usize, Vec<[f32; 3]>)>> = (0..w.div_ceil(TILE))
            .into_par_iter()
            .map(|i| {
                if cancelled.load(Ordering::Relaxed) {
                    return None;
                }
                let tx = i * TILE;
                let tw = TILE.min(w - tx);
                Some((tx, tw, tile(&input, tx as i64, ty as i64, tw, th, prm)))
            })
            .collect();
        for part in parts {
            let Some((tx, tw, d)) = part else { return Err(MergeError::Cancelled) };
            for r in 0..th {
                for c in 0..tw {
                    let node = (ty + r) * w + tx + c;
                    for (ch, v) in d[r * tw + c].iter().enumerate() {
                        if v.is_nan() {
                            holes.insert(node * 3 + ch);
                        } else {
                            data[node * 3 + ch] = v.round().clamp(0.0, white as f32) as u16;
                        }
                    }
                }
            }
        }
    }
    if !progress(1.0) {
        return Err(MergeError::Cancelled);
    }
    let filled = fill_holes(&mut data, w, h, &holes);
    let [cx, cy, cw, ch] = crop;
    Ok(Merged { width: w, height: h, data, black: prm.black as f32, white: white as f32, scale: 2, crop: [cx * 2, cy * 2, cw * 2, ch * 2], filled })
}

/// Fills the channel values no sample reached from the nearest node (within 8) that has one, as the plain merge does.
fn fill_holes(data: &mut [u16], w: usize, h: usize, holes: &std::collections::HashSet<usize>) -> usize {
    let mut nodes: Vec<usize> = holes.iter().map(|i| i / 3).collect();
    nodes.sort_unstable();
    nodes.dedup();
    for &node in &nodes {
        let (x, y) = ((node % w) as i64, (node / w) as i64);
        for ch in 0..3 {
            if !holes.contains(&(node * 3 + ch)) {
                continue;
            }
            'search: for r in 1..=8i64 {
                for dy in -r..=r {
                    for dx in -r..=r {
                        let (nx, ny) = (x + dx, y + dy);
                        if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                            continue;
                        }
                        let j = ny as usize * w + nx as usize;
                        if !holes.contains(&(j * 3 + ch)) {
                            data[node * 3 + ch] = data[j * 3 + ch];
                            break 'search;
                        }
                    }
                }
            }
        }
    }
    nodes.len()
}

/// One tile of `tw × th` nodes at `(x0, y0)`; nodes no sample reached stay NaN.
fn tile(input: &Input, x0: i64, y0: i64, tw: usize, th: usize, prm: &Params) -> Vec<[f32; 3]> {
    let (ww, hh) = (tw + 2 * MARGIN, th + 2 * MARGIN);
    let (ox, oy) = (x0 - MARGIN as i64, y0 - MARGIN as i64); // node origin of the work box
    // samples of the work box, per colour, in node coordinates relative to (ox, oy)
    let mut by: [Vec<Smp>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for k in 0..input.src.frame_count() {
        // displacement at the middle of the box (the model varies slowly over a tile)
        let pm = input.disp(k, ox as f64 / 2.0 + ww as f64 / 4.0, oy as f64 / 2.0 + hh as f64 / 4.0);
        let (xa, xb) = (((ox - 6) as f64 / 2.0 - pm[0]).floor() as i64 - 1, ((ox + ww as i64 + 6) as f64 / 2.0 - pm[0]).ceil() as i64 + 1);
        let (ya, yb) = (((oy - 6) as f64 / 2.0 - pm[1]).floor() as i64 - 1, ((oy + hh as i64 + 6) as f64 / 2.0 - pm[1]).ceil() as i64 + 1);
        for y in ya..=yb {
            for x in xa..=xb {
                if let Some((c, v)) = input.sample(k, x, y) {
                    let d = input.disp(k, x as f64, y as f64);
                    let (nx, ny) = (2.0 * (x as f64 + d[0]) - ox as f64, 2.0 * (y as f64 + d[1]) - oy as f64);
                    by[usize::from(c) % 3].push(Smp { x: nx as f32, y: ny as f32, v });
                }
            }
        }
    }
    let om = kernel_shapes(&by[1], ww, hh, prm);
    pass_b(&by, &om, ww, hh, tw, th, prm)
}

/// The inverse covariance of the kernel of every node (for unit sigma), `(a, b, c)` for `[[a, b], [b, c]]`, from the
/// structure tensor of a first green pass (Gaussian 0.4 sites), smoothed over 5 × 5 nodes.
fn kernel_shapes(green: &[Smp], ww: usize, hh: usize, prm: &Params) -> Vec<[f32; 3]> {
    let mut s = vec![0f32; ww * hh];
    let mut wt = vec![0f32; ww * hh];
    let inv = 1.0 / (2.0 * 0.8 * 0.8);
    for p in green {
        let (ix, iy) = (p.x.floor() as i64, p.y.floor() as i64);
        for j in iy - 2..=iy + 3 {
            for i in ix - 2..=ix + 3 {
                if i < 0 || j < 0 || i >= ww as i64 || j >= hh as i64 {
                    continue;
                }
                let d2 = (p.x - i as f32).powi(2) + (p.y - j as f32).powi(2);
                let w = (-d2 * inv as f32).exp();
                s[j as usize * ww + i as usize] += w * p.v;
                wt[j as usize * ww + i as usize] += w;
            }
        }
    }
    let g0: Vec<f32> = s.iter().zip(&wt).map(|(a, b)| if *b > 1e-3 { a / b } else { f32::NAN }).collect();
    let mut t = vec![[0f32; 3]; ww * hh];
    for j in 1..hh - 1 {
        for i in 1..ww - 1 {
            let (a, b, c, d) = (g0[j * ww + i + 1], g0[j * ww + i - 1], g0[(j + 1) * ww + i], g0[(j - 1) * ww + i]);
            if a.is_nan() || b.is_nan() || c.is_nan() || d.is_nan() {
                continue;
            }
            let (gx, gy) = ((a - b) * 0.5, (c - d) * 0.5);
            t[j * ww + i] = [gx * gx, gx * gy, gy * gy];
        }
    }
    let mut ts = vec![[0f32; 3]; ww * hh];
    for j in 2..hh - 2 {
        for i in 2..ww - 2 {
            let mut a = [0f32; 3];
            for jj in j - 2..=j + 2 {
                for ii in i - 2..=i + 2 {
                    for q in 0..3 {
                        a[q] += t[jj * ww + ii][q] / 25.0;
                    }
                }
            }
            ts[j * ww + i] = a;
        }
    }
    let mut om = vec![[1f32, 0f32, 1f32]; ww * hh];
    for n in 0..ww * hh {
        let [gxx, gxy, gyy] = ts[n];
        let tr = gxx + gyy;
        let level = if g0[n].is_nan() { 1000.0 } else { (g0[n] - prm.black as f32).max(1.0) };
        let noise2 = (prm.noise_a as f32) * level; // noise variance of one sample
        if tr < 2.0 * noise2 {
            continue; // flat: isotropic
        }
        let det = ((gxx - gyy) * 0.5).powi(2) + gxy * gxy;
        let r = det.sqrt();
        let (l1, l2) = (tr * 0.5 + r, (tr * 0.5 - r).max(0.0));
        let coh = ((l1 - l2) / (l1 + l2 + 1e-9)).sqrt();
        let a_f = 1.0 + prm.stretch as f32 * coh;
        let th_ = 0.5 * (2.0 * gxy).atan2(gxx - gyy); // direction of the gradient
        let (c, sn) = (th_.cos(), th_.sin());
        let (sa2, sl2) = (1.0 / a_f, a_f * a_f); // sigma^2 across and along the edge, relative to sigma0^2
        om[n] = [c * c / sa2 + sn * sn / sl2, c * sn / sa2 - sn * c / sl2, sn * sn / sa2 + c * c / sl2];
    }
    om
}

/// Local linear regression of the samples at kernel scale `sg` (sites): the fitted value, the kernel weight sum and the
/// sum of squared weights per node.
fn fit_scale(samples: &[Smp], om: &[[f32; 3]], sg: f64, ww: usize, hh: usize, ridge: f64) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let rad = ((3.0 * sg * 2.0) / 0.5).ceil().max(1.0) as i64; // widest kernel: stretch up to 2
    let mut mo = vec![[0f32; 10]; ww * hh];
    let k2 = 1.0 / (sg * sg) as f32;
    for p in samples {
        let (ix, iy) = (p.x.round() as i64, p.y.round() as i64);
        for j in iy - rad..=iy + rad {
            for i in ix - rad..=ix + rad {
                if i < 0 || j < 0 || i >= ww as i64 || j >= hh as i64 {
                    continue;
                }
                let n = j as usize * ww + i as usize;
                let (dx, dy) = ((p.x - i as f32) * 0.5, (p.y - j as f32) * 0.5); // sites
                let o = om[n];
                let q = (o[0] * dx * dx + 2.0 * o[1] * dx * dy + o[2] * dy * dy) * k2;
                if q > 18.0 {
                    continue;
                }
                let wgt = (-0.5 * q).exp();
                let m = &mut mo[n];
                m[0] += wgt * p.v;
                m[1] += wgt * p.v * dx;
                m[2] += wgt * p.v * dy;
                m[3] += wgt;
                m[4] += wgt * dx;
                m[5] += wgt * dy;
                m[6] += wgt * dx * dx;
                m[7] += wgt * dx * dy;
                m[8] += wgt * dy * dy;
                m[9] += wgt * wgt;
            }
        }
    }
    let mut val = vec![f32::NAN; ww * hh];
    let mut ws = vec![0f32; ww * hh];
    let mut w2s = vec![0f32; ww * hh];
    for n in 0..ww * hh {
        let m: Vec<f64> = mo[n].iter().map(|v| f64::from(*v)).collect();
        let (w, wx, wy) = (m[3], m[4], m[5]);
        ws[n] = w as f32;
        w2s[n] = m[9] as f32;
        if w < 0.02 {
            continue;
        }
        let r = ridge * w * (sg * sg);
        let (wxx, wxy, wyy) = (m[6] + r, m[7], m[8] + r);
        let det = w * (wxx * wyy - wxy * wxy) - wx * (wx * wyy - wxy * wy) + wy * (wx * wxy - wxx * wy);
        val[n] = if det.abs() < 1e-18 {
            (m[0] / w) as f32
        } else {
            ((m[0] * (wxx * wyy - wxy * wxy) - wx * (m[1] * wyy - wxy * m[2]) + wy * (m[1] * wxy - wxx * m[2])) / det) as f32
        };
    }
    (val, ws, w2s)
}

/// Per node, the fit of the smallest scale whose kernel holds at least `n_t` effective samples (the largest scale
/// otherwise); effective samples are `(sum of weights)² / sum of squared weights`.
fn select_scale(samples: &[Smp], om: &[[f32; 3]], scales: &[f64], n_t: f64, ww: usize, hh: usize, ridge: f64) -> Vec<f32> {
    let mut out = vec![f32::NAN; ww * hh];
    let mut done = vec![false; ww * hh];
    for (si, &s) in scales.iter().enumerate() {
        let (v, w, w2) = fit_scale(samples, om, s, ww, hh, ridge);
        let last = si + 1 == scales.len();
        for n in 0..ww * hh {
            if done[n] || v[n].is_nan() {
                continue;
            }
            let neff = if w2[n] > 0.0 { f64::from(w[n] * w[n] / w2[n]) } else { 0.0 };
            if neff >= n_t || last {
                out[n] = v[n];
                done[n] = true;
            }
        }
    }
    out
}

fn fill_nan(val: &mut [f32], ww: usize, hh: usize) {
    for _ in 0..4 {
        let prev = val.to_vec();
        for j in 1..hh - 1 {
            for i in 1..ww - 1 {
                if prev[j * ww + i].is_nan() {
                    let nb: Vec<f32> = [prev[j * ww + i - 1], prev[j * ww + i + 1], prev[(j - 1) * ww + i], prev[(j + 1) * ww + i]]
                        .into_iter()
                        .filter(|v| !v.is_nan())
                        .collect();
                    if !nb.is_empty() {
                        val[j * ww + i] = nb.iter().sum::<f32>() / nb.len() as f32;
                    }
                }
            }
        }
    }
}

/// Catmull-Rom interpolation of a node array at fractional node coordinates.
fn cubic_at(a: &[f32], ww: usize, hh: usize, x: f32, y: f32) -> Option<f32> {
    let (ix, iy) = (x.floor() as i64, y.floor() as i64);
    if ix < 1 || iy < 1 || ix + 2 >= ww as i64 || iy + 2 >= hh as i64 {
        return None;
    }
    let (fx, fy) = (x - ix as f32, y - iy as f32);
    let k = |t: f32| -> [f32; 4] {
        [
            -0.5 * t * t * t + t * t - 0.5 * t,
            1.5 * t * t * t - 2.5 * t * t + 1.0,
            -1.5 * t * t * t + 2.0 * t * t + 0.5 * t,
            0.5 * t * t * t - 0.5 * t * t,
        ]
    };
    let (kx, ky) = (k(fx), k(fy));
    let mut v = 0.0;
    for (j, wy) in ky.iter().enumerate() {
        for (i, wx) in kx.iter().enumerate() {
            let p = a[(iy as usize + j - 1) * ww + ix as usize + i - 1];
            if p.is_nan() {
                return None;
            }
            v += p * wx * wy;
        }
    }
    Some(v)
}

/// Green by the narrow kernel where it holds nearly two samples and by the effective-sample-count scale elsewhere; red
/// and blue from the smoothed colour ratio (or difference) with the narrow-kernel value blended in where a sample sits.
fn pass_b(by: &[Vec<Smp>; 3], om: &[[f32; 3]], ww: usize, hh: usize, tw: usize, th: usize, prm: &Params) -> Vec<[f32; 3]> {
    let clamp01 = |v: f32| v.clamp(0.0, 1.0);
    let (vn, wn, w2n) = fit_scale(&by[1], om, prm.sigma_small, ww, hh, prm.ridge);
    let ms = select_scale(&by[1], om, &prm.g_scales, prm.n_g, ww, hh, prm.ridge_g);
    let mut g: Vec<f32> = (0..ww * hh)
        .map(|n| {
            let neff = if w2n[n] > 0.0 { wn[n] * wn[n] / w2n[n] } else { 0.0 };
            let a = clamp01((neff - prm.g_lo as f32) / 0.6);
            match (vn[n].is_nan(), ms[n].is_nan()) {
                (false, false) => a * vn[n] + (1.0 - a) * ms[n],
                (false, true) => vn[n],
                (true, false) => ms[n],
                _ => f32::NAN,
            }
        })
        .collect();
    fill_nan(&mut g, ww, hh);
    let mut chans: Vec<Vec<f32>> = vec![Vec::new(), g.clone(), Vec::new()];
    for col in [0usize, 2] {
        let diff: Vec<Smp> = by[col]
            .iter()
            .filter_map(|s| cubic_at(&g, ww, hh, s.x, s.y).map(|gv| Smp { x: s.x, y: s.y, v: if prm.ratio { s.v / gv.max(50.0) } else { s.v - gv } }))
            .collect();
        let (vn, wn, _) = fit_scale(&by[col], om, prm.sigma_small, ww, hh, prm.ridge);
        let d = select_scale(&diff, om, &prm.d_scales, prm.n_d, ww, hh, prm.ridge_d);
        let mut val: Vec<f32> = (0..ww * hh)
            .map(|n| {
                let cd = if d[n].is_nan() {
                    f32::NAN
                } else if prm.ratio {
                    g[n].max(50.0) * d[n]
                } else {
                    g[n] + d[n]
                };
                let a = clamp01((wn[n] - 0.3) / 0.7);
                match (vn[n].is_nan(), cd.is_nan()) {
                    (false, false) => a * vn[n] + (1.0 - a) * cd,
                    (false, true) => vn[n],
                    (true, false) => cd,
                    _ => f32::NAN,
                }
            })
            .collect();
        fill_nan(&mut val, ww, hh);
        chans[col] = val;
    }
    let mut out = vec![[0f32; 3]; tw * th];
    for j in 0..th {
        for i in 0..tw {
            let n = (j + MARGIN) * ww + i + MARGIN;
            for c in 0..3 {
                out[j * tw + i][c] = chans[c][n];
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::coarse::nominal_positions;
    use super::super::synth::{BLACK, NOISE_A, Scene, Spec, mosaics};
    use super::*;

    const W: usize = 360;
    const H: usize = 300;

    fn spec(positions: Vec<[f64; 2]>) -> Spec {
        Spec { aw: W, ah: H, frames: 16, noise_free: true, positions: Some(positions), ..Spec::default() }
    }

    fn models(pos: &[[f64; 2]]) -> Vec<Similarity> {
        pos.iter().map(|p| Similarity { t: [p[0] - pos[0][0], p[1] - pos[0][1]], theta: 0.0, scale: 0.0 }).collect()
    }

    fn deviated(a: f64) -> Vec<[f64; 2]> {
        nominal_positions()
            .iter()
            .enumerate()
            .map(|(k, n)| [n[0] + a * (((k * 5) % 7) as f64 / 6.0 - 0.5), n[1] + a * (((k * 3) % 11) as f64 / 10.0 - 0.5)])
            .collect()
    }

    fn prm() -> Params {
        Params::v4(BLACK, NOISE_A)
    }

    /// The interior box of 200 x 160 nodes.
    const BOX: (i64, i64, usize, usize) = (200, 160, 200, 160);

    fn run(s: &Spec, m: &[Similarity], gains: Option<&Gains>, p: &Params) -> Vec<[f32; 3]> {
        let mo = mosaics(s).unwrap();
        reconstruct_box(&mo, gains, m, BOX.0, BOX.1, BOX.2, BOX.3, p)
    }

    #[test]
    fn a_constant_scene_stays_constant_in_every_colour() {
        let pos = deviated(0.3);
        let s = Spec { flat: true, ..spec(pos.clone()) };
        let out = run(&s, &models(&pos), None, &prm());
        for p in &out {
            // green 2000, red 0.8 and blue 0.5 of it above black
            assert!((f64::from(p[1]) - (BLACK + 2000.0)).abs() < 0.05, "{p:?}");
            assert!((f64::from(p[0]) - (BLACK + 1600.0)).abs() < 0.5, "{p:?}");
            assert!((f64::from(p[2]) - (BLACK + 1000.0)).abs() < 0.5, "{p:?}");
        }
    }

    #[test]
    fn at_the_true_positions_green_matches_the_scene_and_beats_wrong_positions() {
        let pos = deviated(0.3);
        let s = spec(pos.clone());
        let scene = Scene::new(s.seed, s.level, s.contrast);
        let rms = |img: &[[f32; 3]]| -> f64 {
            let mut e = 0.0;
            for j in 0..BOX.3 {
                for i in 0..BOX.2 {
                    let (u, v) = ((BOX.0 as f64 + i as f64) / 2.0 + pos[0][0], (BOX.1 as f64 + j as f64) / 2.0 + pos[0][1]);
                    e += (f64::from(img[j * BOX.2 + i][1]) - BLACK - scene.green_at(u, v)).powi(2);
                }
            }
            (e / (BOX.2 * BOX.3) as f64).sqrt()
        };
        let right = rms(&run(&s, &models(&pos), None, &prm()));
        let wrong = rms(&run(&s, &models(&nominal_positions()), None, &prm()));
        assert!(right < 12.0, "true positions: rms {right}");
        assert!(wrong > 2.0 * right, "nominal positions on a deviating series: rms {wrong} against {right}");
    }

    #[test]
    fn the_result_is_finite_in_range_and_does_not_depend_on_the_threads() {
        let pos = deviated(0.3);
        let s = spec(pos.clone());
        let a = run(&s, &models(&pos), None, &prm());
        assert!(a.iter().all(|p| p.iter().all(|v| v.is_finite() && *v > 0.0 && *v < 16383.0)));
        let one = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap().install(|| run(&s, &models(&pos), None, &prm()));
        assert!(a == one, "one thread and many give the same bits");
        assert!(a == run(&s, &models(&pos), None, &prm()), "two runs give the same bits");
    }

    #[test]
    fn green_does_not_depend_on_the_chroma_setting() {
        let pos = deviated(0.3);
        let s = spec(pos.clone());
        let (a, b) = (run(&s, &models(&pos), None, &prm()), run(&s, &models(&pos), None, &Params { ratio: false, ..prm() }));
        assert!(a.iter().zip(&b).all(|(p, q)| p[1] == q[1]), "green is the same bits");
        assert!(a.iter().zip(&b).any(|(p, q)| p[0] != q[0] || p[2] != q[2]), "red and blue do follow the setting");
    }

    #[test]
    fn samples_that_sit_on_a_node_pass_through_at_nominal_positions() {
        let pos = nominal_positions();
        let s = spec(pos.clone());
        let mo = mosaics(&s).unwrap();
        let out = reconstruct_box(&mo, None, &models(&pos), BOX.0, BOX.1, BOX.2, BOX.3, &prm());
        // green nodes: the plain placement's value is the mean of the (two) samples on the node
        let mut sum = std::collections::HashMap::<(i64, i64), (f64, usize)>::new();
        for k in 0..16 {
            for y in 90..170i64 {
                for x in 105..195i64 {
                    let Some((1, v)) = mo.sample(k, x, y) else { continue };
                    let node = ((2.0 * (x as f64 + pos[k][0] - pos[0][0])).round() as i64, (2.0 * (y as f64 + pos[k][1] - pos[0][1])).round() as i64);
                    let e = sum.entry(node).or_insert((0.0, 0));
                    e.0 += f64::from(v);
                    e.1 += 1;
                }
            }
        }
        let (mut worst, mut n) = (0.0f64, 0usize);
        for ((nx, ny), (s, c)) in sum {
            let (i, j) = (nx - BOX.0, ny - BOX.1);
            if i < 8 || j < 8 || i >= BOX.2 as i64 - 8 || j >= BOX.3 as i64 - 8 || c != 2 {
                continue;
            }
            worst = worst.max((f64::from(out[j as usize * BOX.2 + i as usize][1]) - s / c as f64).abs());
            n += 1;
        }
        assert!(n > 1000, "{n}");
        assert!(worst < 6.0, "worst {worst}");
    }

    #[test]
    fn flicker_gains_put_the_frames_back_on_one_level() {
        let pos = deviated(0.3);
        let gains: Vec<f64> = (0..16).map(|k| 1.0 + 0.1 * (((k * 7) % 16) as f64 / 15.0 - 0.5)).collect();
        let flat = run(&spec(pos.clone()), &models(&pos), None, &prm());
        let s = Spec { gains: gains.clone(), ..spec(pos.clone()) };
        let uncorrected = run(&s, &models(&pos), None, &prm());
        let g = Gains { per_frame: gains.iter().map(|f| [*f; 3]).collect() };
        let corrected = run(&s, &models(&pos), Some(&g), &prm());
        let rms = |a: &[[f32; 3]]| (a.iter().zip(&flat).map(|(p, q)| f64::from(p[1] - q[1]).powi(2)).sum::<f64>() / a.len() as f64).sqrt();
        assert!(rms(&corrected) < 2.0, "corrected {}", rms(&corrected));
        assert!(rms(&uncorrected) > 5.0 * rms(&corrected).max(0.05), "uncorrected {}", rms(&uncorrected));
    }

    fn small() -> (Spec, Vec<Similarity>) {
        let pos = deviated(0.3);
        (Spec { aw: 120, ah: 90, ..spec(pos.clone()) }, models(&pos))
    }

    #[test]
    fn the_whole_image_has_the_plain_merges_geometry_and_range() {
        let (s, m) = small();
        let mo = mosaics(&s).unwrap();
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let r = reconstruct(&mo, None, &m, [2, 3, 100, 80], &prm(), &|_| {
            calls.fetch_add(1, Ordering::Relaxed);
            true
        })
        .unwrap();
        assert_eq!((r.width, r.height, r.scale, r.crop), (240, 180, 2, [4, 6, 200, 160]));
        assert_eq!(r.data.len(), 240 * 180 * 3);
        assert!(r.data.iter().all(|v| *v <= 16383));
        assert!(calls.load(Ordering::Relaxed) >= 2);
        // the same tiles as the box of the whole image give the same values
        let b = reconstruct_box(&mo, None, &m, 0, 0, 240, 180, &prm());
        for j in 0..180 {
            for i in 0..240 {
                for c in 0..3 {
                    if b[j * 240 + i][c] == 0.0 {
                        continue; // a node no sample reached: filled from a neighbour in the image
                    }
                    assert_eq!(r.data[(j * 240 + i) * 3 + c], b[j * 240 + i][c].round().clamp(0.0, 16383.0) as u16);
                }
            }
        }
    }

    #[test]
    fn cancelling_stops_the_reconstruction() {
        let (s, m) = small();
        let mo = mosaics(&s).unwrap();
        let r = reconstruct(&mo, None, &m, [0, 0, 120, 90], &prm(), &|f| f < 0.3);
        assert!(matches!(r, Err(MergeError::Cancelled)));
    }

    #[test]
    fn hostile_input_does_not_panic() {
        let (s, _) = small();
        let mo = mosaics(&s).unwrap();
        // no models: every frame at zero displacement; absurd models: far outside the frame
        let none = reconstruct_box(&mo, None, &[], 10, 10, 40, 40, &prm());
        assert_eq!(none.len(), 1600);
        let far = vec![Similarity { t: [1e6, -1e6], theta: 1e3, scale: -3.0 }; 16];
        let out = reconstruct_box(&mo, None, &far, 10, 10, 40, 40, &prm());
        assert!(out.iter().all(|p| p.iter().all(|v| v.is_finite())));
        let empty = reconstruct_box(&mo, None, &[], 10, 10, 0, 0, &prm());
        assert!(empty.is_empty());
    }
}
