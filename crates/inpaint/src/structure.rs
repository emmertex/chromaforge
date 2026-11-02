//! Structure pass: straight edges and lines that cross the hole.
//!
//! Patch-based completion copies texture well but, left alone, often breaks a straight structure
//! that runs through the hole (a horizon, a door frame, a power line): nothing in the patch distance
//! says *where* inside the hole the structure has to continue. This module finds such structures
//! and turns them into guide channels for the patch distance, in the spirit of guided synthesis
//! (A. Hertzmann et al., *Image Analogies*, SIGGRAPH 2001) and of structure propagation along a
//! curve (J. Sun, L. Yuan, J. Jia, H.-Y. Shum, *Image Completion with Structure Propagation*,
//! SIGGRAPH 2005), with the line found automatically instead of drawn by the user:
//!
//! 1. **Detection.** On a ring of known pixels around the hole, Sobel gradients of a perceptual
//!    luminance vote into a Hough accumulator (orientation × offset, each pixel voting only near
//!    its own gradient orientation, weighted by magnitude: R. Duda, P. Hart, *Use of the Hough
//!    Transformation to Detect Lines and Curves in Pictures*, CACM 1972; orientation-restricted
//!    voting as in D. H. Ballard 1981). A peak is a candidate line.
//! 2. **Pairing.** A candidate is kept only when it crosses the hole and is *supported on both
//!    sides*: along the line, at least 60 % of the ring on the entry side and on the exit side holds
//!    an edge pixel of matching orientation. That pairs an edge entering the hole with one leaving it,
//!    and rejects texture. The line is then refit to its support pixels (weighted principal axis), so
//!    its angle is precise over the whole span, then turned so that the averaged cross-sections
//!    on the two sides line up (sub-pixel), and accepted only if those cross-sections stand out of
//!    the texture and correlate.
//! 3. **Guidance.** Each kept line contributes one channel: the signed distance to the line, ramped
//!    to zero beyond two bands, within the line's supported span (each pixel only for its nearest
//!    line). It is known everywhere, inside the hole too, so a target patch on the line's
//!    continuation can only match source patches at the same offset from the line: the structure
//!    is carried across. Away from lines the channel is zero and texture matching is unchanged.
//!    PatchMatch also tries, for each target near a line, the source at the line's common shift in
//!    both directions (the whole band copied as one block, as a structure-propagation step would),
//!    else the nearest valid source along the line.
//!
//! Lines are found once (on the finest pyramid level where the hole spans at most 256 px) and
//! their guide channels are evaluated in each pyramid level's coordinates.

use crate::mask::distance_sq;

/// A straight structure crossing the hole (full-resolution pixel coordinates, pixel centres at
/// integers).
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// Unit normal; points `p` on the line satisfy `n · p = rho`.
    pub n: (f32, f32),
    pub rho: f32,
    /// Supported span along the line direction `u = (-n.1, n.0)`: `u · p` in `t0..=t1`.
    pub t0: f32,
    pub t1: f32,
    /// Edge contrast (linear units, about the step across the edge).
    pub contrast: f32,
}

impl Line {
    #[inline]
    pub fn u(&self) -> (f32, f32) {
        (-self.n.1, self.n.0)
    }

    /// The same line found on a pyramid level `scale` times smaller (whose pixel `(x, y)` covers
    /// full-resolution centre `(x·scale + (scale-1)/2, …)`), in full-resolution coordinates.
    pub fn to_full_res(&self, scale: f32) -> Line {
        let off = (scale - 1.0) * 0.5;
        let u = self.u();
        let (dn, du) = (off * (self.n.0 + self.n.1), off * (u.0 + u.1));
        Line { n: self.n, rho: self.rho * scale + dn, t0: self.t0 * scale + du, t1: self.t1 * scale + du, contrast: self.contrast }
    }
}

/// Hough orientation bins over `[0, π)`.
const N_THETA: usize = 180;
/// Orientation tolerance for a support pixel.
const ANGLE_TOL: f32 = 12.0 * std::f32::consts::PI / 180.0;
/// Fraction of each side's ring length that must carry support.
const MIN_COVERAGE: f32 = 0.6;
/// At most this many lines are kept.
pub const MAX_LINES: usize = 4;

/// An edge pixel on the ring around the hole.
struct Edge {
    x: f32,
    y: f32,
    /// Gradient magnitude (perceptual luminance).
    m: f32,
    /// Gradient orientation folded into `[0, π)` (the line's normal direction).
    theta: f32,
    /// Linear-light gradient magnitude over all channels.
    lin: f32,
}

fn angle_diff(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(std::f32::consts::PI);
    d.min(std::f32::consts::PI - d)
}

/// Find straight structures that cross `hole` (see module docs). `exclude`d pixels are not looked
/// at. Returns at most [`MAX_LINES`] lines, strongest first; empty on size mismatches.
pub fn detect_lines(
    w: usize,
    h: usize,
    ch: usize,
    img: &[f32],
    hole: &[bool],
    exclude: Option<&[bool]>,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Vec<Line> {
    let n = match w.checked_mul(h) {
        Some(n) if n > 0 && ch > 0 && hole.len() == n && Some(img.len()) == n.checked_mul(ch) => n,
        _ => return Vec::new(),
    };
    let ex = |i: usize| exclude.and_then(|e| e.get(i)).copied().unwrap_or(false);
    let blocked = |i: usize| hole.get(i).copied().unwrap_or(true) || ex(i);
    // Hole extent → ring width.
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for (i, _) in hole.iter().enumerate().filter(|(_, h)| **h) {
        let (x, y) = (i % w, i / w);
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
    }
    if x1 <= x0 || y1 <= y0 {
        return Vec::new();
    }
    let ext = (x1 - x0).max(y1 - y0);
    let band = (ext as f32 / 3.0).clamp(6.0, 64.0);
    let Some(dist) = distance_sq(w, h, hole) else { return Vec::new() };
    let ring: Vec<bool> = (0..n).map(|i| !blocked(i) && dist.get(i).is_some_and(|d| *d <= band * band)).collect();
    let lum: Vec<f32> = img.chunks_exact(ch).map(|p| (p.iter().sum::<f32>() / ch as f32).max(0.0).powf(0.45)).collect();
    // Edge pixels: Sobel on the ring where the 3×3 neighbourhood is fully known.
    let mut edges: Vec<Edge> = Vec::new();
    let mut mags: Vec<f32> = Vec::new();
    for y in 1..h.saturating_sub(1) {
        if y % 64 == 0 && cancelled() {
            return Vec::new();
        }
        for x in 1..w - 1 {
            let i = y * w + x;
            if !ring[i] || [i - w - 1, i - w, i - w + 1, i - 1, i + 1, i + w - 1, i + w, i + w + 1].iter().any(|j| blocked(*j)) {
                continue;
            }
            let l = |j: usize| lum.get(j).copied().unwrap_or(0.0);
            let gx = (l(i - w + 1) + 2.0 * l(i + 1) + l(i + w + 1) - l(i - w - 1) - 2.0 * l(i - 1) - l(i + w - 1)) / 8.0;
            let gy = (l(i + w - 1) + 2.0 * l(i + w) + l(i + w + 1) - l(i - w - 1) - 2.0 * l(i - w) - l(i - w + 1)) / 8.0;
            let m = gx.hypot(gy);
            if !m.is_finite() {
                continue;
            }
            mags.push(m);
            let mut lin = 0.0f32;
            for c in 0..ch {
                let v = |j: usize| img.get(j * ch + c).copied().unwrap_or(0.0);
                let (dx, dy) = ((v(i + 1) - v(i - 1)) * 0.5, (v(i + w) - v(i - w)) * 0.5);
                lin += dx * dx + dy * dy;
            }
            edges.push(Edge { x: x as f32, y: y as f32, m, theta: gy.atan2(gx).rem_euclid(std::f32::consts::PI), lin: lin.sqrt() });
        }
    }
    if mags.is_empty() {
        return Vec::new();
    }
    let k = mags.len() / 2;
    let (_, med, _) = mags.select_nth_unstable_by(k, f32::total_cmp);
    let tau = (2.5 * *med).max(0.01);
    edges.retain(|e| e.m > tau && e.lin.is_finite());
    if edges.len() < 8 {
        return Vec::new();
    }
    // Hough accumulator: ρ = x cos θ + y sin θ, offset by `rmax`, 1 px bins.
    let rmax = (w + h) as i32;
    let n_rho = (2 * rmax + 1) as usize;
    let mut acc = vec![0.0f32; N_THETA * n_rho];
    let trig: Vec<(f32, f32)> = (0..N_THETA).map(|b| (b as f32 * std::f32::consts::PI / N_THETA as f32).sin_cos()).map(|(s, c)| (c, s)).collect();
    for e in &edges {
        let b = ((e.theta / std::f32::consts::PI * N_THETA as f32).round() as i32).rem_euclid(N_THETA as i32);
        for db in -2..=2 {
            let tb = (b + db).rem_euclid(N_THETA as i32) as usize;
            let Some(&(c, s)) = trig.get(tb) else { continue };
            let r = (e.x * c + e.y * s).round() as i32 + rmax;
            if let Some(a) = acc.get_mut(tb * n_rho + r.clamp(0, 2 * rmax) as usize) {
                *a += e.m;
            }
        }
    }
    let mut lines: Vec<(f32, Line)> = Vec::new();
    let mut used = vec![false; edges.len()];
    for _attempt in 0..16 {
        if lines.len() >= MAX_LINES || cancelled() {
            break;
        }
        let Some((best, &val)) = acc.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1).then(b.0.cmp(&a.0))) else { break };
        if val <= 0.0 {
            break;
        }
        let (tb, rb) = (best / n_rho, (best % n_rho) as i32 - rmax);
        // Suppress the neighbourhood of this peak.
        for dt in -4i32..=4 {
            let t = (tb as i32 + dt).rem_euclid(N_THETA as i32) as usize;
            // Across the θ wrap the same line has the opposite ρ.
            let wrapped = (tb as i32 + dt) < 0 || (tb as i32 + dt) >= N_THETA as i32;
            let rc = if wrapped { -rb } else { rb };
            for dr in -6..=6 {
                let r = (rc + dr + rmax).clamp(0, 2 * rmax) as usize;
                if let Some(a) = acc.get_mut(t * n_rho + r) {
                    *a = 0.0;
                }
            }
        }
        let theta = tb as f32 * std::f32::consts::PI / N_THETA as f32;
        let ctx = Ctx { w, h, hole, ring: &ring, lum: &lum, edges: &edges, used: &used };
        let Some((line, sup)) = validate(&ctx, theta, rb as f32) else { continue };
        // Its edge pixels belong to this line now: no second, slightly rotated copy of it.
        for i in sup {
            if let Some(u) = used.get_mut(i) {
                *u = true;
            }
        }
        lines.push((val, line));
    }
    lines.sort_by(|a, b| b.0.total_cmp(&a.0));
    lines.into_iter().map(|(_, l)| l).collect()
}

/// What [`support`] and [`validate`] look at.
struct Ctx<'a> {
    w: usize,
    h: usize,
    hole: &'a [bool],
    ring: &'a [bool],
    lum: &'a [f32],
    edges: &'a [Edge],
    /// Edge pixels already claimed by an accepted line.
    used: &'a [bool],
}

impl Ctx<'_> {
    /// Grid index of the pixel nearest to `(x, y)`.
    fn at(&self, x: f32, y: f32) -> Option<usize> {
        let (xr, yr) = (x.round(), y.round());
        (xr >= 0.0 && yr >= 0.0 && (xr as usize) < self.w && (yr as usize) < self.h).then(|| yr as usize * self.w + xr as usize)
    }
}

/// Where the line `(n, rho)` crosses the hole: the `u · p` range of its hole samples.
fn chord(cx: &Ctx, n: (f32, f32), rho: f32) -> Option<(f32, f32)> {
    let u = (-n.1, n.0);
    let tmax = (cx.w + cx.h) as f32;
    let (mut t_in, mut t_out) = (f32::INFINITY, f32::NEG_INFINITY);
    let mut t = -tmax;
    while t <= tmax {
        if cx.at(n.0 * rho + u.0 * t, n.1 * rho + u.1 * t).and_then(|i| cx.hole.get(i)).copied().unwrap_or(false) {
            t_in = t_in.min(t);
            t_out = t_out.max(t);
        }
        t += 0.5;
    }
    t_in.is_finite().then_some((t_in, t_out))
}

/// Support of the line `(n, rho)`: indices of unclaimed edge pixels within `tol` px whose orientation
/// matches, on the entry side (`t < t_in`) or exit side (`t > t_out`) of the hole chord. `None` when
/// the line misses the hole or either side's coverage is too low.
fn support(cx: &Ctx, n: (f32, f32), rho: f32, tol: f32) -> Option<Vec<usize>> {
    let u = (-n.1, n.0);
    let theta = n.1.atan2(n.0).rem_euclid(std::f32::consts::PI);
    let tmax = (cx.w + cx.h) as f32;
    let (t_in, t_out) = chord(cx, n, rho)?;
    let nb = (2.0 * tmax) as usize + 2;
    let bin = |t: f32| ((t + tmax).round().max(0.0) as usize).min(nb - 1);
    let mut ring_bins = vec![false; nb];
    let mut t = -tmax;
    while t <= tmax {
        if cx.at(n.0 * rho + u.0 * t, n.1 * rho + u.1 * t).and_then(|i| cx.ring.get(i)).copied().unwrap_or(false) {
            ring_bins[bin(t)] = true;
        }
        t += 1.0;
    }
    let mut sup_bins = vec![false; nb];
    let mut sup = Vec::new();
    for (k, e) in cx.edges.iter().enumerate() {
        if cx.used.get(k).copied().unwrap_or(true) {
            continue;
        }
        let d = e.x * n.0 + e.y * n.1 - rho;
        if d.abs() > tol || angle_diff(e.theta, theta) > ANGLE_TOL {
            continue;
        }
        let t = e.x * u.0 + e.y * u.1;
        if t > t_in && t < t_out {
            continue;
        }
        sup_bins[bin(t)] = true;
        sup.push(k);
    }
    let (bi, bo) = (bin(t_in), bin(t_out));
    let side = |r: std::ops::Range<usize>| -> (usize, usize) {
        let (mut ring_n, mut sup_n) = (0usize, 0usize);
        for b in r {
            if ring_bins.get(b).copied().unwrap_or(false) {
                ring_n += 1;
                // Support within one bin counts (the ring sample and the edge pixel can round apart).
                if (b.saturating_sub(1)..=b + 1).any(|k| sup_bins.get(k).copied().unwrap_or(false)) {
                    sup_n += 1;
                }
            }
        }
        (ring_n, sup_n)
    };
    let (ra, sa) = side(0..bi);
    let (rb, sb) = side(bo + 1..nb);
    let ok = |r: usize, s: usize| r >= 4 && s as f32 >= MIN_COVERAGE * r as f32;
    (ok(ra, sa) && ok(rb, sb)).then_some(sup)
}

/// Half-width (px) of the cross-section profiles [`profiles`] measures.
const PROFILE_R: f32 = 4.0;
/// Profile bin width (px).
const PROFILE_BIN: f32 = 0.5;

/// Cross-section of the line on each side of the hole: mean luminance of ring pixels by their
/// signed distance to the line (bins of [`PROFILE_BIN`] over `±PROFILE_R`), averaged along the
/// ring. Returns per side the profile, its noise (standard error averaged over bins) and the mean
/// `t` of the samples. `None` when a side has empty bins or the line misses the hole.
fn profiles(cx: &Ctx, n: (f32, f32), rho: f32) -> Option<[(Vec<f64>, f64, f32); 2]> {
    let (t_in, t_out) = chord(cx, n, rho)?;
    let u = (-n.1, n.0);
    let tmax = (cx.w + cx.h) as f32;
    let nbin = (2.0 * PROFILE_R / PROFILE_BIN) as usize + 1;
    // [side][bin] → (sum, sum², count); [side] → (Σt, count)
    let mut acc = [vec![(0.0f64, 0.0f64, 0u32); nbin], vec![(0.0f64, 0.0f64, 0u32); nbin]];
    let mut tacc = [(0.0f64, 0u32); 2];
    let mut t = -tmax;
    while t <= tmax {
        let side = if t < t_in - 1.0 {
            0
        } else if t > t_out + 1.0 {
            1
        } else {
            t += 0.5;
            continue;
        };
        let mut d = -PROFILE_R - 1.0;
        while d <= PROFILE_R + 1.0 {
            let (x, y) = (n.0 * (rho + d) + u.0 * t, n.1 * (rho + d) + u.1 * t);
            d += 0.5;
            let Some(i) = cx.at(x, y) else { continue };
            if !cx.ring.get(i).copied().unwrap_or(false) {
                continue;
            }
            // Bin by the pixel centre's own distance (sub-pixel profile).
            let (px, py) = ((i % cx.w) as f32, (i / cx.w) as f32);
            let dd = px * n.0 + py * n.1 - rho;
            let b = ((dd + PROFILE_R) / PROFILE_BIN).round();
            if b < 0.0 || b >= nbin as f32 {
                continue;
            }
            let v = f64::from(cx.lum.get(i).copied().unwrap_or(0.0));
            if let Some(a) = acc.get_mut(side).and_then(|s| s.get_mut(b as usize)) {
                *a = (a.0 + v, a.1 + v * v, a.2 + 1);
            }
            if let Some(ta) = tacc.get_mut(side) {
                *ta = (ta.0 + f64::from(px * u.0 + py * u.1), ta.1 + 1);
            }
        }
        t += 0.5;
    }
    let mut out: [(Vec<f64>, f64, f32); 2] = [(vec![0.0; nbin], 0.0, 0.0), (vec![0.0; nbin], 0.0, 0.0)];
    for (s, o) in out.iter_mut().enumerate() {
        let mut noise2 = 0.0f64;
        for (b, p) in o.0.iter_mut().enumerate() {
            let (sum, sq, c) = acc[s][b];
            if c < 3 {
                return None;
            }
            let c = f64::from(c);
            *p = sum / c;
            noise2 += ((sq / c - *p * *p).max(0.0) / c) / nbin as f64;
        }
        o.1 = noise2.sqrt();
        o.2 = (tacc[s].0 / f64::from(tacc[s].1.max(1))) as f32;
    }
    Some(out)
}

/// Correlation of the de-meaned profiles `a` and `b` shifted by `s` bins (`b[i + s]` against
/// `a[i]`), over the overlap.
fn shifted_corr(a: &[f64], b: &[f64], s: i32) -> f64 {
    let pairs: Vec<(f64, f64)> = (0..a.len() as i32).filter_map(|i| Some((*a.get(i as usize)?, *b.get(usize::try_from(i + s).ok()?)?))).collect();
    if pairs.len() < 5 {
        return f64::NEG_INFINITY;
    }
    let k = pairs.len() as f64;
    let (ma, mb) = (pairs.iter().map(|p| p.0).sum::<f64>() / k, pairs.iter().map(|p| p.1).sum::<f64>() / k);
    let (mut ab, mut aa, mut bb) = (0.0f64, 0.0f64, 0.0f64);
    for (a, b) in pairs {
        let (a, b) = (a - ma, b - mb);
        ab += a * b;
        aa += a * a;
        bb += b * b;
    }
    if aa > 0.0 && bb > 0.0 { ab / (aa * bb).sqrt() } else { f64::NEG_INFINITY }
}

/// The cross-section of a real structure is the same on both sides of the hole and stands out of
/// the texture: both side profiles ([`profiles`]) must have a range well above their noise and
/// correlate with each other. Texture that happens to line up averages out to a flat profile and
/// fails. Returns the profile shift (px, exit side relative to entry side) at the best correlation,
/// which tells how far the fitted line is off on the exit side, or `None` when this is no structure.
fn profile_check(cx: &Ctx, n: (f32, f32), rho: f32) -> Option<(f32, [f32; 2])> {
    let [(pa, na, ta), (pb, nb, tb)] = profiles(cx, n, rho)?;
    for (p, noise) in [(&pa, na), (&pb, nb)] {
        let (lo, hi) = p.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
        if hi - lo < 0.015f64.max(5.0 * noise) {
            return None;
        }
    }
    let max_s = (2.0 / PROFILE_BIN) as i32;
    let cs: Vec<f64> = (-max_s..=max_s).map(|s| shifted_corr(&pa, &pb, s)).collect();
    let (bi, best) = cs.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))?;
    if *best < 0.7 {
        return None;
    }
    // Sub-bin peak (parabola through the neighbours).
    let mut s = bi as f32 - max_s as f32;
    if let (Some(l), Some(r)) = (bi.checked_sub(1).and_then(|i| cs.get(i)), cs.get(bi + 1)) {
        let den = l - 2.0 * best + r;
        if l.is_finite() && r.is_finite() && den < 0.0 {
            s += (0.5 * (l - r) / den).clamp(-0.5, 0.5) as f32;
        }
    }
    Some((s * PROFILE_BIN, [ta, tb]))
}

/// Check a Hough peak and refit it: principal axis of its support pixels, then aligned so that the
/// cross-sections on both sides of the hole coincide. Returns the line and its support's indices.
fn validate(cx: &Ctx, theta: f32, rho: f32) -> Option<(Line, Vec<usize>)> {
    let n = (theta.cos(), theta.sin());
    let sup = support(cx, n, rho, 1.5)?;
    let edge = |k: &usize| cx.edges.get(*k);
    // Weighted principal axis of the support pixels.
    let (mut sw, mut mx, mut my) = (0.0f64, 0.0f64, 0.0f64);
    for e in sup.iter().filter_map(edge) {
        sw += f64::from(e.m);
        mx += f64::from(e.m * e.x);
        my += f64::from(e.m * e.y);
    }
    if sw <= 0.0 {
        return None;
    }
    let (mx, my) = (mx / sw, my / sw);
    let (mut cxx, mut cyy, mut cxy) = (0.0f64, 0.0f64, 0.0f64);
    for e in sup.iter().filter_map(edge) {
        let (dx, dy) = (f64::from(e.x) - mx, f64::from(e.y) - my);
        let m = f64::from(e.m);
        cxx += m * dx * dx;
        cyy += m * dy * dy;
        cxy += m * dx * dy;
    }
    let phi = 0.5 * (2.0 * cxy).atan2(cxx - cyy);
    let mut u = (phi.cos() as f32, phi.sin() as f32);
    // Keep the refit close to the peak (a wild refit means the support was not one line).
    let u0 = (-n.1, n.0);
    let cos = u.0 * u0.0 + u.1 * u0.1;
    if cos.abs() < (6.0f32).to_radians().cos() {
        return None;
    }
    if cos < 0.0 {
        u = (-u.0, -u.1);
    }
    let mut n2 = (u.1, -u.0); // so that u = (-n.1, n.0)
    let mut rho2 = n2.0 * mx as f32 + n2.1 * my as f32;
    // Align the two sides' cross-sections: a residual shift δ between them means the line is off by
    // ±δ/2 at the two sides; turn it through the corrected points and check again.
    let (delta, [ta, tb]) = profile_check(cx, n2, rho2)?;
    if delta.abs() > 0.2 {
        let u2 = (-n2.1, n2.0);
        let pa = (n2.0 * (rho2 - 0.5 * delta) + u2.0 * ta, n2.1 * (rho2 - 0.5 * delta) + u2.1 * ta);
        let pb = (n2.0 * (rho2 + 0.5 * delta) + u2.0 * tb, n2.1 * (rho2 + 0.5 * delta) + u2.1 * tb);
        let len = (pb.0 - pa.0).hypot(pb.1 - pa.1);
        if len.is_nan() || len <= 1.0 {
            return None;
        }
        let u3 = ((pb.0 - pa.0) / len, (pb.1 - pa.1) / len);
        let u3 = if u3.0 * u2.0 + u3.1 * u2.1 < 0.0 { (-u3.0, -u3.1) } else { u3 };
        n2 = (u3.1, -u3.0);
        rho2 = n2.0 * pa.0 + n2.1 * pa.1;
        let (d2, _) = profile_check(cx, n2, rho2)?;
        if d2.abs() > 0.75 {
            return None;
        }
    }
    let u = (-n2.1, n2.0);
    let sup2 = support(cx, n2, rho2, 2.0)?;
    let (mut t0, mut t1) = (f32::INFINITY, f32::NEG_INFINITY);
    let mut lin = 0.0f32;
    for e in sup2.iter().filter_map(edge) {
        let t = e.x * u.0 + e.y * u.1;
        t0 = t0.min(t);
        t1 = t1.max(t);
        lin += e.lin;
    }
    let contrast = 2.0 * lin / sup2.len().max(1) as f32;
    (t0.is_finite() && t1 > t0 && contrast.is_finite()).then_some((Line { n: n2, rho: rho2, t0, t1, contrast }, sup2))
}

/// Guide channels for one pyramid level (`scale` = full-resolution pixels per level pixel): for
/// the nearest line within two bands, the signed distance to it in level pixels over `band`, ramped back to zero between one
/// and two bands, inside its span, times `weight · contrast`. Interleaved `lw × lh × lines.len()`.
pub fn guide_channels(lines: &[Line], lw: usize, lh: usize, scale: f32, band: f32, weight: f32) -> Vec<f32> {
    let gc = lines.len();
    let mut g = vec![0.0f32; lw * lh * gc];
    if gc == 0 || band <= 0.0 || scale <= 0.0 {
        return g;
    }
    let off = (scale - 1.0) * 0.5;
    for y in 0..lh {
        for x in 0..lw {
            let (px, py) = (x as f32 * scale + off, y as f32 * scale + off);
            // Each pixel belongs to its nearest line only: where two lines come close (one crossing
            // the other), demanding the same offset from both would leave almost no source.
            let pick = nearest_line(lines, px, py, scale, band);
            let Some((_, k)) = pick else { continue };
            let (Some(l), Some(v)) = (lines.get(k), g.get_mut((y * lw + x) * gc + k)) else { continue };
            *v = profile(l, px, py, scale, band) * weight * l.contrast;
        }
    }
    g
}

/// The line nearest to `(px, py)` among those it is [`near`], with the distance (level pixels).
fn nearest_line(lines: &[Line], px: f32, py: f32, scale: f32, band: f32) -> Option<(f32, usize)> {
    lines
        .iter()
        .enumerate()
        .filter_map(|(k, l)| near(l, px, py, scale, band).map(|d| (d.abs(), k)))
        .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
}

/// Signed distance to `l` in level pixels when `(px, py)` (full-resolution coordinates) lies within
/// two bands of the line and inside its span (extended by a band).
fn near(l: &Line, px: f32, py: f32, scale: f32, band: f32) -> Option<f32> {
    let u = l.u();
    let t = px * u.0 + py * u.1;
    let margin = band * scale;
    let d = (px * l.n.0 + py * l.n.1 - l.rho) / scale;
    (t >= l.t0 - margin && t <= l.t1 + margin && d.abs() < 2.0 * band).then_some(d)
}

/// `(signed distance in level px) / band` ramped to zero between one and two bands; 0 off the span.
fn profile(l: &Line, px: f32, py: f32, scale: f32, band: f32) -> f32 {
    let Some(d) = near(l, px, py, scale, band) else { return 0.0 };
    let d = d / band;
    if d.abs() <= 1.0 {
        d
    } else if d.abs() < 2.0 {
        d.signum() * (2.0 - d.abs())
    } else {
        0.0
    }
}

/// Line-continuation candidates for each target pixel within two bands of a line (and in its span):
/// in each direction along the line, the source centre at the line's *common* shift (the smallest
/// shift that moves every core-band target of that line out of the hole, so the whole band is
/// copied coherently, as one block, like a structure-propagation step), or when that one is not
/// valid, the nearest valid centre along the line. `(-1, -1)` when there is none. Sparse: per pixel
/// an index into the returned candidate list (`u32::MAX`: none); the index vector is empty when
/// there are no lines.
pub(crate) fn line_candidates(
    lines: &[Line],
    lw: usize,
    lh: usize,
    scale: f32,
    band: f32,
    targets: &[bool],
    is_valid: &dyn Fn(i32, i32) -> bool,
) -> (Vec<u32>, Vec<[(i32, i32); 2]>) {
    let none = [(-1, -1); 2];
    if lines.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let off = (scale - 1.0) * 0.5;
    let max_steps = (lw + lh) as i32 * 2;
    let at = |x: usize, y: usize, u: (f32, f32), s: i32| -> (i32, i32) {
        ((x as f32 + s as f32 * u.0).round() as i32, (y as f32 + s as f32 * u.1).round() as i32)
    };
    let inside = |c: (i32, i32)| c.0 >= 0 && c.1 >= 0 && (c.0 as usize) < lw && (c.1 as usize) < lh;
    // Nearest valid step along `u` in direction `sg` from `(x, y)`.
    let march = |x: usize, y: usize, u: (f32, f32), sg: i32| -> Option<i32> {
        for s in 1..=max_steps {
            let c = at(x, y, u, sg * s);
            if !inside(c) {
                return None;
            }
            if is_valid(c.0, c.1) {
                return Some(s);
            }
        }
        None
    };
    // Per target pixel near a line: that line. The common shift comes from marching on a sparse
    // sample of the core band (every other row and column); pixels it does not fit march themselves.
    let mut near_px: Vec<(usize, usize)> = Vec::new();
    let mut common = vec![[0i32; 2]; lines.len()];
    for y in 0..lh {
        for x in 0..lw {
            let i = y * lw + x;
            if !targets.get(i).copied().unwrap_or(false) {
                continue;
            }
            let (px, py) = (x as f32 * scale + off, y as f32 * scale + off);
            let pick = nearest_line(lines, px, py, scale, band);
            let Some((d, k)) = pick else { continue };
            let (Some(l), Some(cm)) = (lines.get(k), common.get_mut(k)) else { continue };
            if d <= band && x % 2 == 0 && y % 2 == 0 {
                for (j, sg) in [1i32, -1].into_iter().enumerate() {
                    if let Some(s) = march(x, y, l.u(), sg) {
                        cm[j] = cm[j].max(s);
                    }
                }
            }
            near_px.push((i, k));
        }
    }
    let mut idx = vec![u32::MAX; lw * lh];
    let mut out = Vec::with_capacity(near_px.len());
    for (i, k) in near_px {
        let (x, y) = (i % lw, i / lw);
        let (Some(l), Some(cm)) = (lines.get(k), common.get(k)) else { continue };
        let u = l.u();
        let mut c = none;
        for (j, sg) in [1i32, -1].into_iter().enumerate() {
            let g = at(x, y, u, sg * cm[j]);
            c[j] = if cm[j] > 0 && inside(g) && is_valid(g.0, g.1) {
                g
            } else if let Some(s) = march(x, y, u, sg) {
                at(x, y, u, sg * s)
            } else {
                (-1, -1)
            };
        }
        if let Some(o) = idx.get_mut(i) {
            *o = out.len() as u32;
            out.push(c);
        }
    }
    (idx, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge_img(s: usize, angle: f32, c: (f32, f32)) -> Vec<f32> {
        let a = angle.to_radians();
        let n = (-a.sin(), a.cos());
        (0..s * s)
            .flat_map(|i| {
                let d = ((i % s) as f32 - c.0) * n.0 + ((i / s) as f32 - c.1) * n.1;
                let t = (d + 0.5).clamp(0.0, 1.0);
                [0.1 + 0.5 * t, 0.2 + 0.2 * t, 0.5 - 0.3 * t]
            })
            .collect()
    }

    #[test]
    fn finds_an_edge_through_the_hole() {
        let s = 120;
        let img = edge_img(s, 33.0, (60.0, 60.0));
        let hole: Vec<bool> = (0..s * s).map(|i| ((i % s) as f32 - 60.0).hypot((i / s) as f32 - 60.0) < 20.0).collect();
        let lines = detect_lines(s, s, 3, &img, &hole, None, &|| false);
        assert_eq!(lines.len(), 1, "{lines:?}");
        let l = &lines[0];
        // Direction within half a degree, passes within a pixel of (60, 60).
        let ang = l.u().1.atan2(l.u().0).to_degrees().rem_euclid(180.0);
        assert!((ang - 33.0).abs() < 0.5, "{ang}");
        assert!((60.0 * l.n.0 + 60.0 * l.n.1 - l.rho).abs() < 1.0);
        assert!(l.contrast > 0.3 && l.contrast < 0.8, "{}", l.contrast);
    }

    #[test]
    fn an_edge_ending_in_the_hole_is_not_a_line() {
        // Edge only on the left half: one-sided support must be rejected.
        let s = 120;
        let full = edge_img(s, 10.0, (60.0, 60.0));
        let img: Vec<f32> =
            (0..s * s).flat_map(|i| if i % s < 60 { [full[i * 3], full[i * 3 + 1], full[i * 3 + 2]] } else { [0.1, 0.2, 0.5] }).collect();
        let hole: Vec<bool> = (0..s * s).map(|i| ((i % s) as f32 - 60.0).hypot((i / s) as f32 - 60.0) < 20.0).collect();
        assert!(detect_lines(s, s, 3, &img, &hole, None, &|| false).is_empty());
    }

    #[test]
    fn flat_and_hostile_inputs_find_nothing() {
        let hole = vec![true; 16];
        assert!(detect_lines(4, 4, 1, &[0.5; 16], &hole, None, &|| false).is_empty());
        assert!(detect_lines(4, 4, 1, &[0.5; 15], &hole, None, &|| false).is_empty());
        assert!(detect_lines(0, 4, 1, &[], &[], None, &|| false).is_empty());
        let img = vec![f32::NAN; 64 * 64];
        let hole: Vec<bool> = (0..64 * 64).map(|i| i % 64 > 20 && i % 64 < 40 && i / 64 > 20 && i / 64 < 40).collect();
        assert!(detect_lines(64, 64, 1, &img, &hole, Some(&[true; 3]), &|| false).is_empty());
    }
}
