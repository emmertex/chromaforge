//! Dead, hot and stuck sensor pixels: finding them in one raw mosaic and correcting them before demosaic.
//!
//! Works on the stored samples (any [`Cfa`] layout — Bayer, X-Trans — as well as monochrome and three-sample linear
//! data), in the coordinates of the full data (`RawImage::width × height`, masked borders included), inside the
//! active area. A defect is replaced by the median of its nearest same-colour neighbours that are not defects
//! themselves, so every later stage (binning, demosaic, highlight rebuild) sees the corrected mosaic.
//!
//! **Finding them in one photo** ([`detect`]) is deliberately conservative. On black-subtracted, white-normalised
//! values, for each sample with same-colour neighbours `N` (the up to 8 nearest of its colour):
//! - *hot*: it exceeds `max(N)` by more than `k` noise sigmas, by `ratio` × `median(N)` and by an absolute floor;
//! - *stuck*: hot at the clip level while its neighbours sit well below it (no `ratio` needed);
//! - *dead*: lit neighbours (`median(N)` above a floor) and the sample below `dead_ratio` × that median.
//!
//! The noise sigma per colour is measured on the photo itself (median absolute difference of same-colour samples
//! along rows), and the local spread of `N` raises it on texture, so thresholds follow ISO and exposure.
//!
//! A real point of light — a star, a catchlight, a glint — is never a single sample: optics, the low-pass filter
//! and diffraction spread it onto the adjacent pixels (which are other colours on a CFA). The *isolation guard*
//! measures each 4-adjacent sample against the median of its own same-colour neighbours: two or more of them
//! lifted (by more than 8 % of the candidate's excess plus 3 sigmas), or one lifted only a little, and the candidate
//! is left alone. One adjacent sample lifted as much as the candidate itself is a second defect (a pair), which
//! both members accept.
//!
//! The *context guard* looks a little further: glints of sun on sand or water, glitter and sequins make fields of
//! single-pixel sparkles that pass the isolation guard one by one, so a candidate with another sample standing out
//! by a third of its excess within 16 pixels is left alone too. Defects are spread thinly over a sensor; a pixel the
//! camera body's map knows is corrected whatever its surroundings.
//!
//! No code from other raw processors was used; the method is the usual "isolated outlier among same-colour
//! neighbours" idea, with the guard and thresholds above.

use crate::{Cfa, RawData, RawImage, Rect};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

/// What is wrong with a pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DefectKind {
    /// Brighter than it should be (dark current, a leaky photosite); grows with exposure time and heat.
    Hot,
    /// Always dark.
    Dead,
    /// At (or near) the clip level whatever the light.
    Stuck,
}

/// One defective sample: full-data coordinates and the sample index (`0` unless the data has 3 samples a pixel).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Defect {
    pub x: u32,
    pub y: u32,
    #[serde(default)]
    pub s: u8,
    pub kind: DefectKind,
    /// How far it sits from its neighbours (normalised, 1 = white).
    pub level: f32,
}

/// Detection thresholds (normalised values, 1 = white).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thresholds {
    /// Noise sigmas a hot sample must exceed its brightest neighbour by.
    pub k: f32,
    /// ... and this fraction of its neighbours' median.
    pub ratio: f32,
    /// ... and this absolute amount.
    pub floor: f32,
    /// Dead: neighbours' median at least this, and the sample below `dead_ratio` of it.
    pub dead_min: f32,
    pub dead_ratio: f32,
    /// More than this many defects in one photo means the detector is reading the scene, not the sensor (a dense
    /// star field on a sensor without a low-pass filter): nothing is reported.
    pub max: usize,
}

impl Thresholds {
    /// What a photo's own render corrects: only clear, isolated outliers.
    pub const CORRECT: Thresholds = Thresholds { k: 12.0, ratio: 0.35, floor: 0.02, dead_min: 0.08, dead_ratio: 0.15, max: 20_000 };
    /// What is recorded for a camera body's map (confirmed only after several photos of different scenes agree).
    pub const LEARN: Thresholds = Thresholds { k: 7.0, ratio: 0.25, floor: 0.008, dead_min: 0.05, dead_ratio: 0.25, max: 50_000 };
}

/// What [`detect`] found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Detection {
    pub defects: Vec<Defect>,
    /// Noise sigma per colour (red, green, blue; normalised) the thresholds were scaled by.
    pub noise: [f32; 3],
    /// More candidates than [`Thresholds::max`]: `defects` is empty.
    pub overflow: bool,
}

/// A sample to correct (full-data coordinates).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Fix {
    pub x: u32,
    pub y: u32,
    #[serde(default)]
    pub s: u8,
}

impl From<&Defect> for Fix {
    fn from(d: &Defect) -> Fix {
        Fix { x: d.x, y: d.y, s: d.s }
    }
}

/// The margin (pixels) inside the active area left alone: neighbours reach this far.
const MARGIN: usize = 3;
/// How far around a candidate the context guard looks (pixels).
const CONTEXT: usize = 16;
/// A normalised value this close to white counts as clipped.
const CLIP: f32 = 0.97;

/// Read-only view of a raw's samples with the geometry the checks need.
pub(crate) struct View<'a> {
    raw: &'a RawImage,
    w: usize,
    pub(crate) cpp: usize,
    pub(crate) area: Rect,
    /// 1 / (white − black) per sample.
    pub(crate) scale: Vec<f32>,
    /// Same-colour neighbour offsets per CFA position (nearest first, at most 8); one entry without a CFA.
    table: Vec<Vec<(isize, isize)>>,
    cfa: Option<&'a Cfa>,
}

impl<'a> View<'a> {
    pub(crate) fn new(raw: &'a RawImage) -> Option<View<'a>> {
        raw.validate().ok()?;
        let area = raw.active_area;
        if area.width <= 2 * MARGIN + 1 || area.height <= 2 * MARGIN + 1 {
            return None;
        }
        let cfa = raw.cfa.as_ref().filter(|_| raw.cpp == 1);
        let scale = (0..raw.cpp)
            .map(|s| {
                let range = raw.white_at(s) - raw.black.mean();
                if range > 0.0 && range.is_finite() { 1.0 / range } else { 1.0 }
            })
            .collect();
        let table = match cfa {
            Some(c) => (0..c.height).flat_map(|py| (0..c.width).map(move |px| (px, py))).map(|(px, py)| same_colour_offsets(c, px, py)).collect(),
            None => vec![ring8()],
        };
        if table.iter().any(|t| t.len() < 3) {
            return None;
        }
        Some(View { raw, w: raw.width, cpp: raw.cpp, area, scale, table, cfa })
    }

    #[inline]
    fn class(&self, x: usize, y: usize) -> usize {
        match self.cfa {
            Some(c) => (y % c.height) * c.width + (x % c.width),
            None => 0,
        }
    }

    /// Colour (0 red, 1 green, 2 blue) of sample `s` at `(x, y)`.
    #[inline]
    pub(crate) fn colour(&self, x: usize, y: usize, s: usize) -> usize {
        match self.cfa {
            Some(c) => c.color_at(x, y).min(2) as usize,
            None if self.cpp >= 3 => s.min(2),
            None => 1,
        }
    }

    #[inline]
    pub(crate) fn raw_at(&self, x: usize, y: usize, s: usize) -> f32 {
        let i = (y * self.w + x) * self.cpp + s;
        match &self.raw.data {
            RawData::U16(v) => v.get(i).map_or(0.0, |&v| f32::from(v)),
            RawData::F32(v) => v.get(i).copied().filter(|v| v.is_finite()).unwrap_or(0.0),
        }
    }

    #[inline]
    fn black(&self, x: usize, y: usize, s: usize) -> f32 {
        self.raw.black.at(x.saturating_sub(self.area.x), y.saturating_sub(self.area.y), s, self.cpp)
    }

    /// Black-subtracted, white-normalised value.
    #[inline]
    pub(crate) fn val(&self, x: usize, y: usize, s: usize) -> f32 {
        (self.raw_at(x, y, s) - self.black(x, y, s)) * self.scale.get(s).copied().unwrap_or(1.0)
    }

    fn offsets(&self, x: usize, y: usize) -> &[(isize, isize)] {
        self.table.get(self.class(x, y)).map_or(&[], Vec::as_slice)
    }

    /// Inside the area the checks may read around.
    #[inline]
    pub(crate) fn inner(&self, x: usize, y: usize) -> bool {
        let a = self.area;
        x >= a.x + MARGIN && y >= a.y + MARGIN && x + MARGIN < a.x + a.width && y + MARGIN < a.y + a.height
    }

    /// Values of the same-colour neighbours, sorted ascending, into `buf`.
    fn neighbours(&self, x: usize, y: usize, s: usize, buf: &mut Vec<f32>) {
        buf.clear();
        for &(dx, dy) in self.offsets(x, y) {
            let (nx, ny) = ((x as isize + dx) as usize, (y as isize + dy) as usize);
            buf.push(self.val(nx, ny, s));
        }
        buf.sort_by(f32::total_cmp);
    }

    /// Robust noise sigma per colour: the median absolute difference of same-colour samples along sampled rows.
    pub(crate) fn noise(&self) -> [f32; 3] {
        let a = self.area;
        let mut diffs: [Vec<f32>; 3] = Default::default();
        let step = (a.height / 120).max(1);
        let mut y = a.y + MARGIN;
        while y + MARGIN < a.y + a.height {
            let mut x = a.x + MARGIN;
            while x + MARGIN < a.x + a.width {
                // the nearest same-colour neighbour on the same row, to the right
                let right = self.offsets(x, y).iter().filter(|o| o.1 == 0 && o.0 > 0).map(|o| o.0).min();
                if let Some(dx) = right {
                    let nx = x + dx as usize;
                    if nx + MARGIN < a.x + a.width {
                        for s in 0..self.cpp {
                            let c = self.colour(x, y, s);
                            if let Some(d) = diffs.get_mut(c) {
                                d.push((self.val(x, y, s) - self.val(nx, y, s)).abs());
                            }
                        }
                    }
                }
                x += 3;
            }
            y += step;
        }
        let mut out = [0.0f32; 3];
        for (o, d) in out.iter_mut().zip(diffs.iter_mut()) {
            if d.is_empty() {
                continue;
            }
            let mid = d.len() / 2;
            let (_, m, _) = d.select_nth_unstable_by(mid, f32::total_cmp);
            *o = *m * 1.4826 / std::f32::consts::SQRT_2;
        }
        // a colour with no samples (none on that layout) takes the others' level
        let known = out.iter().copied().filter(|v| *v > 0.0).fold(0.0f32, f32::max);
        out.map(|v| if v > 0.0 { v } else { known.max(1e-5) })
    }

    /// The defect at sample `(x, y, s)`, if it is one.
    fn judge(&self, x: usize, y: usize, s: usize, t: &Thresholds, noise: &[f32; 3], buf: &mut Vec<f32>) -> Option<Defect> {
        self.neighbours(x, y, s, buf);
        let (&lo, &hi) = (buf.first()?, buf.last()?);
        let med = *buf.get(buf.len() / 2)?;
        let v = self.val(x, y, s);
        let c = self.colour(x, y, s);
        // (the range of 8 noisy samples is about 3 sigmas: a quarter of it is under one on a smooth area and grows on texture)
        let sigma = noise.get(c).copied().unwrap_or(1e-4).max((hi - lo) * 0.25);
        let (kind, excess) = if v > hi {
            let excess = v - hi;
            let stuck = v >= CLIP && hi < 0.6;
            let need = if stuck { (t.k * sigma).max(t.floor) } else { (t.k * sigma).max(t.ratio * med.max(0.0)).max(t.floor) };
            if excess <= need {
                return None;
            }
            (if stuck { DefectKind::Stuck } else { DefectKind::Hot }, excess)
        } else if med > t.dead_min && v < t.dead_ratio * med && lo - v > (t.k * sigma).max(0.5 * med) {
            (DefectKind::Dead, med - v)
        } else {
            return None;
        };
        let dark = kind == DefectKind::Dead;
        (self.isolated(x, y, s, excess, dark, noise, buf) && self.alone(x, y, s, excess, dark)).then_some(Defect {
            x: x as u32,
            y: y as u32,
            s: s as u8,
            kind,
            level: excess,
        })
    }

    /// The context guard: no other sample within [`CONTEXT`] pixels stands out from its nearest same-colour samples by
    /// a third of this one's excess. Glints — sun on sand or water, glitter, sequins, frost — come in fields of
    /// single-pixel sparkles; a hot pixel is alone (defects are spread thinly over a sensor).
    fn alone(&self, x: usize, y: usize, s: usize, excess: f32, dark: bool) -> bool {
        let a = self.area;
        let (x0, x1) = (x.saturating_sub(CONTEXT).max(a.x + MARGIN), (x + CONTEXT + 1).min(a.x + a.width - MARGIN));
        let (y0, y1) = (y.saturating_sub(CONTEXT).max(a.y + MARGIN), (y + CONTEXT + 1).min(a.y + a.height - MARGIN));
        let bar = excess / 3.0;
        for ny in y0..y1 {
            for nx in x0..x1 {
                // (the candidate and its own 3 × 3: a defect pair, or the guard above's business)
                if nx.abs_diff(x) <= 1 && ny.abs_diff(y) <= 1 {
                    continue;
                }
                let near = self.offsets(nx, ny);
                let v = self.val(nx, ny, s);
                let mut edge = if dark { f32::MAX } else { f32::MIN };
                for &(dx, dy) in near.iter().take(4) {
                    let n = self.val((nx as isize + dx) as usize, (ny as isize + dy) as usize, s);
                    edge = if dark { edge.min(n) } else { edge.max(n) };
                }
                let stands_out = if dark { edge - v } else { v - edge };
                if stands_out > bar {
                    return false;
                }
            }
        }
        true
    }

    /// The isolation guard (see the module documentation): the 8 adjacent samples against the background around
    /// the candidate (per colour, the median of the samples 3 and 4 pixels away). A point of light lifts them —
    /// even a blob a few pixels wide, whose samples all stand out from that background — a hot pixel does not.
    #[allow(clippy::too_many_arguments)]
    fn isolated(&self, x: usize, y: usize, s: usize, excess: f32, dark: bool, noise: &[f32; 3], buf: &mut Vec<f32>) -> bool {
        // background per colour (three-sample data: per channel)
        let mut ring: [Vec<f32>; 3] = Default::default();
        for dy in -4isize..=4 {
            for dx in -4isize..=4 {
                if dx.abs().max(dy.abs()) < 3 {
                    continue;
                }
                let (nx, ny) = ((x as isize + dx) as usize, (y as isize + dy) as usize);
                for ns in 0..self.cpp {
                    if let Some(r) = ring.get_mut(self.colour(nx, ny, ns)) {
                        r.push(self.val(nx, ny, ns));
                    }
                }
            }
        }
        let bg: [Option<f32>; 3] = std::array::from_fn(|c| {
            let r = ring.get_mut(c)?;
            if r.is_empty() {
                return None;
            }
            let mid = r.len() / 2;
            Some(*r.select_nth_unstable_by(mid, f32::total_cmp).1)
        });
        buf.clear();
        let (mut lifted, mut strong) = (0u32, 0u32);
        // the mean lift of the adjacent samples that aren't defects themselves, and their noise level: a faint
        // sharp star lifts each of them by less than the noise, but all of them together by more
        let (mut sum, mut n, mut sig) = (0.0f32, 0u32, 0.0f32);
        let mut check = |nx: usize, ny: usize, ns: usize| {
            let c = self.colour(nx, ny, ns);
            let Some(Some(b)) = bg.get(c) else { return };
            let e = self.val(nx, ny, ns) - b;
            let e = if dark { -e } else { e };
            let sigma = noise.get(c).copied().unwrap_or(1e-4);
            if e > 0.5 * excess {
                strong += 1;
                lifted += 1;
                return;
            }
            if e > 0.08 * excess + 3.0 * sigma {
                lifted += 1;
            }
            sum += e;
            n += 1;
            sig += sigma;
        };
        for dy in -1isize..=1 {
            for dx in -1isize..=1 {
                let (nx, ny) = ((x as isize + dx) as usize, (y as isize + dy) as usize);
                for ns in 0..self.cpp {
                    // the candidate itself, and (three-sample data) the same channel of the adjacent pixels: those are
                    // its neighbours already
                    if (dx, dy) == (0, 0) && ns == s || self.cpp >= 3 && ns == s {
                        continue;
                    }
                    check(nx, ny, ns);
                }
            }
        }
        // (the mean of n adjacent lifts has about sigma / sqrt(n) of noise)
        let spread = n > 0 && sum / n as f32 > 0.05 * excess + 2.0 * (sig / n as f32) / (n as f32).sqrt();
        !spread && (lifted == 0 || (lifted == 1 && strong == 1))
    }
}

/// The 8 pixels around one.
fn ring8() -> Vec<(isize, isize)> {
    let mut v: Vec<(isize, isize)> = (-1..=1).flat_map(|dy| (-1..=1).map(move |dx| (dx, dy))).filter(|&o| o != (0, 0)).collect();
    v.sort_by_key(|&(dx, dy)| dx * dx + dy * dy);
    v
}

/// The same-colour neighbours of CFA position `(px, py)` within [`MARGIN`] pixels, nearest first, at most 8.
pub fn same_colour_offsets(cfa: &Cfa, px: usize, py: usize) -> Vec<(isize, isize)> {
    let r = MARGIN as isize;
    let (bw, bh) = (cfa.width as isize * 8, cfa.height as isize * 8);
    let own = cfa.color_at(px, py);
    let mut v: Vec<(isize, isize)> = Vec::new();
    for dy in -r..=r {
        for dx in -r..=r {
            if (dx, dy) == (0, 0) {
                continue;
            }
            let (x, y) = ((px as isize + dx + bw) as usize, (py as isize + dy + bh) as usize);
            if cfa.color_at(x, y) == own {
                v.push((dx, dy));
            }
        }
    }
    v.sort_by_key(|&(dx, dy)| (dx * dx + dy * dy, dy, dx));
    v.truncate(8);
    v
}

/// Find the defective samples of one photo with thresholds `t`.
pub fn detect(raw: &RawImage, t: &Thresholds) -> Detection {
    let Some(view) = View::new(raw) else { return Detection::default() };
    let noise = view.noise();
    let a = view.area;
    let (y0, y1) = (a.y + MARGIN, a.y + a.height - MARGIN);
    let (x0, x1) = (a.x + MARGIN, a.x + a.width - MARGIN);
    let black = raw.black.mean();
    // a quick test on the stored values that every defect passes: brighter than the 4 nearest same-colour samples
    // by the floor, or darker than half the darkest of them (the full check follows only then)
    let floor_raw: Vec<f32> = view.scale.iter().map(|s| t.floor / s).collect();
    let found: Vec<Defect> = (y0..y1)
        .into_par_iter()
        .flat_map_iter(|y| {
            let mut buf = Vec::with_capacity(8);
            let mut out = Vec::new();
            for x in x0..x1 {
                let near = view.offsets(x, y);
                for s in 0..view.cpp {
                    let v = view.raw_at(x, y, s);
                    let (mut mx, mut mn) = (f32::MIN, f32::MAX);
                    for &(dx, dy) in near.iter().take(4) {
                        let n = view.raw_at((x as isize + dx) as usize, (y as isize + dy) as usize, s);
                        mx = mx.max(n);
                        mn = mn.min(n);
                    }
                    let fr = floor_raw.get(s).copied().unwrap_or(0.0);
                    let bright = v > mx + fr;
                    let dark = v - black < 0.5 * (mn - black) && mn - black > 0.0;
                    if (bright || dark)
                        && let Some(d) = view.judge(x, y, s, t, &noise, &mut buf)
                    {
                        out.push(d);
                    }
                }
            }
            out
        })
        .collect();
    if found.len() > t.max {
        return Detection { defects: Vec::new(), noise, overflow: true };
    }
    Detection { defects: found, noise, overflow: false }
}

/// The defects of `found` (from [`detect`] with looser thresholds) that also pass `t`: one detection pass serves
/// both what a photo corrects and what a camera body's map learns.
pub fn refine(raw: &RawImage, found: &Detection, t: &Thresholds) -> Vec<Defect> {
    let Some(view) = View::new(raw) else { return Vec::new() };
    let mut buf = Vec::with_capacity(8);
    let out: Vec<Defect> = found
        .defects
        .iter()
        .filter_map(|d| {
            let (x, y, s) = (d.x as usize, d.y as usize, d.s as usize);
            (s < view.cpp && view.inner(x, y)).then(|| view.judge(x, y, s, t, &found.noise, &mut buf)).flatten()
        })
        .collect();
    if out.len() > t.max { Vec::new() } else { out }
}

/// Defects often come in small clusters (two or three photosites side by side, on older sensors especially), whose
/// members fail the isolation guard one by one. Around each trusted `seed` (a camera body's confirmed pixel), the
/// adjacent samples that stand out from their own same-colour neighbours — by 30 % of their level, by a hundredth of
/// the seed's excess and by the noise (a saturated defect blooms into its neighbours) — are defects too: returned, up
/// to two steps out from the seeds.
pub fn grow(raw: &RawImage, seeds: &[Fix]) -> Vec<Fix> {
    let Some(view) = View::new(raw) else { return Vec::new() };
    let noise = view.noise();
    let mut have: std::collections::HashSet<(u32, u32, u8)> = seeds.iter().map(|f| (f.x, f.y, f.s)).collect();
    let mut out = Vec::new();
    let mut buf = Vec::with_capacity(8);
    // (the median of a sample's same-colour neighbours that aren't known defects)
    let base = |x: usize, y: usize, s: usize, have: &std::collections::HashSet<(u32, u32, u8)>, buf: &mut Vec<f32>| {
        buf.clear();
        for &(dx, dy) in view.offsets(x, y) {
            let (nx, ny) = ((x as isize + dx) as usize, (y as isize + dy) as usize);
            if !have.contains(&(nx as u32, ny as u32, s as u8)) {
                buf.push(view.val(nx, ny, s));
            }
        }
        buf.sort_by(f32::total_cmp);
        buf.get(buf.len() / 2).copied()
    };
    let mut frontier: Vec<(Fix, f32)> = Vec::new();
    for f in seeds {
        let (x, y, s) = (f.x as usize, f.y as usize, f.s as usize);
        if s < view.cpp
            && view.inner(x, y)
            && let Some(m) = base(x, y, s, &have, &mut buf)
        {
            frontier.push((*f, (view.val(x, y, s) - m).abs()));
        }
    }
    for _ in 0..2 {
        let mut next = Vec::new();
        for (f, excess) in &frontier {
            for dy in -1isize..=1 {
                for dx in -1isize..=1 {
                    let (nx, ny) = ((f.x as isize + dx) as usize, (f.y as isize + dy) as usize);
                    if !view.inner(nx, ny) {
                        continue;
                    }
                    for ns in 0..view.cpp {
                        let key = (nx as u32, ny as u32, ns as u8);
                        if have.contains(&key) {
                            continue;
                        }
                        let Some(m) = base(nx, ny, ns, &have, &mut buf) else { continue };
                        let sigma = noise.get(view.colour(nx, ny, ns)).copied().unwrap_or(1e-4);
                        let e = view.val(nx, ny, ns) - m;
                        // (a saturated defect blooms into its neighbours: a halo far fainter than itself, still a
                        // bright ring on a dark exposure)
                        if e > (excess / 100.0).max(8.0 * sigma).max(0.004) && e > 0.3 * m.max(0.0) {
                            have.insert(key);
                            let fix = Fix { x: key.0, y: key.1, s: key.2 };
                            out.push(fix);
                            next.push((fix, e));
                        }
                    }
                }
            }
        }
        if next.is_empty() || out.len() > 4 * seeds.len() + 64 {
            break;
        }
        frontier = next;
    }
    out
}

/// Replace each sample in `fixes` with the median of its same-colour neighbours that aren't in `fixes` themselves.
/// Samples outside the active area's inner part, or with no usable neighbour, are left. Returns how many changed.
pub fn correct(raw: &mut RawImage, fixes: &[Fix]) -> usize {
    if fixes.is_empty() {
        return 0;
    }
    let new: Vec<(usize, f32)> = {
        let Some(view) = View::new(raw) else { return 0 };
        let index = |x: usize, y: usize, s: usize| (y * view.w + x) * view.cpp + s;
        let set: std::collections::HashSet<usize> =
            fixes.iter().filter(|f| (f.s as usize) < view.cpp).map(|f| index(f.x as usize, f.y as usize, f.s as usize)).collect();
        let mut buf = Vec::with_capacity(8);
        fixes
            .iter()
            .filter_map(|f| {
                let (x, y, s) = (f.x as usize, f.y as usize, f.s as usize);
                if s >= view.cpp || !view.inner(x, y) {
                    return None;
                }
                buf.clear();
                for &(dx, dy) in view.offsets(x, y) {
                    let (nx, ny) = ((x as isize + dx) as usize, (y as isize + dy) as usize);
                    if !set.contains(&index(nx, ny, s)) {
                        buf.push(view.val(nx, ny, s));
                    }
                }
                if buf.is_empty() {
                    return None;
                }
                buf.sort_by(f32::total_cmp);
                let m = buf.len();
                let med = if m % 2 == 1 { buf.get(m / 2).copied()? } else { (buf.get(m / 2 - 1).copied()? + buf.get(m / 2).copied()?) * 0.5 };
                let scale = view.scale.get(s).copied().unwrap_or(1.0);
                Some((index(x, y, s), med / scale + view.black(x, y, s)))
            })
            .collect()
    };
    let mut n = 0;
    match &mut raw.data {
        RawData::U16(v) => {
            for (i, val) in new {
                if let Some(d) = v.get_mut(i) {
                    *d = val.round().clamp(0.0, 65535.0) as u16;
                    n += 1;
                }
            }
        }
        RawData::F32(v) => {
            for (i, val) in new {
                if let Some(d) = v.get_mut(i) {
                    *d = val;
                    n += 1;
                }
            }
        }
    }
    n
}

/// A cheap fingerprint of a photo's samples: 64 values spread over the active area, for telling shots apart.
pub fn sample_fingerprint(raw: &RawImage) -> u64 {
    let a = raw.active_area;
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for k in 0..64usize {
        let x = a.x + (a.width.saturating_sub(1)) * ((k * 37) % 64) / 63;
        let y = a.y + (a.height.saturating_sub(1)) * k / 63;
        let i = (y * raw.width + x) * raw.cpp;
        let v = match &raw.data {
            RawData::U16(v) => v.get(i).map_or(0, |&v| u64::from(v)),
            RawData::F32(v) => v.get(i).map_or(0, |v| u64::from(v.to_bits())),
        };
        h = (h ^ v).wrapping_mul(0x0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests;
