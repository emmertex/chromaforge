//! Hot pixels on sensors that don't list their defects.
//!
//! A hot pixel is a single photosite that reads far too bright: a leaky or stuck cell, most visible
//! at high ISO and in dark areas. Demosaicing turns it into a small saturated dot of its colour
//! (red, green or blue), and colour noise reduction can only make that dot fainter, since the photosite
//! is too bright, not just the wrong colour. DNGs list such pixels in their opcodes and Panasonic marks
//! them in the data (both repaired elsewhere); other raws, such as Sony ARWs, carry no list.
//!
//! [`RawImage::suppress_hot_pixels`] finds them in the mosaic, before demosaicing:
//!
//! 1. **Noise model.** On a sparse sample of the image, each photosite is compared with the median of
//!    its same-colour neighbours. The spread of that difference, per brightness, gives the noise
//!    σ(level) as `σ² = a · level + b` (shot noise plus read noise), fitted to the image itself.
//! 2. **Candidates.** A photosite more than [`K_HOT`] σ above *every* same-colour neighbour (the
//!    nearest eight, two photosites away on a Bayer grid, the nearest greens for green).
//! 3. **Real light is kept.** A star, a spark or a specular point lights more than one photosite: its
//!    direct neighbours are raised too. A candidate with at least [`MIN_LIT`] neighbours more than
//!    [`K_SPOT`] σ above their own surroundings is left alone. Hot photosites also come in adjacent
//!    pairs, so a single lit neighbour doesn't count.
//! 4. The rest are replaced by the median of their same-colour neighbours.
//!
//! If more than [`MAX_SHARE`] of the photosites would be replaced, the image doesn't fit the model
//! (a synthetic pattern, a star field shot through a pinhole) and nothing is changed.

use crate::cfa_offsets::{FlatOffsets, MAX_NEIGHBOURS, MIN_NEIGHBOURS};
use crate::{RawData, RawImage};
use rayon::prelude::*;

/// A candidate stands more than this many σ above the brightest of its same-colour neighbours (pure noise
/// hardly reaches 4.5 σ above the brightest of eight; measured on two ILCE-7M4 files at ISO 3200 and 25600).
const K_HOT: f32 = 5.0;
/// A direct neighbour this many σ above its own surroundings means the light is real.
const K_SPOT: f32 = 4.0;
/// ...and it takes this many lit neighbours (a hot pair has one).
const MIN_LIT: usize = 2;
/// Every this many rows and columns are sampled for the noise model.
const SAMPLE_STEP: usize = 5;
/// Brightness bins of the noise model (on a square-root scale), and the samples a bin needs.
const BINS: usize = 32;
const MIN_BIN_SAMPLES: usize = 64;
/// Photosites this close to the active area's edge are left alone (their neighbourhood is cut).
const MARGIN: usize = 4;
/// More replaced than this share of the photosites and the result is discarded.
const MAX_SHARE: f64 = 2e-3;

/// How far (photosites, each way) [`phases`] looks for same-colour neighbours. [`MARGIN`] must cover it, and the
/// 1 photosite ring of direct neighbours [`lit_around`] looks at on top of it.
const WINDOW_RADIUS: isize = 2;

/// For one position in the CFA pattern: the offsets of the nearest same-colour photosites (at most
/// [`MAX_NEIGHBOURS`], within [`WINDOW_RADIUS`] photosites) and of the 8 direct neighbours.
#[derive(Clone)]
struct Phase {
    same: Vec<(isize, isize)>,
    around: [(isize, isize); 8],
}

fn phases(cfa: &crate::Cfa) -> Vec<Phase> {
    let around = [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)];
    let (pw, ph) = (cfa.width, cfa.height);
    (0..ph)
        .flat_map(|j| (0..pw).map(move |i| (i, j)))
        .map(|(i, j)| {
            // anchored far enough from 0 that i + dx stays positive
            let (x0, y0) = (i + WINDOW_RADIUS as usize * pw, j + WINDOW_RADIUS as usize * ph);
            let c = cfa.color_at(x0, y0);
            let mut same: Vec<(isize, isize)> = (-WINDOW_RADIUS..=WINDOW_RADIUS)
                .flat_map(|dy| (-WINDOW_RADIUS..=WINDOW_RADIUS).map(move |dx| (dx, dy)))
                .filter(|&(dx, dy)| (dx, dy) != (0, 0) && cfa.color_at((x0 as isize + dx) as usize, (y0 as isize + dy) as usize) == c)
                .collect();
            same.sort_by_key(|&(dx, dy)| dx * dx + dy * dy);
            same.truncate(MAX_NEIGHBOURS);
            Phase { same, around }
        })
        .collect()
}

/// The noise σ at a brightness (in raw counts above black): `sqrt(a · level + b)`.
#[derive(Clone, Copy, Debug)]
struct Noise {
    a: f32,
    b: f32,
}

impl Noise {
    /// σ never goes below this (counts): a photosite no more than `K_HOT * MIN_SIGMA` counts above its brightest
    /// neighbour can't be a candidate whatever the model says, which the candidate test uses to skip the median.
    const MIN_SIGMA: f32 = 1.0;

    fn sigma(&self, level: f32) -> f32 {
        (self.a * level.max(0.0) + self.b).max(Self::MIN_SIGMA).sqrt()
    }
}

fn median(v: &mut [f32]) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let n = v.len();
    // The noise model puts thousands of samples in each brightness bin. Only its middle
    // value(s) are needed: partition once instead of ordering the whole bin. Keep sorting the
    // tiny neighbourhoods, for which partitioning has no advantage.
    if n > 16 {
        let (lower, middle, _) = v.select_nth_unstable_by(n / 2, f32::total_cmp);
        let upper = *middle;
        return if n % 2 == 1 { upper } else { 0.5 * (lower.iter().copied().max_by(f32::total_cmp).unwrap_or(upper) + upper) };
    }
    if n == 8 && v.iter().all(|x| !x.is_nan()) {
        // An 18-comparator network that puts the four smallest in t[0..4] and the four largest in t[4..8], but is NOT a full
        // sort: t[3] and t[4] may come out swapped. Only their sum is guaranteed to equal the sorted pair's, and that is all
        // the median needs (0.5 * (a + b) is symmetric). Don't read t[3] or t[4] alone.
        // crates/pipeline/src/nrc_joint.rs has its own `median8` network, with another comparator order and another NaN
        // policy (this one is only taken when no value is NaN); crates/raw cannot depend on pipeline, so the two are
        // separate on purpose: a change to one does not carry over to the other.
        let mut t = [v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7]];
        for (i, j) in [
            (0, 1),
            (2, 3),
            (4, 5),
            (6, 7),
            (0, 2),
            (1, 3),
            (4, 6),
            (5, 7),
            (1, 2),
            (5, 6),
            (0, 4),
            (3, 7),
            (1, 5),
            (2, 6),
            (1, 4),
            (3, 6),
            (2, 4),
            (3, 5),
        ] {
            let (lo, hi) = (t[i].min(t[j]), t[i].max(t[j]));
            t[i] = lo;
            t[j] = hi;
        }
        return 0.5 * (t[3] + t[4]);
    }
    v.sort_by(f32::total_cmp);
    if n % 2 == 1 { v[n / 2] } else { 0.5 * (v[n / 2 - 1] + v[n / 2]) }
}

/// The sample view the passes work on: the u16 mosaic, its width, the active area, the CFA phases.
struct View<'a> {
    d: &'a [u16],
    w: usize,
    pw: usize,
    ph: usize,
    phases: &'a [Phase],
}

impl View<'_> {
    /// Value at full-image `(x, y)`; callers keep `(x, y)` inside the image.
    #[inline]
    fn at(&self, x: isize, y: isize) -> f32 {
        self.d[y as usize * self.w + x as usize] as f32
    }
    #[inline]
    fn phase(&self, x: usize, y: usize) -> &Phase {
        &self.phases[(y % self.ph) * self.pw + (x % self.pw)]
    }
    /// The same-colour neighbours' values around `(x, y)` into `buf`, and their maximum.
    #[inline]
    fn neighbours(&self, x: usize, y: usize, buf: &mut Vec<f32>) -> f32 {
        buf.clear();
        let mut max = f32::MIN;
        for &(dx, dy) in &self.phase(x, y).same {
            let v = self.at(x as isize + dx, y as isize + dy);
            max = max.max(v);
            buf.push(v);
        }
        max
    }
}

impl RawImage {
    /// Replace isolated hot photosites by the median of their same-colour neighbours (see the module docs).
    /// Returns how many were replaced. Only single-plane integer CFA data is examined.
    pub fn suppress_hot_pixels(&mut self) -> usize {
        let Some(cfa) = self.cfa.clone() else { return 0 };
        let (w, h, a) = (self.width, self.height, self.active_area);
        let black = self.black.mean();
        let RawData::U16(d) = &mut self.data else { return 0 };
        if self.cpp != 1
            || !cfa.valid()
            || w.checked_mul(h) != Some(d.len())
            || a.width < 4 * MARGIN
            || a.height < 4 * MARGIN
            || a.x + a.width > w
            || a.y + a.height > h
            || !black.is_finite()
        {
            return 0;
        }
        let phases = phases(&cfa);
        if !phases_usable(&phases) {
            return 0;
        }
        let (x0, x1, y0, y1) = (a.x + MARGIN, a.x + a.width - MARGIN, a.y + MARGIN, a.y + a.height - MARGIN);
        let range = (self.white.iter().copied().fold(0.0f32, f32::max) - black).max(1.0);
        let fixes = {
            let v = View { d: d.as_slice(), w, pw: cfa.width, ph: cfa.height, phases: &phases };
            let Some(noise) = noise_model(&v, (x0, x1, y0, y1), black, range) else { return 0 };
            let candidates = candidates(&v, (x0, x1, y0, y1), black, noise);
            let sites = (x1 - x0) * (y1 - y0);
            if candidates.len() as f64 > MAX_SHARE * sites as f64 {
                return 0;
            }
            let mut buf = Vec::with_capacity(8);
            let n_cand = candidates.len();
            let kept: Vec<_> = candidates.into_iter().filter(|&(x, y, _)| !lit_around(&v, x, y, black, noise, &mut buf)).collect();
            // (full-image passes for tuning, seconds a photo: on their own switch, not with every profile)
            if lcc_brand::env_is_set("PROFILE_HOT_PIXELS") {
                eprintln!(
                    "  hot pixels: noise σ² = {:.3}·L + {:.1} (σ at 0: {:.1}, at 1 %: {:.1}), {n_cand} candidates, {} real light",
                    noise.a,
                    noise.b,
                    noise.sigma(0.0),
                    noise.sigma(0.01 * range),
                    n_cand - kept.len()
                );
                // the strongest excesses below the threshold, to see how far the dots are
                let mut hist = [0usize; 12];
                let mut buf2 = Vec::with_capacity(8);
                for y in (y0..y1).step_by(1) {
                    for x in x0..x1 {
                        let s = v.at(x as isize, y as isize);
                        let max = v.neighbours(x, y, &mut buf2);
                        if s <= max {
                            continue;
                        }
                        let m = median(&mut buf2);
                        let k = ((s - max) / noise.sigma(m - black)) as usize;
                        hist[k.min(11)] += 1;
                    }
                }
                eprintln!("  hot pixels: photosites by (s - max neighbour) / σ, 0..11+: {hist:?}");
                // very bright photosites in dark surroundings, whatever the verdict
                let mut bright = 0;
                let mut buf4 = Vec::with_capacity(8);
                for y in y0..y1 {
                    for x in x0..x1 {
                        let s = v.at(x as isize, y as isize) - black;
                        if s < 1500.0 {
                            continue;
                        }
                        v.neighbours(x, y, &mut buf4);
                        let m = median(&mut buf4) - black;
                        if m > 300.0 {
                            continue;
                        }
                        bright += 1;
                        if bright <= 4 {
                            eprintln!(
                                "  bright at ({x}, {y}), colour {}, s {s:.0}, median {m:.0}, lit {}:",
                                cfa.color_at(x, y),
                                lit_around(&v, x, y, black, noise, &mut buf4)
                            );
                            for dy in -3isize..=3 {
                                let row: Vec<String> =
                                    (-3isize..=3).map(|dx| format!("{:6.0}", v.at(x as isize + dx, y as isize + dy) - black)).collect();
                                eprintln!("    {}", row.join(" "));
                            }
                        }
                    }
                }
                eprintln!("  bright photosites in dark surroundings: {bright}");
                // a few candidates rejected as real light: their 5 × 5 neighbourhood above black
                let mut shown = 0;
                let mut buf3 = Vec::with_capacity(8);
                for y in y0..y1 {
                    for x in x0..x1 {
                        if shown >= 6 {
                            break;
                        }
                        let s = v.at(x as isize, y as isize);
                        let max = v.neighbours(x, y, &mut buf3);
                        if s <= max {
                            continue;
                        }
                        let m = median(&mut buf3);
                        if s - max > 8.0 * noise.sigma(m - black) && lit_around(&v, x, y, black, noise, &mut buf3) {
                            shown += 1;
                            eprintln!("  rejected at ({x}, {y}), colour {}:", cfa.color_at(x, y));
                            for dy in -2isize..=2 {
                                let row: Vec<String> =
                                    (-2isize..=2).map(|dx| format!("{:6.0}", v.at(x as isize + dx, y as isize + dy) - black)).collect();
                                eprintln!("    {}", row.join(" "));
                            }
                        }
                    }
                }
            }
            kept
        };
        for &(x, y, r) in &fixes {
            d[y * w + x] = r;
        }
        fixes.len()
    }
}

/// Fit `σ² = a · level + b` to the spread of (photosite − median of its same-colour neighbours) on a sample.
fn noise_model(v: &View<'_>, (x0, x1, y0, y1): (usize, usize, usize, usize), black: f32, range: f32) -> Option<Noise> {
    let bin_of = |level: f32| (((level.max(0.0) / range).sqrt() * BINS as f32) as usize).min(BINS - 1);
    let rows: Vec<usize> = (y0..y1).step_by(SAMPLE_STEP).collect();
    let per_row: Vec<Vec<(usize, f32, f32)>> = rows
        .par_iter()
        .map(|&y| {
            let mut buf = Vec::with_capacity(8);
            (x0..x1)
                .step_by(SAMPLE_STEP)
                .map(|x| {
                    v.neighbours(x, y, &mut buf);
                    let m = median(&mut buf);
                    let level = m - black;
                    (bin_of(level), level, (v.at(x as isize, y as isize) - m).abs())
                })
                .collect()
        })
        .collect();
    let mut bins: Vec<(Vec<f32>, f64, usize)> = vec![(Vec::new(), 0.0, 0); BINS];
    for (b, level, dev) in per_row.into_iter().flatten() {
        bins[b].0.push(dev);
        bins[b].1 += level as f64;
        bins[b].2 += 1;
    }
    // per bin: mean level and σ from the median absolute deviation (robust to the hot pixels themselves)
    let pts: Vec<(f64, f64, f64)> = bins
        .into_iter()
        .filter(|(dev, _, n)| *n >= MIN_BIN_SAMPLES && !dev.is_empty())
        .map(|(mut dev, sum, n)| {
            let s = 1.4826 * median(&mut dev) as f64;
            (sum / n as f64, s * s, n as f64)
        })
        .collect();
    if pts.is_empty() {
        return None;
    }
    // weighted least squares for σ² = a·L + b, both kept ≥ 0
    let (sw, sl, sv) = pts.iter().fold((0.0, 0.0, 0.0), |(w, l, s), &(lv, var, n)| (w + n, l + n * lv, s + n * var));
    let (ml, mv) = (sl / sw, sv / sw);
    let (cov, varl) = pts.iter().fold((0.0, 0.0), |(c, vl), &(lv, var, n)| (c + n * (lv - ml) * (var - mv), vl + n * (lv - ml) * (lv - ml)));
    let a = if varl > 0.0 { (cov / varl).max(0.0) } else { 0.0 };
    let b = (mv - a * ml).max(0.0);
    (a.is_finite() && b.is_finite()).then_some(Noise { a: a as f32, b: b as f32 })
}

/// Can every phase be tested? Each needs [`MIN_NEIGHBOURS`] same-colour neighbours (a statistic of fewer isn't one).
/// This is the one place that guarantees it: [`accept`] is only called for phases that passed it, and gives no verdict
/// for a phase with none (where the plain loop's `f32::MIN`-seeded maximum would have accepted every photosite).
fn phases_usable(phases: &[Phase]) -> bool {
    phases.iter().all(|p| p.same.len() >= MIN_NEIGHBOURS)
}

fn flat_phases(phases: &[Phase], w: usize) -> Vec<FlatOffsets> {
    phases.iter().map(|p| FlatOffsets::new(&p.same, w)).collect()
}

/// The candidate rule, once, for [`candidate_at`] and the fast loop of [`candidates`]: given a photosite's value and
/// the values of its `n` same-colour neighbours (the first `n` of `nb`, the rest 0), the replacement value (their median)
/// if it is more than [`K_HOT`] σ above the brightest of them, else `None`. The comparison is strict, and made in `f32`
/// on the counts. `None` too for no neighbour (no verdict; see [`phases_usable`]) or more than the cap.
#[inline(always)]
fn accept(site: u16, nb: &[u16; MAX_NEIGHBOURS], n: usize, black: f32, noise: Noise) -> Option<u16> {
    if n == 0 || n > MAX_NEIGHBOURS {
        return None;
    }
    // (the entries of `nb` past `n` are 0, which no maximum of counts notices)
    let max = nb.iter().copied().max().unwrap_or(0);
    let excess = f32::from(site) - f32::from(max);
    // (σ is never below MIN_SIGMA, so an excess of K_HOT * MIN_SIGMA counts or less can't pass the test below: skip the median)
    if excess <= K_HOT * Noise::MIN_SIGMA {
        return None;
    }
    let mut buf: [f32; MAX_NEIGHBOURS] = std::array::from_fn(|k| f32::from(nb[k]));
    let m = if n == MAX_NEIGHBOURS { median(&mut buf) } else { median(buf.get_mut(..n)?) };
    (excess > K_HOT * noise.sigma(m - black)).then(|| m.round().clamp(0.0, u16::MAX as f32) as u16)
}

/// The candidate test of one photosite, with bounds-checked reads: the replacement value if it is far above every
/// same-colour neighbour. (The fast loop in [`candidates`] makes the same test through [`accept`]; this one serves what
/// it doesn't cover: phases with fewer than 8 neighbours, rows near the edge of the buffer.)
fn candidate_at(d: &[u16], idx: usize, phase: &FlatOffsets, black: f32, noise: Noise) -> Option<u16> {
    let site = *d.get(idx)?;
    let mut vals = [0u16; MAX_NEIGHBOURS];
    for (slot, &o) in vals.iter_mut().zip(phase.offsets()) {
        *slot = *d.get((idx as isize).wrapping_add(o) as usize)?;
    }
    accept(site, &vals, phase.n, black, noise)
}

/// Photosites far above every same-colour neighbour, with the value to replace them by.
///
/// The hot loop of the pass (every photosite of the active area), so it is written for speed: photosites are
/// visited phase by phase along a row (the neighbour offsets are the same for all of them), the eight neighbours
/// are read at flat offsets into a fixed array, and the median is only taken for photosites that can pass (see
/// [`accept`], the rule both this loop and [`candidate_at`] apply). Same photosites, same replacement values and the same
/// order as the plain per-photosite loop (`candidates_plain` in the tests).
fn candidates(v: &View<'_>, (x0, x1, y0, y1): (usize, usize, usize, usize), black: f32, noise: Noise) -> Vec<(usize, usize, u16)> {
    if v.pw == 0 || v.ph == 0 {
        return Vec::new();
    }
    let flat = flat_phases(v.phases, v.w);
    (y0..y1)
        .into_par_iter()
        .flat_map_iter(|y| {
            let mut out = Vec::new();
            let row = y * v.w;
            let phase_row = (y % v.ph) * v.pw;
            for c in 0..v.pw {
                let start = x0 + c;
                let Some(phase) = flat.get(phase_row + start % v.pw).copied() else { continue };
                // every read of this row and phase is inside the buffer (so the fast loop can index it)
                if phase.n == MAX_NEIGHBOURS && phase.row_inside(row, start, x1, v.d.len()) {
                    let o = phase.off;
                    for x in (start..x1).step_by(v.pw) {
                        let idx = (row + x) as isize;
                        let site = v.d[idx as usize];
                        let nb: [u16; MAX_NEIGHBOURS] = std::array::from_fn(|k| v.d[(idx + o[k]) as usize]);
                        if let Some(r) = accept(site, &nb, MAX_NEIGHBOURS, black, noise) {
                            out.push((x, y, r));
                        }
                    }
                } else {
                    for x in (start..x1).step_by(v.pw) {
                        if let Some(r) = candidate_at(v.d, row + x, &phase, black, noise) {
                            out.push((x, y, r));
                        }
                    }
                }
            }
            out.sort_unstable_by_key(|&(x, _, _)| x);
            out
        })
        .collect()
}

/// Are at least [`MIN_LIT`] direct neighbours of `(x, y)` clearly above their own same-colour surroundings
/// (real light)? A single one is not enough: hot photosites also come in adjacent pairs.
fn lit_around(v: &View<'_>, x: usize, y: usize, black: f32, noise: Noise, buf: &mut Vec<f32>) -> bool {
    v.phase(x, y)
        .around
        .iter()
        .filter(|&&(dx, dy)| {
            let (nx, ny) = ((x as isize + dx) as usize, (y as isize + dy) as usize);
            let s = v.at(nx as isize, ny as isize);
            v.neighbours(nx, ny, buf);
            let m = median(buf);
            s - m > K_SPOT * noise.sigma(m - black)
        })
        .count()
        >= MIN_LIT
}

#[cfg(test)]
mod tests {
    use crate::*;

    #[test]
    fn noise_bin_partition_matches_the_sorted_median() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        for n in (0..=257).chain([1024, 4095, 4096, 65536]) {
            let mut values: Vec<f32> = (0..n)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    // Repeated counts and both signs, as well as bins with unique values.
                    (seed % 4096) as f32 - 2048.0
                })
                .collect();
            for special in [0.0, -0.0, f32::INFINITY, f32::NEG_INFINITY, f32::NAN] {
                if let Some(first) = values.first_mut() {
                    *first = special;
                }
                let mut sorted = values.clone();
                sorted.sort_by(f32::total_cmp);
                let expected = if n == 0 {
                    0.0
                } else if n % 2 == 1 {
                    sorted[n / 2]
                } else {
                    0.5 * (sorted[n / 2 - 1] + sorted[n / 2])
                };
                assert_eq!(super::median(&mut values.clone()).to_bits(), expected.to_bits(), "{n} samples, {special}");
            }
        }
    }

    /// A `w × h` RGGB mosaic, black 512, white 16383, flat grey 2000 counts above black with
    /// deterministic noise of about σ ≈ 30 counts.
    fn mosaic(w: usize, h: usize) -> RawImage {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut noise = || {
            // sum of 4 uniforms, about N(0, 1)
            (0..4)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    (seed >> 11) as f32 / (1u64 << 53) as f32 - 0.5
                })
                .sum::<f32>()
                * 1.732
        };
        let data = (0..w * h).map(|_| (512.0 + 2000.0 + 30.0 * noise()).round() as u16).collect();
        RawImage {
            format: RawFormat::Arw,
            width: w,
            height: h,
            cpp: 1,
            data: RawData::U16(data),
            cfa: Cfa::bayer("RGGB"),
            bits: 14,
            black: BlackLevel::uniform(512.0),
            white: vec![16383.0],
            active_area: Rect::new(0, 0, w, h),
            crop: Rect::new(0, 0, w, h),
            orientation: Orientation::Normal,
            color: ColorData::default(),
            wb_multipliers: None,
            linearized: false,
            opcodes: OpcodeLists::default(),
            metadata: Metadata::default(),
            pixel_aspect: 1.0,
            lens_metadata: Default::default(),
        }
    }

    fn get(r: &RawImage, x: usize, y: usize) -> u16 {
        match &r.data {
            RawData::U16(d) => d[y * r.width + x],
            RawData::F32(_) => 0,
        }
    }

    fn set(r: &mut RawImage, x: usize, y: usize, v: u16) {
        let w = r.width;
        if let RawData::U16(d) = &mut r.data {
            d[y * w + x] = v;
        }
    }

    #[test]
    fn flat_noise_is_left_alone() {
        let mut r = mosaic(256, 256);
        let before = r.data.clone();
        assert_eq!(r.suppress_hot_pixels(), 0);
        assert_eq!(r.data, before);
    }

    #[test]
    fn single_hot_photosites_of_each_colour_are_replaced() {
        let mut r = mosaic(256, 256);
        // red (even, even), green (odd, even), blue (odd, odd)
        for &(x, y) in &[(100, 100), (101, 60), (151, 151)] {
            set(&mut r, x, y, 9000);
        }
        assert_eq!(r.suppress_hot_pixels(), 3);
        for &(x, y) in &[(100, 100), (101, 60), (151, 151)] {
            let v = get(&r, x, y) as f32;
            assert!((v - 2512.0).abs() < 120.0, "({x}, {y}) = {v}");
        }
    }

    #[test]
    fn a_small_star_is_kept() {
        let mut r = mosaic(256, 256);
        // a point of light over a 3 × 3 block: brightest in the middle, its neighbours lit too
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                let v = if dx == 0 && dy == 0 { 12000 } else { 5000 };
                set(&mut r, (120 + dx) as usize, (90 + dy) as usize, v);
            }
        }
        let before = r.data.clone();
        assert_eq!(r.suppress_hot_pixels(), 0);
        assert_eq!(r.data, before);
    }

    #[test]
    fn a_hot_photosite_in_a_dark_area_is_replaced() {
        let mut r = mosaic(256, 256);
        // a dark patch near black (noise is then read noise, smaller)
        for y in 32..96 {
            for x in 32..96 {
                set(&mut r, x, y, 512 + 20 + ((x * 7 + y * 13) % 11) as u16);
            }
        }
        set(&mut r, 64, 64, 1500);
        assert_eq!(r.suppress_hot_pixels(), 1);
        assert!(get(&r, 64, 64) < 600);
        // an adjacent hot pair is replaced
        {
            let mut r = mosaic(256, 256);
            set(&mut r, 80, 80, 9000);
            set(&mut r, 80, 81, 8000);
            assert_eq!(r.suppress_hot_pixels(), 2);
            assert!(get(&r, 80, 80) < 2700 && get(&r, 80, 81) < 2700);
        }
    }

    #[test]
    fn too_many_candidates_change_nothing() {
        let mut r = mosaic(128, 128);
        for y in (8..120).step_by(4) {
            for x in (8..120).step_by(4) {
                set(&mut r, x, y, 12000);
            }
        }
        let before = r.data.clone();
        assert_eq!(r.suppress_hot_pixels(), 0);
        assert_eq!(r.data, before);
    }
    /// The plain per-photosite loop `candidates` replaced: the oracle for the fast one.
    fn candidates_plain(
        v: &super::View<'_>,
        (x0, x1, y0, y1): (usize, usize, usize, usize),
        black: f32,
        noise: super::Noise,
    ) -> Vec<(usize, usize, u16)> {
        let mut out = Vec::new();
        let mut buf = Vec::with_capacity(8);
        for y in y0..y1 {
            for x in x0..x1 {
                let s = v.at(x as isize, y as isize);
                let max = v.neighbours(x, y, &mut buf);
                if s <= max {
                    continue;
                }
                let m = super::median(&mut buf);
                if s - max > super::K_HOT * noise.sigma(m - black) {
                    out.push((x, y, m.round().clamp(0.0, u16::MAX as f32) as u16));
                }
            }
        }
        out
    }

    /// A noisy `w × h` mosaic with hot photosites of every size and position sprinkled in, singly and in pairs.
    fn speckled(w: usize, h: usize, level: f32, sigma: f32, seed0: u64) -> Vec<u16> {
        let mut seed = seed0;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut d: Vec<u16> = (0..w * h)
            .map(|_| (level + sigma * ((next() >> 11) as f32 / (1u64 << 53) as f32 - 0.5) * 3.4).round().clamp(0.0, 65535.0) as u16)
            .collect();
        for _ in 0..(w * h / 40) {
            let i = (next() % (w * h) as u64) as usize;
            let boost = [3, 6, 7, 12, 40, 400, 5000][(next() % 7) as usize];
            d[i] = d[i].saturating_add(boost);
            if next() % 4 == 0 && i + 1 < d.len() {
                d[i + 1] = d[i + 1].saturating_add(boost / 2);
            }
        }
        d
    }

    #[test]
    fn the_fast_candidate_loop_matches_the_plain_one() {
        let noise = super::Noise { a: 0.4, b: 30.0 };
        // (CFA, width, height, level): Bayer in each order, an odd-sized window, a flat dark and a bright field, and X-Trans
        // (a 6 × 6 pattern whose phases have fewer or more neighbours than 8 within two photosites)
        let cfas = [Cfa::bayer("RGGB"), Cfa::bayer("BGGR"), Cfa::bayer("GRBG"), Some(Cfa::xtrans())];
        let mut checked = 0;
        for (n, cfa) in cfas.into_iter().flatten().enumerate() {
            for &(w, h, level, sigma) in &[(61usize, 47usize, 2000.0f32, 30.0f32), (96, 64, 80.0, 6.0), (73, 55, 14000.0, 90.0), (40, 40, 0.0, 0.0)] {
                let d = speckled(w, h, level, sigma, 0x9e37_79b9_7f4a_7c15 ^ (n as u64 * 977 + w as u64));
                let ph = super::phases(&cfa);
                let v = super::View { d: &d, w, pw: cfa.width, ph: cfa.height, phases: &ph };
                for black in [0.0f32, 512.0] {
                    for bounds in [(MARGIN_T, w - MARGIN_T, MARGIN_T, h - MARGIN_T), (MARGIN_T + 3, w - MARGIN_T - 1, MARGIN_T + 1, h - MARGIN_T - 2)]
                    {
                        let want = candidates_plain(&v, bounds, black, noise);
                        let got = super::candidates(&v, bounds, black, noise);
                        assert_eq!(got, want, "cfa {n}, {w}x{h}, level {level}, black {black}, bounds {bounds:?}");
                        checked += want.len();
                    }
                }
            }
        }
        // the sprinkled hot photosites must actually have produced candidates, or the comparison proves nothing
        assert!(checked > 200, "only {checked} candidates compared");
    }

    const MARGIN_T: usize = super::MARGIN;

    #[test]
    fn candidate_reads_outside_the_buffer_give_none_not_a_panic() {
        let d = vec![100u16; 64];
        let phase = super::FlatOffsets { n: 8, off: [-33, -2, -1, 1, 2, 3, 7, 40] };
        let noise = super::Noise { a: 0.4, b: 30.0 };
        for idx in [0usize, 1, 31, 62, 63, 64, 1000, usize::MAX / 2] {
            assert_eq!(super::candidate_at(&d, idx, &phase, 0.0, noise), None, "idx {idx}");
        }
    }

    #[test]
    fn median_of_eight_by_network_matches_the_sorted_middle_pair() {
        // The network is not a full sort (the two middle values may come out swapped), so what is pinned is the median:
        // the sum of the sorted middle pair, bit for bit. Every 0/1 input first, then ties and extremes.
        for bits in 0u32..256 {
            let v: Vec<f32> = (0..8).map(|i| ((bits >> i) & 1) as f32).collect();
            let mut sorted = v.clone();
            sorted.sort_by(f32::total_cmp);
            let want = 0.5 * (sorted[3] + sorted[4]);
            assert_eq!(super::median(&mut v.clone()).to_bits(), want.to_bits(), "bits {bits:08b}");
        }
        // integer-valued and fractional inputs with ties and the extremes of the u16 range
        let mut seed = 0x1234_5678_9abc_def1u64;
        for _ in 0..20_000 {
            let v: Vec<f32> = (0..8)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    match seed % 5 {
                        0 => 0.0,
                        1 => 65535.0,
                        2 => (seed >> 8) as f32 % 16.0,
                        3 => ((seed >> 8) % 65536) as f32,
                        _ => ((seed >> 8) % 4096) as f32 + 0.25,
                    }
                })
                .collect();
            let mut sorted = v.clone();
            sorted.sort_by(f32::total_cmp);
            let want = 0.5 * (sorted[3] + sorted[4]);
            assert_eq!(super::median(&mut v.clone()).to_bits(), want.to_bits(), "{v:?}");
        }
        // a NaN among them takes the sort path, as before
        let mut v = vec![1.0, f32::NAN, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let mut sorted = v.clone();
        sorted.sort_by(f32::total_cmp);
        assert_eq!(super::median(&mut v).to_bits(), (0.5 * (sorted[3] + sorted[4])).to_bits());
    }

    #[test]
    fn sigma_is_never_below_one() {
        // the candidate loop skips the median for an excess of K_HOT counts or less, which holds only while this does
        for (a, b) in [(0.0, 0.0), (0.0, 0.5), (0.4, 30.0), (1e-9, 0.0), (f32::NAN, f32::NAN), (-3.0, -3.0)] {
            for level in [-5000.0f32, 0.0, 1.0, 12.0, 65535.0, f32::NAN] {
                let s = super::Noise { a, b }.sigma(level);
                assert!(s >= 1.0, "a {a} b {b} level {level}: {s}");
            }
        }
    }

    /// σ = 1 (a = b = 0): the threshold is exactly K_HOT counts above the brightest neighbour, and strictly above it.
    #[test]
    fn the_threshold_is_strictly_above_k_hot_counts_at_the_smallest_sigma() {
        let (w, h) = (48usize, 48usize);
        let cfa = Cfa::bayer("RGGB").expect("RGGB");
        let ph = super::phases(&cfa);
        let noise = super::Noise { a: 0.0, b: 0.0 };
        let mut d = vec![2000u16; w * h];
        // bumps of +4, +5, +6, +7, +9 on isolated photosites of every phase (spaced 6 apart so no one is a neighbour of another)
        let mut sites = Vec::new();
        for (k, bump) in [4u16, 5, 6, 7, 9, 5, 6, 4].into_iter().enumerate() {
            let (x, y) = (8 + 6 * (k % 5), 8 + 6 * (k / 5) + (k % 2));
            d[y * w + x] += bump;
            sites.push((x, y, bump));
        }
        let v = super::View { d: &d, w, pw: cfa.width, ph: cfa.height, phases: &ph };
        let bounds = (super::MARGIN, w - super::MARGIN, super::MARGIN, h - super::MARGIN);
        let got = super::candidates(&v, bounds, 0.0, noise);
        assert_eq!(got, candidates_plain(&v, bounds, 0.0, noise));
        for (x, y, bump) in sites {
            assert_eq!(got.iter().any(|&(cx, cy, _)| (cx, cy) == (x, y)), bump > 5, "bump +{bump} at ({x}, {y})");
        }
    }

    /// Hot photosites of the given excesses (counts over a flat `level`) on a grid 7 apart, so that no site is another's
    /// neighbour; each excess is cycled over the grid. Returns the mosaic and `(x, y, excess)` of every bump.
    fn bump_grid(w: usize, h: usize, level: u16, excesses: &[u16]) -> (Vec<u16>, Vec<(usize, usize, u16)>) {
        let mut d = vec![level; w * h];
        let mut bumps = Vec::new();
        for (k, (x, y)) in (8..w - 8).step_by(7).flat_map(|x| (8..h - 8).step_by(7).map(move |y| (x, y))).enumerate() {
            let e = excesses[k % excesses.len()];
            d[y * w + x] += e;
            bumps.push((x, y, e));
        }
        (d, bumps)
    }

    /// T05, T06, T20: at, one below and one above `K_HOT` σ over the brightest neighbour, at low noise (σ between 1 and 4,
    /// where the `<= K_HOT` early-out is within reach of the real threshold) and at a level-dependent σ with a fractional
    /// limit; on Bayer (the fast loop) and X-Trans (phases with fewer than 8 neighbours: `candidate_at`). The fast loop,
    /// `candidate_at` and the plain loop must agree on every photosite, and agree with the rule: strictly above.
    #[test]
    fn the_candidate_rule_is_strict_and_identical_in_both_paths_at_the_boundary() {
        let (w, h, level) = (96usize, 96usize, 2000u16);
        let cfas = [Cfa::bayer("RGGB"), Cfa::bayer("GRBG"), Some(Cfa::xtrans())];
        // (noise model, black, the excess the rule needs to be exceeded: K_HOT σ)
        let cases = [
            (super::Noise { a: 0.0, b: 0.0 }, 0.0f32, 5.0f32),               // σ 1
            (super::Noise { a: 0.0, b: 4.0 }, 0.0, 10.0),                    // σ 2: the tie is an integer, 10
            (super::Noise { a: 0.0, b: 9.0 }, 512.0, 15.0),                  // σ 3
            (super::Noise { a: 0.5, b: 0.0 }, 0.0, 5.0 * 1000.0f32.sqrt()),  // σ² = 0.5 · 2000
            (super::Noise { a: 0.5, b: 0.0 }, 500.0, 5.0 * 750.0f32.sqrt()), // σ² = 0.5 · 1500
        ];
        let mut sites_on_short_phases = 0;
        for cfa in cfas.into_iter().flatten() {
            let ph = super::phases(&cfa);
            let flat = super::flat_phases(&ph, w);
            for &(noise, black, limit) in &cases {
                let l = limit.floor() as u16;
                let excesses = [l - 1, l, l + 1, l + 2, 3 * l, 20 * l.min(3000)];
                let (d, bumps) = bump_grid(w, h, level, &excesses);
                let v = super::View { d: &d, w, pw: cfa.width, ph: cfa.height, phases: &ph };
                let bounds = (super::MARGIN, w - super::MARGIN, super::MARGIN, h - super::MARGIN);
                let got = super::candidates(&v, bounds, black, noise);
                assert_eq!(got, candidates_plain(&v, bounds, black, noise), "noise {noise:?}, black {black}");
                let mut accepted = 0;
                for &(x, y, e) in &bumps {
                    let want = f32::from(e) > limit;
                    let phase = &flat[(y % cfa.height) * cfa.width + x % cfa.width];
                    let direct = super::candidate_at(&d, y * w + x, phase, black, noise);
                    let fast = got.iter().find(|&&(gx, gy, _)| (gx, gy) == (x, y)).map(|&(_, _, r)| r);
                    assert_eq!(fast.is_some(), want, "fast/plain path: excess {e} vs limit {limit}, noise {noise:?}, ({x}, {y})");
                    assert_eq!(direct.is_some(), want, "candidate_at: excess {e} vs limit {limit}, noise {noise:?}, ({x}, {y})");
                    // (the replacement is the neighbours' median: the flat level)
                    assert_eq!(fast, direct);
                    if want {
                        assert_eq!(direct, Some(level));
                        accepted += 1;
                        sites_on_short_phases += usize::from(phase.n < super::MAX_NEIGHBOURS);
                    }
                }
                assert_eq!(got.len(), accepted, "nothing else is a candidate");
                assert!(accepted > 20, "only {accepted} accepted: the comparison needs both sides");
            }
        }
        // X-Trans' red and blue phases have fewer than 8 same-colour neighbours within two photosites
        assert!(sites_on_short_phases > 50, "{sites_on_short_phases}");
    }

    /// T05 without any CFA: `candidate_at` on a phase of 5 neighbours, at σ = 2 (a limit of 10 counts), where the early-out
    /// at `K_HOT` counts must let an excess of 11 through to the real test and the real test must refuse 10.
    #[test]
    fn candidate_at_with_a_short_phase_applies_the_sigma_test_not_just_the_early_out() {
        let phase = super::FlatOffsets::new(&[(-2, 0), (-1, 0), (1, 0), (2, 0), (3, 0)], 100);
        assert_eq!(phase.n, 5);
        let noise = super::Noise { a: 0.0, b: 4.0 };
        for (excess, want) in [(0u16, None), (5, None), (6, None), (10, None), (11, Some(1000u16)), (12, Some(1000)), (30, Some(1000))] {
            let mut d = vec![1000u16; 64];
            d[30] += excess;
            assert_eq!(super::candidate_at(&d, 30, &phase, 0.0, noise), want, "excess {excess}");
        }
        // the brightest neighbour, not the median, is what the excess is measured from
        let mut d = vec![1000u16; 64];
        d[31] += 40;
        d[30] += 40 + 10;
        assert_eq!(super::candidate_at(&d, 30, &phase, 0.0, noise), None);
        d[30] += 1;
        assert_eq!(super::candidate_at(&d, 30, &phase, 0.0, noise), Some(1000));
    }

    #[test]
    fn a_phase_without_neighbours_gives_no_verdict_and_short_phases_are_unusable() {
        let noise = super::Noise { a: 0.0, b: 0.0 };
        assert_eq!(super::accept(60000, &[0; super::MAX_NEIGHBOURS], 0, 0.0, noise), None);
        assert_eq!(super::accept(60000, &[0; super::MAX_NEIGHBOURS], super::MAX_NEIGHBOURS + 1, 0.0, noise), None);
        assert_eq!(super::candidate_at(&[7u16; 16], 8, &super::FlatOffsets::new(&[], 4), 0.0, noise), None);
        for cfa in [Cfa::bayer("RGGB"), Cfa::bayer("BGGR"), Some(Cfa::xtrans())].into_iter().flatten() {
            assert!(super::phases_usable(&super::phases(&cfa)));
        }
        let ph = super::phases(&Cfa::bayer("RGGB").expect("RGGB"));
        for keep in [0, super::MIN_NEIGHBOURS - 1] {
            let mut short = ph.clone();
            short[1].same.truncate(keep);
            assert!(!super::phases_usable(&short), "{keep} neighbours");
        }
        let mut just = ph.clone();
        just[1].same.truncate(super::MIN_NEIGHBOURS);
        assert!(super::phases_usable(&just));
    }

    /// I1/I2: the margin the passes leave around the active area covers every read: the same-colour window, and the ring of
    /// direct neighbours `lit_around` looks at plus their windows. No phase has more neighbours than the arrays hold.
    #[test]
    fn the_margin_covers_the_window_and_no_phase_outgrows_the_arrays() {
        // (the ring of direct neighbours is 1 photosite out, and each of them reads its own window)
        assert!(super::MARGIN as isize > super::WINDOW_RADIUS);
        for cfa in [Cfa::bayer("RGGB"), Cfa::bayer("GBRG"), Some(Cfa::xtrans())].into_iter().flatten() {
            for p in super::phases(&cfa) {
                assert!(p.same.len() <= super::MAX_NEIGHBOURS);
                for &(dx, dy) in p.same.iter().chain(p.around.iter()) {
                    assert!(dx.abs().max(dy.abs()) <= super::WINDOW_RADIUS, "({dx}, {dy})");
                }
                // (the window is full wherever it holds more same-colour photosites than the cap)
                assert_eq!(super::FlatOffsets::new(&p.same, 64).n, p.same.len());
            }
        }
        // X-Trans' greens have more than 8 same-colour photosites within 2: the cap is what keeps 8
        let greens = super::phases(&Cfa::xtrans()).iter().filter(|p| p.same.len() == super::MAX_NEIGHBOURS).count();
        assert!(greens > 0);
    }
}
