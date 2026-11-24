//! Frame positions of one tile: every frame is registered against a dense reference built from the other frames.
//!
//! Why a dense reference: two green planes are both aliased quincunx lattices, and comparing aliased content between
//! two frames gives a shift-dependent bias that no prefilter removes. The reference is the other frames' samples
//! placed on a grid twice as fine as the sensor with a Gaussian-weighted local linear regression (Takeda, Farsiu and
//! Milanfar, "Kernel regression for image processing and reconstruction", IEEE Trans. Image Processing 16(2), 2007),
//! so "frame sample = scene at site + position" holds without aliasing mismatch. A frame is fitted to the cubic
//! (Keys, IEEE Trans. Acoustics, Speech, Signal Processing 29(6), 1981) interpolation of that reference by
//! Lucas–Kanade iterations with gain and offset free (Lucas and Kanade, IJCAI 1981; Baker and Matthews, Int. J.
//! Computer Vision 56(3), 2004). The frames' positions are refined round by round, the reference rebuilt at the
//! current positions each time; only differences between frames are observable, so the mean update is removed.

use super::fft::hann;
use super::series::GreenSeries;
use super::tiles::{PLANE, REF_HALF, Tile};
use rayon::prelude::*;

const B: i64 = REF_HALF;
/// Side of the reference grid in nodes (the reference region of ±`B` sites at two nodes per site, plus a margin).
const GRID: usize = (4 * REF_HALF + 20) as usize;
/// Offsets from the grid origin, in nodes.
const ORIGIN_MARGIN: i64 = 8;
/// Largest iteration count of one Lucas–Kanade fit.
const LK_ITER: usize = 30;
/// A round whose largest update is below this ends the refinement.
const STOP_UPDATE: f64 = 2e-4;
/// A tile counts as converged when the largest update of its last round is at most this (sites).
pub const CONVERGED: f64 = 0.05;
/// Early stop: from the third round on, a tile whose update is above [`CONVERGED`] and above this value, or above
/// [`EARLY_RATIO`] times the update before, is given up. The ratio was chosen on the development series: 0.7 gave up two tiles of the noisy ISO 3200 series that would have converged; 0.85 gives up none of them and saves a quarter of its measurement time.
pub const EARLY_ABS: f64 = 0.2;
pub const EARLY_RATIO: f64 = 0.85;

/// Thresholds of the early stop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EarlyStop {
    pub abs: f64,
    pub ratio: f64,
}

impl Default for EarlyStop {
    fn default() -> EarlyStop {
        EarlyStop { abs: EARLY_ABS, ratio: EARLY_RATIO }
    }
}

type Mom = [f32; 9];

/// Reference kernel and capture range of a measurement.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Kernel {
    /// Gaussian scale of the reference (sites).
    pub sigma: f64,
    /// Smallest kernel weight at a node for the reference to be used there.
    pub wmin: f64,
    /// Largest offset (sites) a Lucas–Kanade fit may reach before it is rejected.
    pub lk_limit: f64,
}

impl Kernel {
    /// Fewer frames sample the scene more sparsely, so the kernel is wider.
    pub fn normal(frames: usize) -> Kernel {
        let sigma = match frames {
            0..=5 => 0.9,
            6..=11 => 0.65,
            _ => 0.45,
        };
        Kernel { sigma, wmin: wmin(frames, sigma), lk_limit: 1.0 }
    }

    /// The wide first stage of the coarse start: a blurred reference and a larger capture range.
    pub fn wide(frames: usize) -> Kernel {
        Kernel { sigma: 1.2, wmin: wmin(frames, 1.2), lk_limit: 3.0 }
    }
}

fn wmin(frames: usize, sigma: f64) -> f64 {
    0.133 * frames.saturating_sub(1) as f64 * (sigma / 0.45).powi(2)
}

/// Result of measuring one tile.
#[derive(Debug, Clone, PartialEq)]
pub struct TileFit {
    /// Position of each frame (sites, absolute, only differences are meaningful); `None` for a frame that could
    /// not be fitted in some round.
    pub positions: Vec<Option<[f64; 2]>>,
    /// Largest update of any frame in the last round (sites); infinite if no round ran.
    pub last_update: f64,
    pub rounds_run: usize,
    pub stopped_early: bool,
}

impl TileFit {
    fn failed(n: usize) -> TileFit {
        TileFit { positions: vec![None; n], last_update: f64::INFINITY, rounds_run: 0, stopped_early: false }
    }

    /// Every frame measured.
    pub fn complete(&self) -> bool {
        !self.positions.is_empty() && self.positions.iter().all(Option::is_some)
    }

    /// Complete and settled: the last update is at most [`CONVERGED`].
    pub fn converged(&self) -> bool {
        self.complete() && self.last_update <= CONVERGED
    }

    /// Positions relative to frame 0, if every frame was measured.
    pub fn relative(&self) -> Option<Vec<[f64; 2]>> {
        let all: Option<Vec<[f64; 2]>> = self.positions.iter().copied().collect();
        let all = all?;
        let first = *all.first()?;
        Some(all.iter().map(|p| [p[0] - first[0], p[1] - first[1]]).collect())
    }
}

fn keys(t: f64) -> f64 {
    let a = -0.5;
    let t = t.abs();
    if t < 1.0 {
        (a + 2.0) * t * t * t - (a + 3.0) * t * t + 1.0
    } else if t < 2.0 {
        a * t * t * t - 5.0 * a * t * t + 8.0 * a * t - 4.0 * a
    } else {
        0.0
    }
}

fn dkeys(t: f64) -> f64 {
    let a = -0.5;
    let sg = t.signum();
    let t = t.abs();
    sg * if t < 1.0 {
        3.0 * (a + 2.0) * t * t - 2.0 * (a + 3.0) * t
    } else if t < 2.0 {
        3.0 * a * t * t - 10.0 * a * t + 8.0 * a
    } else {
        0.0
    }
}

/// Cubic value and gradient (per node) of `a` at a fractional node position; `None` if a tap is missing or not finite.
fn bicubic(a: &[f32], w: usize, h: usize, x: f64, y: f64) -> Option<(f64, f64, f64)> {
    let (ix, iy) = (x.floor() as i64, y.floor() as i64);
    if ix < 1 || iy < 1 || ix + 2 >= w as i64 || iy + 2 >= h as i64 {
        return None;
    }
    let (mut v, mut gx, mut gy) = (0.0, 0.0, 0.0);
    for j in -1..=2i64 {
        let ty = y - (iy + j) as f64;
        let (wy, dwy) = (keys(ty), dkeys(ty));
        for i in -1..=2i64 {
            let tx = x - (ix + i) as f64;
            let p = *a.get((iy + j) as usize * w + (ix + i) as usize)?;
            if p.is_nan() {
                return None;
            }
            let p = f64::from(p);
            let (wx, dwx) = (keys(tx), dkeys(tx));
            v += p * wx * wy;
            gx += p * dwx * wy;
            gy += p * wx * dwy;
        }
    }
    Some((v, gx, gy))
}

#[allow(clippy::too_many_arguments)]
/// One sample of weight `sign` at the sensor position `(px, py)` added to the moment sums of the nodes around it.
/// Per node: `[S, Sx, Sy, W, Wx, Wy, Wxx, Wxy, Wyy]` with `(dx, dy)` the sample's offset from the node in sites.
fn splat(m: &mut [Mom], x0: i64, y0: i64, px: f64, py: f64, v: f64, sign: f32, inv2s2: f64) {
    let (fx, fy) = (2.0 * px - x0 as f64, 2.0 * py - y0 as f64);
    let (ix0, iy0) = (fx.floor() as i64, fy.floor() as i64);
    for iy in iy0 - 2..=iy0 + 3 {
        if iy < 0 || iy >= GRID as i64 {
            continue;
        }
        let dy = (fy - iy as f64) * 0.5;
        for ix in ix0 - 2..=ix0 + 3 {
            if ix < 0 || ix >= GRID as i64 {
                continue;
            }
            let dx = (fx - ix as f64) * 0.5;
            let w = (-(dx * dx + dy * dy) * inv2s2).exp() * f64::from(sign);
            if let Some(c) = m.get_mut(iy as usize * GRID + ix as usize) {
                c[0] += (w * v) as f32;
                c[1] += (w * v * dx) as f32;
                c[2] += (w * v * dy) as f32;
                c[3] += w as f32;
                c[4] += (w * dx) as f32;
                c[5] += (w * dy) as f32;
                c[6] += (w * dx * dx) as f32;
                c[7] += (w * dx * dy) as f32;
                c[8] += (w * dy * dy) as f32;
            }
        }
    }
}

/// The local-regression value at the node (NaN where the fit is not well determined).
fn node_value(c: &Mom, wmin: f64) -> f32 {
    let (s, sx, sy) = (f64::from(c[0]), f64::from(c[1]), f64::from(c[2]));
    let (w, wx, wy, wxx, wxy, wyy) = (f64::from(c[3]), f64::from(c[4]), f64::from(c[5]), f64::from(c[6]), f64::from(c[7]), f64::from(c[8]));
    if w < wmin {
        return f32::NAN;
    }
    // Cramer's rule for the constant term of [[w,wx,wy],[wx,wxx,wxy],[wy,wxy,wyy]] [c0,a,b] = [S,Sx,Sy]
    let det = w * (wxx * wyy - wxy * wxy) - wx * (wx * wyy - wxy * wy) + wy * (wx * wxy - wxx * wy);
    if det <= 1e-6 * w * (wxx * wyy).max(1e-30) {
        return f32::NAN;
    }
    let num = s * (wxx * wyy - wxy * wxy) - wx * (sx * wyy - wxy * sy) + wy * (sx * wxy - wxx * sy);
    (num / det) as f32
}

/// The sites of the reference region (green sites, camera-filled rows left out).
fn region_sites(s: &GreenSeries, xc: i64, yc: i64) -> Vec<(i64, i64)> {
    let mut v = Vec::new();
    for y in yc - B..yc + B {
        if s.masked_row(y) {
            continue;
        }
        let mut x = xc - B;
        if ((x + y).rem_euclid(2) as usize) != s.gpar {
            x += 1;
        }
        while x < xc + B {
            v.push((x, y));
            x += 2;
        }
    }
    v
}

/// Lucas–Kanade fit of one frame's window samples `(value, node x, node y, weight)` to the reference `arr`: the offset
/// `(dx, dy)` in sites and the weighted residual rms; `None` if the fit leaves the capture range, is singular or lacks coverage.
fn lk(arr: &[f32], sites: &[(f64, f64, f64, f64)], limit: f64) -> Option<([f64; 2], f64)> {
    let mut e = [0.0f64; 2];
    let mut rms = 0.0;
    for _ in 0..LK_ITER {
        let samples: Vec<Option<(f64, f64, f64)>> = sites.iter().map(|s| bicubic(arr, GRID, GRID, s.1 + 2.0 * e[0], s.2 + 2.0 * e[1])).collect();
        let (mut sw, mut sr, mut sf, mut srr, mut srf) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for (s, p) in sites.iter().zip(&samples) {
            if let Some(p) = p {
                sw += s.3;
                sr += s.3 * p.0;
                sf += s.3 * s.0;
                srr += s.3 * p.0 * p.0;
                srf += s.3 * p.0 * s.0;
            }
        }
        if sw < 1000.0 {
            return None;
        }
        let den = sw * srr - sr * sr;
        let gain = (sw * srf - sr * sf) / den;
        let o = (sf - gain * sr) / sw;
        let (mut m00, mut m01, mut m11, mut r0, mut r1, mut ss) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        for (s, p) in sites.iter().zip(&samples) {
            if let Some(p) = p {
                let err = s.0 - gain * p.0 - o;
                let (jx, jy) = (2.0 * gain * p.1, 2.0 * gain * p.2);
                m00 += s.3 * jx * jx;
                m01 += s.3 * jx * jy;
                m11 += s.3 * jy * jy;
                r0 += s.3 * jx * err;
                r1 += s.3 * jy * err;
                ss += s.3 * err * err;
            }
        }
        rms = (ss / sw).sqrt();
        let det = m00 * m11 - m01 * m01;
        if !det.is_finite() || det.abs() < 1e-12 {
            return None;
        }
        let d = [(m11 * r0 - m01 * r1) / det, (m00 * r1 - m01 * r0) / det];
        if !d[0].is_finite() || !d[1].is_finite() {
            return None;
        }
        e[0] += d[0];
        e[1] += d[1];
        if d[0].abs().max(d[1].abs()) < 1e-5 {
            break;
        }
        if e[0].abs() > limit || e[1].abs() > limit {
            return None;
        }
    }
    Some((e, rms))
}

/// One round: the reference from all frames at positions `p` (sites), then each frame against the reference of the
/// other frames. Returns each frame's remaining offset and residual rms; `None` if a sample is missing.
fn round(s: &GreenSeries, tile: &Tile, p: &[[f64; 2]], kernel: &Kernel) -> Option<Vec<Option<([f64; 2], f64)>>> {
    let (xc, yc) = (tile.xc, tile.yc);
    let (x0, y0) = (2 * (xc - B) - ORIGIN_MARGIN, 2 * (yc - B) - ORIGIN_MARGIN);
    let inv2s2 = 1.0 / (2.0 * kernel.sigma * kernel.sigma);
    let st = region_sites(s, xc, yc);
    let mut vals: Vec<Vec<f64>> = Vec::with_capacity(p.len());
    for k in 0..p.len() {
        let mut v = Vec::with_capacity(st.len());
        for &(x, y) in &st {
            v.push(s.g(k, x, y)?);
        }
        vals.push(v);
    }
    let mut mo: Vec<Mom> = vec![[0f32; 9]; GRID * GRID];
    for (k, pk) in p.iter().enumerate() {
        for (i, &(x, y)) in st.iter().enumerate() {
            splat(&mut mo, x0, y0, x as f64 + pk[0], y as f64 + pk[1], vals[k][i], 1.0, inv2s2);
        }
    }
    let win = hann(PLANE);
    let h2 = (PLANE / 2) as i64;
    let out: Vec<Option<([f64; 2], f64)>> = (0..p.len())
        .into_par_iter()
        .map(|k| {
            let mut mk = mo.clone();
            for (i, &(x, y)) in st.iter().enumerate() {
                splat(&mut mk, x0, y0, x as f64 + p[k][0], y as f64 + p[k][1], vals[k][i], -1.0, inv2s2);
            }
            let arr: Vec<f32> = mk.iter().map(|c| node_value(c, kernel.wmin)).collect();
            let mut win_sites: Vec<(f64, f64, f64, f64)> = Vec::with_capacity(PLANE * PLANE);
            for j in 0..PLANE {
                for i in 0..PLANE {
                    let (u, v) = (i as i64 - h2, j as i64 - h2);
                    let (x, y) = (xc + u + v, yc + u - v);
                    if s.masked_row(y) {
                        continue;
                    }
                    let val = s.g(k, x, y)?;
                    win_sites.push((val, 2.0 * (x as f64 + p[k][0]) - x0 as f64, 2.0 * (y as f64 + p[k][1]) - y0 as f64, win[i] * win[j]));
                }
            }
            lk(&arr, &win_sites, kernel.lk_limit)
        })
        .collect();
    Some(out)
}

/// Frame positions of one tile: start from `init` (sites, absolute) and refine for at most `rounds` rounds. With
/// `early_stop` a tile that is clearly not settling after the third round is given up (its `last_update` is then
/// above [`CONVERGED`]); converged tiles are not affected.
pub fn measure_tile(s: &GreenSeries, tile: &Tile, init: &[[f64; 2]], rounds: usize, kernel: &Kernel, early_stop: Option<EarlyStop>) -> TileFit {
    let n = init.len();
    if n < 2 || n != s.frame_count() || init.iter().any(|p| !p[0].is_finite() || !p[1].is_finite()) {
        return TileFit::failed(n);
    }
    let mut p: Vec<[f64; 2]> = init.to_vec();
    let mut ok = vec![true; n];
    let (mut last, mut prev, mut run, mut early) = (f64::INFINITY, f64::INFINITY, 0usize, false);
    for it in 0..rounds {
        let Some(r) = round(s, tile, &p, kernel) else {
            return TileFit::failed(n);
        };
        let (mut mean, mut cnt) = ([0.0; 2], 0.0);
        for (k, rk) in r.iter().enumerate() {
            match rk {
                Some((e, _)) => {
                    mean[0] += e[0];
                    mean[1] += e[1];
                    cnt += 1.0;
                }
                None => {
                    if let Some(f) = ok.get_mut(k) {
                        *f = false;
                    }
                }
            }
        }
        if cnt == 0.0 {
            break;
        }
        mean = [mean[0] / cnt, mean[1] / cnt];
        let mut worst: f64 = 0.0;
        for (k, rk) in r.iter().enumerate() {
            if let (Some((e, _)), Some(pk)) = (rk, p.get_mut(k)) {
                pk[0] += e[0] - mean[0];
                pk[1] += e[1] - mean[1];
                worst = worst.max((e[0] - mean[0]).abs()).max((e[1] - mean[1]).abs());
            }
        }
        last = worst;
        run = it + 1;
        if worst < STOP_UPDATE {
            break;
        }
        if let Some(es) = early_stop
            && it >= 2
            && worst > CONVERGED
            && (worst > es.abs || worst > es.ratio * prev)
        {
            early = true;
            break;
        }
        prev = worst;
    }
    TileFit {
        positions: (0..n).map(|k| if ok.get(k).copied().unwrap_or(false) { p.get(k).copied() } else { None }).collect(),
        last_update: last,
        rounds_run: run,
        stopped_early: early,
    }
}

/// Measures tiles `par` at a time; `inits[i]` is the start of `tiles[i]`.
pub fn measure_tiles(
    s: &GreenSeries,
    tiles: &[Tile],
    inits: &[Vec<[f64; 2]>],
    rounds: usize,
    kernel: &Kernel,
    early_stop: Option<EarlyStop>,
    par: usize,
) -> Vec<TileFit> {
    let mut out = Vec::with_capacity(tiles.len());
    let par = par.max(1);
    for (ct, ci) in tiles.chunks(par).zip(inits.chunks(par)) {
        let part: Vec<TileFit> = ct.par_iter().zip(ci.par_iter()).map(|(t, i)| measure_tile(s, t, i, rounds, kernel, early_stop)).collect();
        out.extend(part);
    }
    out
}
