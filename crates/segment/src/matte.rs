//! Clean-up for people mattes made on a coarse grid (Selfie Multiclass's 256 × 256, SAM 2.1's
//! silhouettes): enclosed specks filled ([`fill_small_holes`]), each person's region kept to the
//! part joined to their face ([`clean_owners`]), and edges pulled onto the photo's own edges by a
//! guided filter ([`guided_filter`], He, Sun and Tang, "Guided Image Filtering", 2010/2013).
//!
//! Own design (OneSlips); plain Rust on slices.

use std::collections::VecDeque;

/// No one's cell in an owner grid.
pub const NOBODY: u16 = u16::MAX;

/// Fill the holes of a `side × side` coverage grid that the outside can't reach (cells under
/// 0.5 enclosed by cells at 0.5 or more) and that are at most `max_cells` large: the specks a
/// coarse class map leaves in lace, sequins or a bouquet held in front. Larger enclosed gaps (the
/// space between an arm and the body) stay. Filled cells take 1.0.
pub fn fill_small_holes(cov: &mut [f32], side: usize, max_cells: usize) {
    let n = side.saturating_mul(side);
    if side == 0 || cov.len() < n {
        return;
    }
    let low = |c: f32| c.is_nan() || c < 0.5;
    // 0 unvisited, 1 outside, 2 visited hole
    let mut seen = vec![0u8; n];
    let mut q = VecDeque::new();
    for i in 0..side {
        for idx in [i, (side - 1) * side + i, i * side, i * side + side - 1] {
            if low(cov[idx]) && seen[idx] == 0 {
                seen[idx] = 1;
                q.push_back(idx);
            }
        }
    }
    flood(&mut q, &mut seen, side, 1, |j| low(cov[j]));
    let mut comp = Vec::new();
    for start in 0..n {
        if seen[start] != 0 || !low(cov[start]) {
            continue;
        }
        comp.clear();
        seen[start] = 2;
        q.push_back(start);
        while let Some(i) = q.pop_front() {
            comp.push(i);
            for j in neighbours(i, side) {
                if seen[j] == 0 && low(cov[j]) {
                    seen[j] = 2;
                    q.push_back(j);
                }
            }
        }
        if comp.len() <= max_cells {
            for &i in &comp {
                cov[i] = 1.0;
            }
        }
    }
}

pub(crate) fn neighbours(i: usize, side: usize) -> impl Iterator<Item = usize> {
    let (x, y) = (i % side, i / side);
    [(x > 0).then(|| i - 1), (x + 1 < side).then(|| i + 1), (y > 0).then(|| i - side), (y + 1 < side).then(|| i + side)].into_iter().flatten()
}

fn flood(q: &mut VecDeque<usize>, seen: &mut [u8], side: usize, mark: u8, open: impl Fn(usize) -> bool) {
    while let Some(i) = q.pop_front() {
        for j in neighbours(i, side) {
            if seen[j] == 0 && open(j) {
                seen[j] = mark;
                q.push_back(j);
            }
        }
    }
}

/// An owner grid cleaned up: each person keeps only the cells of theirs joined to their face cell
/// (`seeds`, one per person, `None` when off the grid); islands they claimed elsewhere (a SAM
/// silhouette leaking onto a neighbour's lace dress) become [`NOBODY`], and then every unowned
/// cell between one person's cells (two or more of its four neighbours theirs, none anyone
/// else's) is given to them, a few passes over.
pub fn clean_owners(owner: &mut [u16], side: usize, seeds: &[Option<usize>]) {
    let n = side.saturating_mul(side);
    if side == 0 || owner.len() < n {
        return;
    }
    let mut keep = vec![false; n];
    let mut q = VecDeque::new();
    for (k, seed) in seeds.iter().enumerate() {
        let Some(s) = *seed else { continue };
        let Ok(k) = u16::try_from(k) else { break };
        if s >= n || owner[s] != k || keep[s] {
            continue;
        }
        keep[s] = true;
        q.push_back(s);
        while let Some(i) = q.pop_front() {
            for j in neighbours(i, side) {
                if !keep[j] && owner[j] == k {
                    keep[j] = true;
                    q.push_back(j);
                }
            }
        }
    }
    for (o, k) in owner.iter_mut().zip(&keep) {
        // a person without a seed on the grid keeps what they have
        let seeded = usize::from(*o) < seeds.len() && seeds.get(usize::from(*o)).is_some_and(Option::is_some);
        if *o != NOBODY && seeded && !*k {
            *o = NOBODY;
        }
    }
    for _ in 0..4 {
        let snapshot = owner.to_vec();
        let mut changed = false;
        for i in 0..n {
            if snapshot[i] != NOBODY {
                continue;
            }
            // one person's cells on at least two sides and no one else's
            let mut who = NOBODY;
            let mut count = 0;
            let mut mixed = false;
            for j in neighbours(i, side) {
                let o = snapshot[j];
                if o == NOBODY {
                    continue;
                }
                if who == NOBODY || who == o {
                    who = o;
                    count += 1;
                } else {
                    mixed = true;
                }
            }
            if !mixed && count >= 2 {
                owner[i] = who;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

/// Box sums of a `w × h` grid over `(2r + 1)²` windows (clipped at the edges), divided by the
/// window's cell count: a mean filter.
fn box_mean(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut integral = vec![0f64; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0f64;
        for x in 0..w {
            row += f64::from(v.get(y * w + x).copied().unwrap_or(0.0));
            integral[(y + 1) * (w + 1) + x + 1] = integral[y * (w + 1) + x + 1] + row;
        }
    }
    let mut out = vec![0f32; w * h];
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(r), (y + r + 1).min(h));
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(w));
            let s = integral[y1 * (w + 1) + x1] - integral[y0 * (w + 1) + x1] - integral[y1 * (w + 1) + x0] + integral[y0 * (w + 1) + x0];
            let cells = ((x1 - x0) * (y1 - y0)).max(1) as f64;
            out[y * w + x] = (s / cells) as f32;
        }
    }
    out
}

/// The guided filter of `p` (a `w × h` coverage) with the grey guide `g` (same size, 0..1),
/// radius `r` cells and regulariser `eps`: where the guide has an edge the result follows it,
/// elsewhere it is a smooth version of `p`. Clamped to 0..1. Inputs of the wrong size come back
/// unchanged.
pub fn guided_filter(p: &[f32], g: &[f32], w: usize, h: usize, r: usize, eps: f32) -> Vec<f32> {
    let n = w.saturating_mul(h);
    if n == 0 || p.len() != n || g.len() != n || r == 0 {
        return p.to_vec();
    }
    let mean_g = box_mean(g, w, h, r);
    let mean_p = box_mean(p, w, h, r);
    let gp: Vec<f32> = g.iter().zip(p).map(|(a, b)| a * b).collect();
    let gg: Vec<f32> = g.iter().map(|a| a * a).collect();
    let corr_gp = box_mean(&gp, w, h, r);
    let corr_gg = box_mean(&gg, w, h, r);
    let mut a = vec![0f32; n];
    let mut b = vec![0f32; n];
    for i in 0..n {
        let var = corr_gg[i] - mean_g[i] * mean_g[i];
        let cov = corr_gp[i] - mean_g[i] * mean_p[i];
        a[i] = cov / (var + eps);
        b[i] = mean_p[i] - a[i] * mean_g[i];
    }
    let (ma, mb) = (box_mean(&a, w, h, r), box_mean(&b, w, h, r));
    (0..n)
        .map(|i| {
            let v = ma[i] * g[i] + mb[i];
            if v.is_finite() { v.clamp(0.0, 1.0) } else { p[i] }
        })
        .collect()
}

/// [`guided_filter`] with a colour guide (`g`: `w × h` RGB, 0..1, interleaved): an edge in colour
/// alone — a white dress against pale skin of the same lightness — is followed too (He, Sun and
/// Tang's colour guided filter: a 3 × 3 covariance per window). Inputs of the wrong size come back
/// unchanged.
pub fn guided_filter_rgb(p: &[f32], g: &[f32], w: usize, h: usize, r: usize, eps: f32) -> Vec<f32> {
    let n = w.saturating_mul(h);
    if n == 0 || p.len() != n || g.len() != n.saturating_mul(3) || r == 0 {
        return p.to_vec();
    }
    let ch = |c: usize| -> Vec<f32> { g.iter().skip(c).step_by(3).copied().collect() };
    let gc = [ch(0), ch(1), ch(2)];
    let mean_g: Vec<Vec<f32>> = gc.iter().map(|c| box_mean(c, w, h, r)).collect();
    let mean_p = box_mean(p, w, h, r);
    let corr_gp: Vec<Vec<f32>> = gc.iter().map(|c| box_mean(&c.iter().zip(p).map(|(a, b)| a * b).collect::<Vec<_>>(), w, h, r)).collect();
    // the six distinct products of the colour channels
    let pairs = [(0, 0), (0, 1), (0, 2), (1, 1), (1, 2), (2, 2)];
    let corr_gg: Vec<Vec<f32>> =
        pairs.iter().map(|(i, j)| box_mean(&gc[*i].iter().zip(&gc[*j]).map(|(a, b)| a * b).collect::<Vec<_>>(), w, h, r)).collect();
    let mut a = [vec![0f32; n], vec![0f32; n], vec![0f32; n]];
    let mut b = vec![0f32; n];
    for k in 0..n {
        let m = [mean_g[0][k], mean_g[1][k], mean_g[2][k]];
        let s = |q: usize, i: usize, j: usize| corr_gg[q][k] - m[i] * m[j] + if i == j { eps } else { 0.0 };
        let (s00, s01, s02, s11, s12, s22) = (s(0, 0, 0), s(1, 0, 1), s(2, 0, 2), s(3, 1, 1), s(4, 1, 2), s(5, 2, 2));
        let cov = [corr_gp[0][k] - m[0] * mean_p[k], corr_gp[1][k] - m[1] * mean_p[k], corr_gp[2][k] - m[2] * mean_p[k]];
        // the inverse of the symmetric 3 × 3 matrix, by cofactors
        let (c00, c01, c02) = (s11 * s22 - s12 * s12, s02 * s12 - s01 * s22, s01 * s12 - s02 * s11);
        let det = s00 * c00 + s01 * c01 + s02 * c02;
        let (ak, bk) = if det.abs() > 1e-12 && det.is_finite() {
            let (c11, c12, c22) = (s00 * s22 - s02 * s02, s01 * s02 - s00 * s12, s00 * s11 - s01 * s01);
            let x = [
                (c00 * cov[0] + c01 * cov[1] + c02 * cov[2]) / det,
                (c01 * cov[0] + c11 * cov[1] + c12 * cov[2]) / det,
                (c02 * cov[0] + c12 * cov[1] + c22 * cov[2]) / det,
            ];
            (x, mean_p[k] - x[0] * m[0] - x[1] * m[1] - x[2] * m[2])
        } else {
            ([0.0; 3], mean_p[k])
        };
        for (c, v) in a.iter_mut().zip(ak) {
            c[k] = v;
        }
        b[k] = bk;
    }
    let ma: Vec<Vec<f32>> = a.iter().map(|v| box_mean(v, w, h, r)).collect();
    let mb = box_mean(&b, w, h, r);
    (0..n)
        .map(|k| {
            let v = ma[0][k] * gc[0][k] + ma[1][k] * gc[1][k] + ma[2][k] * gc[2][k] + mb[k];
            if v.is_finite() { v.clamp(0.0, 1.0) } else { p[k] }
        })
        .collect()
}

/// The 4-connected regions of a `side × side` grid's cells at or above `thresh`: each cell's
/// region number ([`u32::MAX`] below `thresh`) and the region count. Inputs of the wrong size give
/// no regions.
pub fn regions(cov: &[f32], side: usize, thresh: f32) -> (Vec<u32>, usize) {
    let n = side.saturating_mul(side);
    let mut label = vec![u32::MAX; n];
    if side == 0 || cov.len() < n {
        return (label, 0);
    }
    let on = |i: usize| cov.get(i).is_some_and(|v| *v >= thresh);
    let mut q = VecDeque::new();
    let mut count = 0u32;
    for start in 0..n {
        if label[start] != u32::MAX || !on(start) {
            continue;
        }
        label[start] = count;
        q.push_back(start);
        while let Some(i) = q.pop_front() {
            for j in neighbours(i, side) {
                if label[j] == u32::MAX && on(j) {
                    label[j] = count;
                    q.push_back(j);
                }
            }
        }
        count = count.saturating_add(1);
    }
    (label, count as usize)
}

/// Smooth step from `lo` (0) to `hi` (1).
pub fn smoothstep(lo: f32, hi: f32, v: f32) -> f32 {
    if hi.partial_cmp(&lo) != Some(std::cmp::Ordering::Greater) {
        return if v >= hi { 1.0 } else { 0.0 };
    }
    let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The photo's colours averaged over each cell of a `side × side` grid laid over its part `rect`
/// (normalized `[x0, y0, x1, y1]`) — the guide of [`refine_edges`] — as interleaved RGB in 0..1;
/// `None` when `rgb` isn't a `w × h` photo or `rect` is empty. Averaging (not point sampling) keeps
/// a fine texture (grass, lace) from aliasing into false edges when the photo is larger than the
/// grid.
pub fn cell_colours(rgb: &[u8], w: usize, h: usize, side: usize, rect: [f64; 4]) -> Option<Vec<f32>> {
    if w == 0 || h == 0 || side == 0 || rgb.len() < w.saturating_mul(h).saturating_mul(3) {
        return None;
    }
    if !(rect.iter().all(|v| v.is_finite()) && rect[2] > rect[0] && rect[3] > rect[1]) {
        return None;
    }
    // the photo pixel span of cell `i` along an axis of `len` pixels
    let span = |i: usize, lo: f64, hi: f64, len: usize| -> (usize, usize) {
        let a = (lo + (hi - lo) * i as f64 / side as f64) * len as f64;
        let b = (lo + (hi - lo) * (i + 1) as f64 / side as f64) * len as f64;
        let a = (a.floor().max(0.0) as usize).min(len - 1);
        let b = (b.ceil().max(0.0) as usize).clamp(a + 1, len);
        (a, b)
    };
    let xs: Vec<(usize, usize)> = (0..side).map(|i| span(i, rect[0], rect[2], w)).collect();
    let mut out = vec![0f32; side * side * 3];
    for j in 0..side {
        let (y0, y1) = span(j, rect[1], rect[3], h);
        for (i, &(x0, x1)) in xs.iter().enumerate() {
            // at most 4 × 4 samples per cell: plenty for a guide, and bounded for huge cells
            let (sx, sy) = ((x1 - x0).div_ceil(4).max(1), (y1 - y0).div_ceil(4).max(1));
            let mut acc = [0f32; 3];
            let mut n = 0f32;
            for y in (y0..y1).step_by(sy) {
                for x in (x0..x1).step_by(sx) {
                    if let Some(p) = rgb.get((y * w + x) * 3..(y * w + x) * 3 + 3) {
                        acc[0] += f32::from(p[0]);
                        acc[1] += f32::from(p[1]);
                        acc[2] += f32::from(p[2]);
                        n += 1.0;
                    }
                }
            }
            let k = (j * side + i) * 3;
            if n > 0.0
                && let Some(o) = out.get_mut(k..k + 3)
            {
                for (o, a) in o.iter_mut().zip(acc) {
                    *o = a / n / 255.0;
                }
            }
        }
    }
    Some(out)
}

/// The edge refinement every AI mask shares (Sky, Subject, People, garments): a coarse coverage
/// (`cov`, `side²` cells, already resampled to the grid) is pulled onto the photo's own edges by a
/// colour guided filter (`guide`: [`cell_colours`] of the same grid) of radius `radius` cells —
/// wide enough to reach across the coarse model's blur — then steepened, so the soft tail outside
/// the object (the halo a low-resolution model leaves when stretched over a photo) goes and its
/// inside stays whole. Inputs of the wrong size come back unchanged.
pub fn refine_edges(cov: &[f32], guide: &[f32], side: usize, radius: usize) -> Vec<f32> {
    let n = side.saturating_mul(side);
    if n == 0 || cov.len() != n || guide.len() != n.saturating_mul(3) || radius == 0 {
        return cov.to_vec();
    }
    guided_filter_rgb(cov, guide, side, side, radius, 1e-3).into_iter().map(|v| smoothstep(0.25, 0.75, v)).collect()
}

/// Bilinear resampling of a `w × h` grid of values to `side × side` cells over the same area.
pub fn resample(v: &[f32], w: usize, h: usize, side: usize) -> Vec<f32> {
    if w == 0 || h == 0 || v.len() < w.saturating_mul(h) || side == 0 {
        return vec![0.0; side.saturating_mul(side)];
    }
    let at = |x: usize, y: usize| v.get(y.min(h - 1) * w + x.min(w - 1)).copied().unwrap_or(0.0);
    let (sx, sy) = (w as f32 / side as f32, h as f32 / side as f32);
    let mut out = vec![0f32; side * side];
    for j in 0..side {
        let fy = ((j as f32 + 0.5) * sy - 0.5).clamp(0.0, (h - 1) as f32);
        let (y0, ty) = (fy.floor() as usize, fy - fy.floor());
        for i in 0..side {
            let fx = ((i as f32 + 0.5) * sx - 0.5).clamp(0.0, (w - 1) as f32);
            let (x0, tx) = (fx.floor() as usize, fx - fx.floor());
            let top = at(x0, y0) * (1.0 - tx) + at(x0 + 1, y0) * tx;
            let bot = at(x0, y0 + 1) * (1.0 - tx) + at(x0 + 1, y0 + 1) * tx;
            out[j * side + i] = top * (1.0 - ty) + bot * ty;
        }
    }
    out
}

/// Sky and foliage sorted by colour where they meet (`cov`: a refined sky coverage, `side²`
/// cells; `guide`: the cells' colours; `foliage`: how much each cell is trees, 0..1). A coarse
/// model paints the gaps between palm fronds and branches as tree, and the thin branches against
/// the sky as sky. In and around the trees, each cell's colour is compared with the clear sky
/// near it (the local mean of cells that are confidently sky, not trees and smooth, within
/// `radius` cells; its spread kept between a hazy gradient's and a small step's): like the sky,
/// it is sky; unlike it (a branch, a leaf), it is not. Only cells the trees reach change, so a
/// white shirt or a roof beside the sky never becomes sky, and clouds away from the trees keep
/// the model's answer. Inputs of the wrong size are left alone.
pub fn fill_sky_holes(cov: &mut [f32], guide: &[f32], foliage: &[f32], side: usize, radius: usize) {
    let n = side.saturating_mul(side);
    if n == 0 || cov.len() != n || guide.len() != n.saturating_mul(3) || foliage.len() != n || radius == 0 {
        return;
    }
    let ch = |c: usize| -> Vec<f32> { guide.iter().skip(c).step_by(3).copied().collect() };
    // smoothness: the local spread of luminance over 3 × 3 cells (a sky is smooth, a canopy isn't)
    let luma: Vec<f32> = (0..n).map(|k| guide.get(k * 3..k * 3 + 3).map_or(0.0, |p| 0.3 * p[0] + 0.59 * p[1] + 0.11 * p[2])).collect();
    let lm = box_mean(&luma, side, side, 1);
    let lmm = box_mean(&luma.iter().map(|l| l * l).collect::<Vec<_>>(), side, side, 1);
    let conf: Vec<f32> = (0..n)
        .map(|k| {
            let smooth = (lmm[k] - lm[k] * lm[k]).max(0.0).sqrt() < 0.03;
            if cov[k] >= 0.9 && foliage[k] < 0.2 && smooth { 1.0 } else { 0.0 }
        })
        .collect();
    let weight = box_mean(&conf, side, side, radius);
    let mut mean = Vec::with_capacity(3);
    let mut var = vec![0f32; n];
    for c in 0..3 {
        let g = ch(c);
        let wg = box_mean(&g.iter().zip(&conf).map(|(a, k)| a * k).collect::<Vec<_>>(), side, side, radius);
        let wgg = box_mean(&g.iter().zip(&conf).map(|(a, k)| a * a * k).collect::<Vec<_>>(), side, side, radius);
        let m: Vec<f32> = wg.iter().zip(&weight).map(|(s, w)| if *w > 1e-6 { s / w } else { 0.0 }).collect();
        for k in 0..n {
            if weight[k] > 1e-6 {
                var[k] += (wgg[k] / weight[k] - m[k] * m[k]).max(0.0);
            }
        }
        mean.push(m);
    }
    for k in 0..n {
        // where the trees reach, and some clear sky is near enough to know its colour
        let reach = smoothstep(0.05, 0.2, foliage[k]) * smoothstep(0.002, 0.02, weight[k]);
        if reach <= 0.0 {
            continue;
        }
        let d2: f32 = (0..3).map(|c| (guide[k * 3 + c] - mean[c][k]).powi(2)).sum();
        // the sky's own spread, between a gentle gradient's and a small step's
        let s2 = var[k].clamp(0.025 * 0.025, 0.06 * 0.06) * 4.0;
        let like = 1.0 - smoothstep(1.0, 3.0, d2 / s2);
        cov[k] = cov[k] * (1.0 - reach) + like * reach;
    }
}

/// A coarse model's probabilities (`probs`, `pw × ph` cells over the whole photo: EoMT's 160-pixel
/// output for Sky, Landscape and its People) refined on the photo (`rgb`, `w × h`, the image the
/// model saw) into a `side × side` coverage of it: resampled, its edges pulled onto the photo's
/// own by [`refine_edges`] (radius: two of the coarse cells, so the whole blur is in reach), and,
/// given `foliage` (the same grid as `probs`: how much each cell is trees and plants), the sky
/// seen through the leaves filled in ([`fill_sky_holes`]). Without a usable photo, just the
/// resampled probabilities.
#[allow(clippy::too_many_arguments)]
pub fn refine_coarse(probs: &[f32], pw: usize, ph: usize, rgb: &[u8], w: usize, h: usize, side: usize, foliage: Option<&[f32]>) -> Vec<f32> {
    let cov = resample(probs, pw, ph, side);
    let Some(guide) = cell_colours(rgb, w, h, side, [0.0, 0.0, 1.0, 1.0]) else { return cov };
    let coarse_cell = side as f32 / pw.max(ph).max(1) as f32;
    let radius = ((coarse_cell * 2.0).ceil() as usize).clamp(2, side / 8 + 2);
    let mut out = refine_edges(&cov, &guide, side, radius);
    if let Some(f) = foliage.filter(|f| f.len() >= pw.saturating_mul(ph)) {
        let foliage = resample(f, pw, ph, side);
        fill_sky_holes(&mut out, &guide, &foliage, side, (side / 8).max(2));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two colours of the same lightness side by side (pale skin, a dress tinted the other way):
    /// the colour guide sees the edge and pulls the soft edge onto it.
    #[test]
    fn the_colour_guided_filter_follows_an_edge_in_colour_alone() {
        let (w, h) = (40usize, 8usize);
        let g: Vec<f32> = (0..w * h).flat_map(|i| if i % w < 20 { [0.8, 0.6, 0.5] } else { [0.5, 0.6, 0.8] }).collect();
        let p: Vec<f32> = (0..w * h).map(|i| (((i % w) as f32 - 14.0) / 12.0).clamp(0.0, 1.0)).collect();
        let q = guided_filter_rgb(&p, &g, w, h, 4, 1e-4);
        let at = |v: &[f32], x: usize| v[3 * w + x];
        assert!(at(&q, 18) < at(&p, 18) - 0.05, "{} {}", at(&q, 18), at(&p, 18));
        assert!(at(&q, 22) > at(&p, 22) + 0.05, "{} {}", at(&q, 22), at(&p, 22));
        assert!(q.iter().all(|v| (0.0..=1.0).contains(v)));
        assert_eq!(guided_filter_rgb(&p, &g[..10], w, h, 4, 1e-3), p);
        // a flat guide: a smooth copy, no NaN
        let flat = vec![0.5f32; w * h * 3];
        assert!(guided_filter_rgb(&p, &flat, w, h, 4, 1e-4).iter().all(|v| v.is_finite()));
    }

    /// A ring of coverage with a speck missing inside a blob, and a gap open to the outside.
    #[test]
    fn small_enclosed_holes_fill_and_open_gaps_stay() {
        let side = 20;
        let mut cov = vec![0f32; side * side];
        for y in 2..18 {
            for x in 2..18 {
                cov[y * side + x] = 1.0;
            }
        }
        // a 2 × 2 speck inside, a 6 × 6 hole inside, a notch open to the outside
        for (x, y) in [(5, 5), (6, 5), (5, 6), (6, 6)] {
            cov[y * side + x] = 0.0;
        }
        for y in 9..15 {
            for x in 9..15 {
                cov[y * side + x] = 0.0;
            }
        }
        for x in 0..4 {
            cov[3 * side + x] = 0.0;
        }
        fill_small_holes(&mut cov, side, 10);
        assert_eq!(cov[5 * side + 5], 1.0, "the speck is filled");
        assert_eq!(cov[11 * side + 11], 0.0, "a large hole stays");
        assert_eq!(cov[3 * side + 3], 0.0, "a notch open to the outside stays");
        // junk sizes never panic
        fill_small_holes(&mut cov, 0, 10);
        fill_small_holes(&mut [0.0; 3], 5, 10);
    }

    /// A neighbour's claim inside a person's region becomes theirs; the neighbour keeps their own.
    #[test]
    fn islands_go_to_the_surrounding_person() {
        let side = 12;
        let mut owner = vec![NOBODY; side * side];
        for y in 0..side {
            for x in 0..side {
                owner[y * side + x] = if x < 6 { 0 } else { 1 };
            }
        }
        // person 1 claims a 2 × 2 island inside person 0's half
        for (x, y) in [(2, 8), (3, 8), (2, 9), (3, 9)] {
            owner[y * side + x] = 1;
        }
        clean_owners(&mut owner, side, &[Some(2 * side + 2), Some(2 * side + 9)]);
        assert_eq!(owner[8 * side + 2], 0, "the island is person 0's");
        assert_eq!(owner[8 * side + 9], 1, "person 1 keeps their own");
        clean_owners(&mut owner, side, &[Some(usize::MAX), None]);
        clean_owners(&mut [], 4, &[]);
    }

    /// The filter pulls a soft edge onto the guide's sharp one and leaves flat areas alone.
    #[test]
    fn guided_filter_follows_the_guides_edge() {
        let (w, h) = (40usize, 8usize);
        let g: Vec<f32> = (0..w * h).map(|i| if i % w < 20 { 0.1 } else { 0.9 }).collect();
        // coverage ramps over cells 14..26
        let p: Vec<f32> = (0..w * h).map(|i| (((i % w) as f32 - 14.0) / 12.0).clamp(0.0, 1.0)).collect();
        let q = guided_filter(&p, &g, w, h, 4, 1e-3);
        let at = |v: &[f32], x: usize| v[3 * w + x];
        assert!(at(&q, 18) < at(&p, 18), "{} {}", at(&q, 18), at(&p, 18));
        assert!(at(&q, 22) > at(&p, 22), "{} {}", at(&q, 22), at(&p, 22));
        assert!(q.iter().all(|v| (0.0..=1.0).contains(v)));
        assert_eq!(guided_filter(&p, &g[..10], w, h, 4, 1e-3), p);
    }

    #[test]
    fn regions_are_four_connected() {
        // three blobs on a 4² grid, touching only at corners
        let cov = [1.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        let (label, count) = regions(&cov, 4, 0.5);
        assert_eq!(count, 3);
        assert_eq!(label[0], label[4]);
        assert_ne!(label[0], label[9]);
        assert_eq!(label[9], label[10]);
        assert_ne!(label[10], label[15]);
        assert_eq!(label[2], u32::MAX);
        assert_eq!(regions(&cov, 5, 0.5).1, 0, "wrong size: no regions");
    }

    /// Regression (client note 26): a sky from EoMT's 160-pixel output stretched over the photo
    /// left a soft halo a dozen pixels wide over the horizon and the people. Refined on the photo,
    /// the edge sits on the photo's own edge.
    #[test]
    fn a_coarse_mask_lands_on_the_photos_edge_without_a_halo() {
        // the photo: blue sky above row 101 of 256, a darker ground below; the coarse model saw it
        // at 16 × 16 with a soft step two cells wide around row 6.3
        let (w, h) = (256usize, 256usize);
        let mut rgb = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let c = if y < 101 { [120, 170, 230] } else { [90, 80, 60] };
                rgb[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&c);
            }
        }
        let probs: Vec<f32> = (0..16 * 16).map(|i| 1.0 - smoothstep(4.8, 7.8, (i / 16) as f32 + 0.5)).collect();
        let coarse = resample(&probs, 16, 16, 256);
        let out = refine_coarse(&probs, 16, 16, &rgb, w, h, 256, None);
        assert_eq!(out.len(), 256 * 256);
        let at = |v: &[f32], y: usize| v[y * 256 + 128];
        // the halo: the coarse mask is well above zero below the horizon, the refined one isn't
        assert!(at(&coarse, 106) > 0.2, "the coarse halo: {}", at(&coarse, 106));
        assert!(at(&out, 104) < 0.05, "no halo below the horizon: {}", at(&out, 104));
        assert!(at(&out, 97) > 0.95, "the sky is whole up to it: {}", at(&out, 97));
        // nothing usable to refine on: the resampled probabilities
        assert_eq!(refine_coarse(&probs, 16, 16, &[], 0, 0, 64, None), resample(&probs, 16, 16, 64));
        assert_eq!(refine_edges(&[0.5; 4], &[0.0; 3], 2, 1), vec![0.5; 4], "wrong sizes: unchanged");
    }

    /// Client note 26: the sky seen between palm fronds was tree in the Sky mask, and branches
    /// against the sky were sky. Where the trees reach, a cell the colour of the clear sky near it
    /// is sky and one unlike it is not; a white wall beside the sky (not trees) is untouched.
    #[test]
    fn sky_through_the_leaves_is_sky() {
        let side = 64;
        let mut guide = vec![0f32; side * side * 3];
        let mut cov = vec![0f32; side * side];
        let mut foliage = vec![0f32; side * side];
        for j in 0..side {
            for i in 0..side {
                let k = j * side + i;
                let sky = [0.5, 0.7, 0.95];
                let c = if j < 24 && (10..12).contains(&i) && (8..20).contains(&j) {
                    // a branch against the sky, which the coarse model called sky
                    cov[k] = 1.0;
                    foliage[k] = 0.5;
                    [0.2, 0.15, 0.1]
                } else if j < 24 {
                    cov[k] = 1.0;
                    sky
                } else if i < 32 {
                    // the palm: green fronds with sky-coloured gaps every fourth cell
                    foliage[k] = 1.0;
                    if (i + j) % 4 == 0 { sky } else { [0.15, 0.35, 0.1] }
                } else {
                    // a white wall
                    [0.95, 0.95, 0.95]
                };
                guide[k * 3..k * 3 + 3].copy_from_slice(&c);
            }
        }
        let before = cov.clone();
        fill_sky_holes(&mut cov, &guide, &foliage, side, 8);
        let gap = 28 * side + 12; // (12 + 28) % 4 == 0
        let frond = 28 * side + 13;
        let wall = 28 * side + 40;
        assert!(cov[gap] > 0.8, "the gap is sky: {}", cov[gap]);
        assert!(cov[frond] < 0.05, "the frond is not: {}", cov[frond]);
        assert!(cov[wall] < 0.05, "the wall is not");
        assert!(cov[60 * side + 12] < 0.05, "a gap too far from any sky stays as it was");
        assert!(cov[14 * side + 10] < 0.1, "the branch is not sky: {}", cov[14 * side + 10]);
        assert_eq!(cov[..8 * side], before[..8 * side], "clear sky stays");
        // wrong sizes: left alone
        let mut short = vec![0.0; 4];
        fill_sky_holes(&mut short, &guide, &foliage, side, 8);
        assert_eq!(short, vec![0.0; 4]);
    }

    #[test]
    fn cell_colours_average_and_reject_bad_input() {
        // a 4 × 2 photo, left half black, right half white, over a 2-cell grid
        let mut rgb = vec![0u8; 4 * 2 * 3];
        for y in 0..2 {
            for x in 2..4 {
                rgb[(y * 4 + x) * 3..(y * 4 + x) * 3 + 3].copy_from_slice(&[255, 255, 255]);
            }
        }
        let g = cell_colours(&rgb, 4, 2, 2, [0.0, 0.0, 1.0, 1.0]).unwrap();
        assert!(g[0] < 0.01 && (g[3] - 1.0).abs() < 0.01);
        assert!(cell_colours(&rgb, 4, 2, 2, [0.5, 0.0, 0.5, 1.0]).is_none());
        assert!(cell_colours(&rgb, 4, 2, 2, [f64::NAN, 0.0, 1.0, 1.0]).is_none());
        assert!(cell_colours(&rgb[..5], 4, 2, 2, [0.0, 0.0, 1.0, 1.0]).is_none());
        assert_eq!(resample(&[], 0, 0, 3), vec![0.0; 9]);
        assert!(resample(&[0.25; 6], 3, 2, 8).iter().all(|v| (v - 0.25).abs() < 1e-6));
        assert_eq!(smoothstep(1.0, 1.0, 2.0), 1.0);
        assert_eq!(smoothstep(1.0, f32::NAN, 0.0), 0.0);
    }
}
