//! Hole filling for Spot Healing: patch-based completion, proximity match and texture synthesis.
//!
//! * [`complete`], content-aware fill: the multi-scale "space-time completion" EM scheme of
//!   Y. Wexler, E. Shechtman, M. Irani, *Space-Time Completion of Video*, IEEE TPAMI 2007, with the
//!   nearest-neighbour field computed by C. Barnes, E. Shechtman, A. Finkelstein, D. B. Goldman,
//!   *PatchMatch: A Randomized Correspondence Algorithm for Structural Image Editing*, SIGGRAPH 2009
//!   (random init, propagation, exponentially shrinking random search). Each EM iteration finds, for
//!   every patch overlapping the hole, its most similar fully-known patch, then re-estimates hole
//!   pixels by a similarity-weighted vote of the overlapping matches. Coarse-to-fine over an image
//!   pyramid (the coarsest level is initialised by membrane interpolation), so large holes stay cheap.
//!   With [`CompleteParams::structure`] on, lines crossing the hole ([`crate::structure`]) add guide
//!   channels to the patch distance and line-continuation candidates to PatchMatch at every level.
//!   [`Guide::exclude`] keeps chosen pixels out of every source patch (exclusion is propagated to
//!   coarser levels conservatively).
//! * `best_offset` (PhotoCraft only), proximity match: the single displacement whose surrounding ring best matches the
//!   ring around the hole (sum of squared differences), i.e. an automatic Healing Brush source.
//! * `synthesize` (PhotoCraft only), create texture: patch-based texture synthesis in the style of A. Efros,
//!   W. Freeman, *Image Quilting for Texture Synthesis and Transfer*, SIGGRAPH 2001 (blocks picked from
//!   random candidates by overlap error, feathered overlap instead of a min-cut seam).
//!
//! Ported from PhotoCraft's `algo::inpaint` (MIT OR Apache-2.0); its proximity match and texture
//! synthesis are left out.
//!
//! All functions take interleaved `w × h × ch` normalised floats (any colour model/depth) and a
//! `w × h` hole mask, and return a full buffer equal to the input outside the hole. They are
//! deterministic (fixed-seed hash RNG, results independent of thread count).

use crate::Cancelled;
use crate::membrane::membrane_fill;

/// Below this many grid cells a level runs single-threaded (thread hand-off costs more than it saves).
#[cfg(not(target_arch = "wasm32"))]
const PAR_MIN: usize = 128 * 128;

/// Lines are detected on the finest pyramid level whose hole extent is at most this many pixels.
const DETECT_EXTENT: usize = 256;

/// SplitMix64: tiny deterministic RNG.
#[derive(Clone)]
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform integer in `lo..=hi`.
    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        if hi <= lo {
            return lo;
        }
        lo + (self.next() % (hi - lo + 1) as u64) as i32
    }
}

/// Parameters for [`complete`].
#[derive(Clone, Debug, PartialEq)]
pub struct CompleteParams {
    /// Patch radius (patches are `2r+1` square).
    pub patch_radius: usize,
    /// EM iterations at the coarsest level (finer levels use fewer, down to `min_em_iters`).
    pub em_iters: usize,
    pub min_em_iters: usize,
    /// PatchMatch passes per EM iteration.
    pub pm_iters: usize,
    /// Levels where the hole is wider than this (in that level's pixels) only *refine* the upsampled
    /// match field (one local PatchMatch pass and one vote), which keeps big holes tractable: the
    /// structure is decided at coarse scale, fine levels just add detail.
    pub refine_extent: usize,
    pub seed: u64,
    /// Strength of the structure pass ([`crate::structure`]): straight edges and lines that cross
    /// the hole are detected and continued through it. `0` turns it off (plain texture completion).
    pub structure: f32,
    /// Blend the fill into the known pixels around it ([`seam_blend`]) so copied blocks meet them
    /// without a hard seam.
    pub seam_blend: bool,
}

impl Default for CompleteParams {
    fn default() -> Self {
        Self { patch_radius: 3, em_iters: 8, min_em_iters: 2, pm_iters: 2, refine_extent: 96, seed: 1, structure: 1.0, seam_blend: true }
    }
}

/// Extra inputs for [`complete_guided`].
#[derive(Clone, Copy, Debug, Default)]
pub struct Guide<'a> {
    /// `w × h` pixels that must never be copied from (other removed areas, faces, …). They are not
    /// filled: they come back unchanged, and they are not used as context for matching either. No
    /// source patch, at any pyramid level, contains an excluded pixel.
    pub exclude: Option<&'a [bool]>,
}

/// One pyramid level.
#[derive(Clone)]
struct Level {
    w: usize,
    h: usize,
    img: Vec<f32>,
    hole: Vec<bool>,
    /// Never a source (and not used as matching context); empty when nothing is excluded.
    excl: Vec<bool>,
}

/// Halve a level. A coarse pixel is a hole if any child is, and excluded if any child is
/// (conservative: no coarse source patch can cover an excluded fine pixel). Its value is the mean of
/// its usable children (neither hole nor excluded), else of its non-hole children.
fn downsample(l: &Level, ch: usize) -> Level {
    let (w, h) = (l.w.div_ceil(2), l.h.div_ceil(2));
    let mut img = vec![0.0f32; w * h * ch];
    let mut hole = vec![false; w * h];
    let has_ex = !l.excl.is_empty();
    let mut excl = if has_ex { vec![false; w * h] } else { Vec::new() };
    let mut acc = vec![0.0f32; 2 * ch];
    for y in 0..h {
        for x in 0..w {
            let (mut n_ok, mut n_ex) = (0.0f32, 0.0f32);
            acc.iter_mut().for_each(|v| *v = 0.0);
            let o = (y * w + x) * ch;
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (sx, sy) = (x * 2 + dx, y * 2 + dy);
                if sx >= l.w || sy >= l.h {
                    continue;
                }
                let si = sy * l.w + sx;
                if l.hole[si] {
                    hole[y * w + x] = true;
                    continue;
                }
                let ex = has_ex && l.excl[si];
                if ex {
                    excl[y * w + x] = true;
                }
                let (base, cnt) = if ex { (ch, &mut n_ex) } else { (0, &mut n_ok) };
                for c in 0..ch {
                    acc[base + c] += l.img[si * ch + c];
                }
                *cnt += 1.0;
            }
            let (base, n) = if n_ok > 0.0 { (0, n_ok) } else { (ch, n_ex) };
            if n > 0.0 {
                for c in 0..ch {
                    img[o + c] = acc[base + c] / n;
                }
            }
        }
    }
    Level { w, h, img, hole, excl }
}

/// Summed-area table of a boolean grid (`(w+1) × (h+1)`).
fn integral(w: usize, h: usize, m: &[bool]) -> Vec<u32> {
    let mut s = vec![0u32; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0u32;
        for x in 0..w {
            row += u32::from(m[y * w + x]);
            s[(y + 1) * (w + 1) + x + 1] = s[y * (w + 1) + x + 1] + row;
        }
    }
    s
}

/// Count of set cells in the clipped window `[x0, x1) × [y0, y1)`.
fn window_count(s: &[u32], gw: usize, gh: usize, x0: i32, y0: i32, x1: i32, y1: i32) -> u32 {
    let (x0, y0) = (x0.max(0) as usize, y0.max(0) as usize);
    let (x1, y1) = ((x1.max(0) as usize).min(gw), (y1.max(0) as usize).min(gh));
    if x1 <= x0 || y1 <= y0 {
        return 0;
    }
    let w1 = gw + 1;
    s[y1 * w1 + x1] + s[y0 * w1 + x0] - s[y0 * w1 + x1] - s[y1 * w1 + x0]
}

fn hole_bbox(w: usize, h: usize, hole: &[bool]) -> Option<(usize, usize, usize, usize)> {
    let mut b: Option<(usize, usize, usize, usize)> = None;
    for y in 0..h {
        for x in 0..w {
            if hole[y * w + x] {
                b = Some(match b {
                    None => (x, y, x + 1, y + 1),
                    Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1)),
                });
            }
        }
    }
    b
}

/// Per-level PatchMatch/EM state.
struct Solver<'a> {
    w: usize,
    h: usize,
    /// Colour channels, then `fs - ch` structure guide channels (see [`crate::structure`]).
    ch: usize,
    fs: usize,
    r: i32,
    /// Feature buffer, `fs` per pixel: colour (re-estimated by [`Solver::vote`]) and fixed guides.
    img: Vec<f32>,
    hole: &'a [bool],
    /// Valid source centres: the whole patch is inside the grid and contains no hole pixel.
    valid: Vec<bool>,
    valid_list: Vec<(i32, i32)>,
    /// Target centres (patch overlaps the hole), row-major.
    targets: Vec<bool>,
    /// Known pixels left out of the target side of the patch distance (excluded ones); may be empty.
    ignore: Vec<bool>,
    /// Line-continuation candidates per pixel (see [`crate::structure::line_candidates`]); may be empty.
    cand_idx: Vec<u32>,
    cands: Vec<[(i32, i32); 2]>,
    /// Cancellation: checked per row band; a cancelled solve's output is discarded.
    ctl: &'a (dyn Fn() -> bool + Sync),
}

impl Solver<'_> {
    #[inline]
    fn is_valid(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h && self.valid[y as usize * self.w + x as usize]
    }

    /// SSD between the (clipped) target patch at `t` and the source patch at `s`, per sample.
    fn dist(&self, t: (i32, i32), s: (i32, i32), cutoff: f32) -> f32 {
        let (w, h, ch, fs, r) = (self.w as i32, self.h as i32, self.ch, self.fs, self.r);
        let full = ((2 * r + 1) * (2 * r + 1)) as f32 * ch as f32;
        let mut sum = 0.0f32;
        let mut n = 0usize;
        for dy in -r..=r {
            let ty = t.1 + dy;
            if ty < 0 || ty >= h {
                continue;
            }
            let sy = s.1 + dy;
            for dx in -r..=r {
                let tx = t.0 + dx;
                if tx < 0 || tx >= w {
                    continue;
                }
                let tp = ty as usize * self.w + tx as usize;
                if self.ignore.get(tp).copied().unwrap_or(false) {
                    continue;
                }
                let sp = sy as usize * self.w + (s.0 + dx) as usize;
                let (ti, si) = (tp * fs, sp * fs);
                for (a, b) in self.img[ti..ti + fs].iter().zip(&self.img[si..si + fs]) {
                    let d = a - b;
                    sum += d * d;
                }
                n += ch;
            }
            // Early out: the mean can only be ≥ sum / (full patch size).
            if sum > cutoff * full {
                return f32::INFINITY;
            }
        }
        if n == 0 { f32::INFINITY } else { sum / n as f32 }
    }

    /// One PatchMatch pass over all target rows, in parallel bands (each band propagates within itself).
    fn patchmatch(&self, nnf: &mut [(i32, i32)], cost: &mut [f32], pass: usize, seed: u64, search_radius: i32) {
        const BAND: usize = 16;
        let w = self.w;
        let reverse = pass % 2 == 1;
        let band_fn = |bi: usize, nn: &mut [(i32, i32)], co: &mut [f32]| {
            if (self.ctl)() {
                return;
            }
            let mut rng = Rng::new(seed ^ (bi as u64).wrapping_mul(0x2545_F491_4F6C_DD1D) ^ ((pass as u64) << 40));
            let y0 = bi * BAND;
            let rows = nn.len() / w;
            let step: i32 = if reverse { -1 } else { 1 };
            for ry in 0..rows {
                let ly = if reverse { rows - 1 - ry } else { ry };
                let y = y0 + ly;
                for rx in 0..w {
                    let x = if reverse { w - 1 - rx } else { rx };
                    let li = ly * w + x;
                    if !self.targets[y * w + x] {
                        continue;
                    }
                    let t = (x as i32, y as i32);
                    let (mut best, mut bc) = (nn[li], co[li]);
                    // Propagation from the already-visited neighbours (within this band).
                    let nx = x as i32 - step;
                    if nx >= 0 && (nx as usize) < w {
                        let c = nn[ly * w + nx as usize];
                        let cand = (c.0 + step, c.1);
                        if self.is_valid(cand.0, cand.1) && cand != best {
                            let d = self.dist(t, cand, bc);
                            if d < bc {
                                (best, bc) = (cand, d);
                            }
                        }
                    }
                    let nly = ly as i32 - step;
                    if nly >= 0 && (nly as usize) < rows {
                        let c = nn[nly as usize * w + x];
                        let cand = (c.0, c.1 + step);
                        if self.is_valid(cand.0, cand.1) && cand != best {
                            let d = self.dist(t, cand, bc);
                            if d < bc {
                                (best, bc) = (cand, d);
                            }
                        }
                    }
                    // Line continuation: the nearest sources along a crossing line.
                    if let Some(cs) = self.cand_idx.get(y * w + x).and_then(|k| self.cands.get(*k as usize)) {
                        for cand in *cs {
                            if self.is_valid(cand.0, cand.1) && cand != best {
                                let d = self.dist(t, cand, bc);
                                if d < bc {
                                    (best, bc) = (cand, d);
                                }
                            }
                        }
                    }
                    // Random search in exponentially shrinking windows.
                    let mut rad = search_radius;
                    while rad >= 1 {
                        let cand = (best.0 + rng.range(-rad, rad), best.1 + rng.range(-rad, rad));
                        if self.is_valid(cand.0, cand.1) && cand != best {
                            let d = self.dist(t, cand, bc);
                            if d < bc {
                                (best, bc) = (cand, d);
                            }
                        }
                        rad /= 2;
                    }
                    nn[li] = best;
                    co[li] = bc;
                }
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        if w * self.h >= PAR_MIN {
            use rayon::prelude::*;
            nnf.par_chunks_mut(BAND * w).zip(cost.par_chunks_mut(BAND * w)).enumerate().for_each(|(bi, (nn, co))| band_fn(bi, nn, co));
            return;
        }
        for (bi, (nn, co)) in nnf.chunks_mut(BAND * w).zip(cost.chunks_mut(BAND * w)).enumerate() {
            band_fn(bi, nn, co);
        }
    }

    /// Re-estimate hole pixels by a similarity-weighted vote of all overlapping patch matches.
    fn vote(&mut self, nnf: &[(i32, i32)], cost: &[f32]) {
        let (w, h, ch, fs, r) = (self.w, self.h, self.ch, self.fs, self.r);
        // σ² from the 75th percentile of match costs (Wexler et al. use a robust scale like this).
        let mut cs: Vec<f32> = cost.iter().zip(&self.targets).filter(|(c, t)| **t && c.is_finite()).map(|(c, _)| *c).collect();
        let sigma2 = if cs.is_empty() {
            1.0
        } else {
            let k = (cs.len() * 3 / 4).min(cs.len() - 1);
            let (_, v, _) = cs.select_nth_unstable_by(k, |a, b| a.total_cmp(b));
            v.max(1e-6)
        };
        let img = &self.img;
        let row = |y: usize| -> Vec<(usize, Vec<f32>)> {
            let mut out = Vec::new();
            if (self.ctl)() {
                return out;
            }
            for x in 0..w {
                if !self.hole[y * w + x] {
                    continue;
                }
                let mut acc = vec![0.0f32; ch];
                let mut wsum = 0.0f32;
                for dy in -r..=r {
                    let ty = y as i32 + dy;
                    if ty < 0 || ty as usize >= h {
                        continue;
                    }
                    for dx in -r..=r {
                        let tx = x as i32 + dx;
                        if tx < 0 || tx as usize >= w {
                            continue;
                        }
                        let ti = ty as usize * w + tx as usize;
                        if !self.targets[ti] || !cost[ti].is_finite() {
                            continue;
                        }
                        let s = nnf[ti];
                        let (sx, sy) = (s.0 - dx, s.1 - dy);
                        let wt = (-cost[ti] / (2.0 * sigma2)).exp().max(1e-8);
                        let si = (sy as usize * w + sx as usize) * fs;
                        for c in 0..ch {
                            acc[c] += img[si + c] * wt;
                        }
                        wsum += wt;
                    }
                }
                if wsum > 0.0 {
                    acc.iter_mut().for_each(|v| *v /= wsum);
                    out.push((x, acc));
                }
            }
            out
        };
        #[cfg(not(target_arch = "wasm32"))]
        let rows: Vec<Vec<(usize, Vec<f32>)>> = if w * h >= PAR_MIN {
            use rayon::prelude::*;
            (0..h).into_par_iter().map(row).collect()
        } else {
            (0..h).map(row).collect()
        };
        #[cfg(target_arch = "wasm32")]
        let rows: Vec<Vec<(usize, Vec<f32>)>> = (0..h).map(row).collect();
        for (y, r) in rows.into_iter().enumerate() {
            for (x, v) in r {
                self.img[(y * w + x) * fs..(y * w + x) * fs + ch].copy_from_slice(&v);
            }
        }
    }
}

/// Content-aware completion of `hole` (see module docs). Returns `None` when there is no fully-known
/// patch to copy from (the caller can fall back to [`membrane_fill`]).
pub fn complete(w: usize, h: usize, ch: usize, img: &[f32], hole: &[bool], p: &CompleteParams) -> Option<Vec<f32>> {
    // Never cancelled, so never `Err`.
    complete_with(w, h, ch, img, hole, p, &|| false).unwrap_or(None)
}

/// [`complete`] that can be cancelled (`cancelled` is polled per EM iteration, PatchMatch pass and
/// row band): `Err(Cancelled)` once it returned `true`. `Ok(None)` also for mismatched buffer sizes
/// or `ch == 0`.
pub fn complete_with(
    w: usize,
    h: usize,
    ch: usize,
    img: &[f32],
    hole: &[bool],
    p: &CompleteParams,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Vec<f32>>, Cancelled> {
    complete_guided(w, h, ch, img, hole, &Guide::default(), p, cancelled)
}

/// [`complete_with`] with extra guidance: `guide.exclude` marks pixels that are never copied (see
/// [`Guide`]). `Ok(None)` when the exclude mask's length is not `w × h`, or when exclusion leaves no
/// fully-usable patch to copy from (fall back to [`membrane_fill`]).
pub fn complete_guided(
    w: usize,
    h: usize,
    ch: usize,
    img: &[f32],
    hole: &[bool],
    guide: &Guide,
    p: &CompleteParams,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Vec<f32>>, Cancelled> {
    if !p.seam_blend || Some(hole.len()) != w.checked_mul(h) || guide.exclude.is_some_and(|e| e.len() != hole.len()) {
        return complete_inner(w, h, ch, img, hole, guide, p, cancelled);
    }
    // Fill a ring of SEAM_RING known pixels too: there the fill can be compared with the photo, and
    // the mismatch is what `seam_blend` takes out.
    let ex = |i: usize| guide.exclude.and_then(|e| e.get(i)).copied().unwrap_or(false);
    let wide: Vec<bool> =
        crate::mask::dilate(w, h, hole, SEAM_RING).iter().enumerate().map(|(i, d)| *d && (hole.get(i).copied().unwrap_or(false) || !ex(i))).collect();
    let Some(mut out) = complete_inner(w, h, ch, img, &wide, guide, p, cancelled)? else {
        return complete_inner(w, h, ch, img, hole, guide, p, cancelled);
    };
    seam_blend(w, h, ch, img, &mut out, hole, &wide);
    Ok(Some(out))
}

/// Width of the ring of known pixels [`complete_guided`] also fills to measure the seam.
const SEAM_RING: usize = 2;

#[allow(clippy::too_many_arguments)]
fn complete_inner(
    w: usize,
    h: usize,
    ch: usize,
    img: &[f32],
    hole: &[bool],
    guide: &Guide,
    p: &CompleteParams,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Vec<f32>>, Cancelled> {
    let check = || if cancelled() { Err(Cancelled) } else { Ok(()) };
    if ch == 0 || Some(hole.len()) != w.checked_mul(h) || Some(img.len()) != hole.len().checked_mul(ch) {
        return Ok(None);
    }
    let excl: Vec<bool> = match guide.exclude {
        Some(e) if e.len() != hole.len() => return Ok(None),
        Some(e) if e.iter().any(|v| *v) => e.to_vec(),
        _ => Vec::new(),
    };
    let Some((bx0, by0, bx1, by1)) = hole_bbox(w, h, hole) else { return Ok(Some(img.to_vec())) };
    let r = p.patch_radius.max(1);
    let psz = 2 * r + 1;
    // Pyramid: shrink until the hole is a few patches across.
    let mut levels = vec![Level { w, h, img: img.to_vec(), hole: hole.to_vec(), excl }];
    let mut ext = (bx1 - bx0).max(by1 - by0);
    let mut exts = vec![ext];
    while let Some(l) = levels.last() {
        check()?;
        if ext <= 2 * psz || l.w / 2 < 3 * psz || l.h / 2 < 3 * psz {
            break;
        }
        let d = downsample(l, ch);
        levels.push(d);
        ext = ext.div_ceil(2);
        exts.push(ext);
    }
    let nlev = levels.len();
    // Structure pass: straight lines crossing the hole, found once, on the finest level where the
    // hole spans at most `DETECT_EXTENT` pixels (big holes: a coarser level, much cheaper, and the
    // line's long support keeps the fit precise), in full-resolution coordinates.
    let lines = if p.structure > 0.0 && p.structure.is_finite() {
        let ld = exts.iter().position(|e| *e <= DETECT_EXTENT).unwrap_or(nlev - 1);
        match levels.get(ld) {
            Some(l) => {
                let ex = (!l.excl.is_empty()).then_some(l.excl.as_slice());
                let found = crate::structure::detect_lines(l.w, l.h, ch, &l.img, &l.hole, ex, cancelled);
                found.iter().map(|ln| ln.to_full_res((1u64 << ld.min(30)) as f32)).collect()
            }
            None => Vec::new(),
        }
    } else {
        Vec::new()
    };
    check()?;
    let mut nnf: Vec<(i32, i32)> = Vec::new();
    let mut prev: Option<(usize, usize, Vec<f32>)> = None;
    for li in (0..nlev).rev() {
        check()?;
        let Level { w: lw, h: lh, img: mut limg, hole: lhole, excl: lexcl } = levels[li].clone();
        let has_ex = !lexcl.is_empty();
        // Initialise the hole: membrane at the coarsest level, upsampled result elsewhere.
        match &prev {
            None => {
                if has_ex {
                    // Excluded pixels are no boundary values either: solve over them, then restore.
                    let unknown: Vec<bool> = lhole.iter().zip(&lexcl).map(|(h, e)| *h || *e).collect();
                    let filled = membrane_fill(lw, lh, ch, &limg, &unknown);
                    for (i, hh) in lhole.iter().enumerate() {
                        if *hh {
                            limg[i * ch..(i + 1) * ch].copy_from_slice(&filled[i * ch..(i + 1) * ch]);
                        }
                    }
                } else {
                    limg = membrane_fill(lw, lh, ch, &limg, &lhole);
                }
            }
            Some((pw, ph, pimg)) => {
                for y in 0..lh {
                    for x in 0..lw {
                        if lhole[y * lw + x] {
                            let (sx, sy) = ((x / 2).min(pw - 1), (y / 2).min(ph - 1));
                            let (d, s) = ((y * lw + x) * ch, (sy * pw + sx) * ch);
                            limg[d..d + ch].copy_from_slice(&pimg[s..s + ch]);
                        }
                    }
                }
            }
        }
        let ri = r as i32;
        let hs = integral(lw, lh, &lhole);
        // Blocked for sources: hole or excluded.
        let bs = if has_ex {
            let blocked: Vec<bool> = lhole.iter().zip(&lexcl).map(|(h, e)| *h || *e).collect();
            integral(lw, lh, &blocked)
        } else {
            hs.clone()
        };
        let mut valid = vec![false; lw * lh];
        let mut valid_list = Vec::new();
        let mut targets = vec![false; lw * lh];
        for y in 0..lh as i32 {
            if y % 32 == 0 {
                check()?;
            }
            for x in 0..lw as i32 {
                let cnt = window_count(&hs, lw, lh, x - ri, y - ri, x + ri + 1, y + ri + 1);
                let inside = x >= ri && y >= ri && x + ri < lw as i32 && y + ri < lh as i32;
                let i = y as usize * lw + x as usize;
                if inside && window_count(&bs, lw, lh, x - ri, y - ri, x + ri + 1, y + ri + 1) == 0 {
                    valid[i] = true;
                    valid_list.push((x, y));
                }
                targets[i] = cnt > 0;
            }
        }
        if valid_list.is_empty() {
            if li == 0 {
                return Ok(None);
            }
            prev = Some((lw, lh, limg));
            nnf.clear();
            continue;
        }
        let ignore: Vec<bool> = if has_ex { lexcl.iter().zip(&lhole).map(|(e, h)| *e && !h).collect() } else { Vec::new() };
        // Structure guide in this level's pixels.
        let scale = (1u64 << li.min(30)) as f32;
        let band = (2 * r + 2) as f32;
        let gc = lines.len();
        let fs = ch + gc;
        let (feat, (cand_idx, cands)) = if gc == 0 {
            (std::mem::take(&mut limg), (Vec::new(), Vec::new()))
        } else {
            let g = crate::structure::guide_channels(&lines, lw, lh, scale, band, p.structure * band);
            let mut feat = vec![0.0f32; lw * lh * fs];
            for ((f, c), gg) in feat.chunks_exact_mut(fs).zip(limg.chunks_exact(ch)).zip(g.chunks_exact(gc)) {
                f[..ch].copy_from_slice(c);
                f[ch..].copy_from_slice(gg);
            }
            let vb = |x: i32, y: i32| x >= 0 && y >= 0 && (x as usize) < lw && (y as usize) < lh && valid[y as usize * lw + x as usize];
            let c = crate::structure::line_candidates(&lines, lw, lh, scale, band, &targets, &vb);
            (feat, c)
        };
        check()?;
        let mut solver =
            Solver { w: lw, h: lh, ch, fs, r: ri, img: feat, hole: &lhole, valid, valid_list, targets, ignore, cand_idx, cands, ctl: cancelled };
        // NNF init: upsampled from the coarser level where possible, then a line candidate, random
        // otherwise.
        let mut rng = Rng::new(p.seed ^ (li as u64) << 20);
        let pw_prev = prev.as_ref().map_or(0, |p| p.0);
        let old = std::mem::take(&mut nnf);
        let mut new_nnf = vec![(0i32, 0i32); lw * lh];
        for y in 0..lh {
            if y % 32 == 0 {
                check()?;
            }
            for x in 0..lw {
                if !solver.targets[y * lw + x] {
                    continue;
                }
                let mut cand = None;
                if !old.is_empty() && pw_prev > 0 {
                    let pi = (y / 2) * pw_prev + (x / 2);
                    if let Some(&(sx, sy)) = old.get(pi) {
                        let c = (sx * 2 + (x % 2) as i32, sy * 2 + (y % 2) as i32);
                        if solver.is_valid(c.0, c.1) {
                            cand = Some(c);
                        }
                    }
                }
                if cand.is_none() {
                    cand = solver
                        .cand_idx
                        .get(y * lw + x)
                        .and_then(|k| solver.cands.get(*k as usize))
                        .and_then(|cs| cs.iter().copied().find(|c| solver.is_valid(c.0, c.1)));
                }
                new_nnf[y * lw + x] = cand.unwrap_or_else(|| solver.valid_list[(rng.next() % solver.valid_list.len() as u64) as usize]);
            }
        }
        nnf = new_nnf;
        let refine_only = li + 1 < nlev && exts[li] > p.refine_extent;
        let em = if li + 1 == nlev {
            p.em_iters
        } else if refine_only {
            1
        } else {
            (p.em_iters >> (nlev - 1 - li)).max(p.min_em_iters)
        };
        let pm_iters = if refine_only { 1 } else { p.pm_iters };
        let full_search = (lw.max(lh)) as i32;
        let mut cost = vec![f32::INFINITY; lw * lh];
        for it in 0..em {
            // Costs change as hole pixels are re-estimated: refresh before each PatchMatch.
            for (i, c) in cost.iter_mut().enumerate() {
                if i % (lw * 32).max(1) == 0 {
                    check()?;
                }
                if solver.targets[i] {
                    *c = solver.dist(((i % lw) as i32, (i / lw) as i32), nnf[i], f32::INFINITY);
                }
            }
            // Wide random search on coarse levels / first iteration, local refinement afterwards.
            let radius = if refine_only {
                2
            } else if li + 1 == nlev || it == 0 {
                full_search
            } else {
                (4 * psz as i32).min(full_search)
            };
            for pass in 0..pm_iters {
                check()?;
                solver.patchmatch(&mut nnf, &mut cost, pass + it * pm_iters, p.seed.wrapping_add((li * 1000 + it) as u64), radius);
            }
            check()?;
            solver.vote(&nnf, &cost);
            check()?;
        }
        let limg = if fs == ch { solver.img } else { solver.img.chunks_exact(fs).flat_map(|f| f[..ch].iter().copied()).collect() };
        prev = Some((lw, lh, limg));
    }
    Ok(prev.map(|(_, _, img)| img))
}

/// Seamless edges (in the spirit of P. Pérez, M. Gangnet, A. Blake, *Poisson Image Editing*,
/// SIGGRAPH 2003, applied as a membrane correction). `out` was completed over `wide` (the hole and a
/// ring of known pixels around it), so on that ring it can be compared with the photo: the
/// mismatch (smoothed along the ring, so the photo's grain is not copied in) is spread smoothly over
/// the hole and added, and the ring gets the photo's pixels back. A copied block whose far end did not
/// match the photo (a band continued from one side of a long hole) then meets it without a hard,
/// straight seam, while a fill that already matches (the mismatch is zero) is left as it is.
pub fn seam_blend(w: usize, h: usize, ch: usize, orig: &[f32], out: &mut [f32], hole: &[bool], wide: &[bool]) {
    let n = w * h;
    if hole.len() != n || wide.len() != n || orig.len() != n * ch || out.len() != n * ch || ch == 0 {
        return;
    }
    let ring: Vec<bool> = (0..n).map(|i| wide[i] && !hole[i]).collect();
    // smoothed mismatch on the ring (ring pixels within R px)
    const R: i64 = 4;
    let mut fix = vec![0.0f32; n * ch];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if !ring[i] {
                continue;
            }
            let mut k = 0.0f32;
            let mut acc = vec![0.0f32; ch];
            for dy in -R..=R {
                for dx in -R..=R {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    if nx < 0 || ny < 0 || nx as usize >= w || ny as usize >= h {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if ring[j] {
                        for (c, a) in acc.iter_mut().enumerate() {
                            *a += orig.get(j * ch + c).copied().unwrap_or(0.0) - out.get(j * ch + c).copied().unwrap_or(0.0);
                        }
                        k += 1.0;
                    }
                }
            }
            for (c, a) in acc.iter().enumerate() {
                if let Some(f) = fix.get_mut(i * ch + c) {
                    *f = a / k.max(1.0);
                }
            }
        }
    }
    // spread over the hole (the ring is the boundary; outside it there is nothing to correct)
    let field = crate::membrane::membrane_fill(w, h, ch, &fix, hole);
    for i in 0..n {
        for c in 0..ch {
            let k = i * ch + c;
            if hole[i] {
                if let (Some(o), Some(f)) = (out.get_mut(k), field.get(k))
                    && f.is_finite()
                {
                    *o += f;
                }
            } else if let (Some(o), Some(v)) = (out.get_mut(k), orig.get(k)) {
                // the ring (and everything else outside the hole) is the photo again
                *o = *v;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smooth gradient plus deterministic fine noise.
    fn texture(w: usize, h: usize) -> Vec<f32> {
        let mut rng = Rng::new(7);
        (0..w * h)
            .flat_map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                let n = (rng.next() % 1000) as f32 / 1000.0 * 0.04 - 0.02;
                [0.3 + 0.004 * x + n, 0.5 + 0.002 * y + n, 0.4 + n]
            })
            .collect()
    }

    fn disc(w: usize, h: usize, cx: f32, cy: f32, r: f32) -> Vec<bool> {
        (0..w * h).map(|i| ((i % w) as f32 - cx).hypot((i / w) as f32 - cy) < r).collect()
    }

    fn hole_rmse(a: &[f32], b: &[f32], hole: &[bool], ch: usize) -> f32 {
        let (mut e, mut n) = (0.0f32, 0);
        for (i, hh) in hole.iter().enumerate() {
            if *hh {
                for c in 0..ch {
                    e += (a[i * ch + c] - b[i * ch + c]).powi(2);
                    n += 1;
                }
            }
        }
        (e / n as f32).sqrt()
    }

    #[test]
    fn completion_fills_blemish_from_texture() {
        let (w, h, ch) = (96, 96, 3);
        let clean = texture(w, h);
        let hole = disc(w, h, 48.0, 48.0, 9.0);
        let mut dirty = clean.clone();
        for (i, hh) in hole.iter().enumerate() {
            if *hh {
                dirty[i * ch..i * ch + ch].copy_from_slice(&[0.02, 0.02, 0.02]);
            }
        }
        let out = complete(w, h, ch, &dirty, &hole, &CompleteParams::default()).unwrap();
        let before = hole_rmse(&dirty, &clean, &hole, ch);
        let after = hole_rmse(&out, &clean, &hole, ch);
        assert!(after < 0.06 && after < before * 0.15, "rmse before {before} after {after}");
        // Outside the hole untouched.
        assert!(out.iter().zip(&dirty).zip(hole.iter().flat_map(|h| [*h; 3])).all(|((a, b), h)| h || a == b));
        // Deterministic.
        assert_eq!(out, complete(w, h, ch, &dirty, &hole, &CompleteParams::default()).unwrap());
    }

    #[test]
    fn completion_continues_structure() {
        // Vertical stripes: the filled hole should keep the stripes (not a blur).
        let (w, h, ch) = (80, 80, 1);
        let clean: Vec<f32> = (0..w * h).map(|i| if (i % w / 4) % 2 == 0 { 0.2 } else { 0.8 }).collect();
        let hole = disc(w, h, 40.0, 40.0, 7.0);
        let dirty: Vec<f32> = clean.iter().zip(&hole).map(|(c, h)| if *h { 0.5 } else { *c }).collect();
        let out = complete(w, h, ch, &dirty, &hole, &CompleteParams::default()).unwrap();
        let err = hole_rmse(&out, &clean, &hole, ch);
        assert!(err < 0.15, "{err}");
    }
}
