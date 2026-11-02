//! Smooth Skin (beyond Lightroom): frequency separation of the scene-linear image inside the skin
//! the models found ([`SkinAnalysis`]), after white balance, spot removal and noise reduction and
//! before tone and colour, so exposure, contrast and colour grading ride on top of it.
//!
//! The image is split, in log luminance and two log colour ratios (`R/G`, `B/G`), into four
//! bands measured in each face's eye distance `d` (so a preview and an export look alike):
//!
//! - **fine** (below [`FINE`]·d): pores and fine texture — kept as they are (Texture moves them
//!   only on request, and never by default: no plastic skin);
//! - **middle** ([`FINE`]..[`MIDDLE`]·d): uneven texture, small bumps, fine lines — softened by
//!   Amount, the classic "invisible" retouch band. Small swings go, strong edges (the nose, a
//!   jawline, a laugh line) stay: the softening fades out with the band's own contrast;
//! - **low** ([`MIDDLE`]..[`LOW`]·d): blotches of tone and colour — evened out by Even Tone, and
//!   patches redder than the skin around them by Redness; strong low-band contrast (shadows,
//!   shine) is kept the same way;
//! - **base** (above [`LOW`]·d): the face's shape and light — untouched.
//!
//! Each band is a normalized convolution over the skin alone (`blur(x·m) / blur(m)`), so hair,
//! eyes, lips and the background never bleed into the skin's bands, and the result is blended in
//! by the skin's coverage: nothing outside the skin changes. Faces of very different sizes (a
//! group with someone in front) are worked in buckets of similar size, each pixel by the face it
//! belongs to. The wide bands are computed on reduced copies, which keeps a 2.5 MP preview in a
//! few tens of milliseconds.

use lcc_develop::{DevelopSettings, SkinAnalysis};
use lcc_geom::Point;
use lcc_raster::{Image, Plane, Rgb32f, blur::gaussian, par_rows};

use crate::geometry::Frame;
use crate::masks::SegGrid;

/// Top of the fine band (pores), in eye distances.
pub const FINE: f64 = 0.010;
/// Top of the middle band (uneven texture), in eye distances.
pub const MIDDLE: f64 = 0.05;
/// Top of the low band (blotches); above it is the face's shape, never touched.
pub const LOW: f64 = 0.22;
/// Eye distance assumed where no face was found (body skin of someone facing away), as a
/// fraction of the long edge.
pub const NO_FACE_EYES: f64 = 0.04;
/// Faces whose eye distances are within this ratio share their bands.
const BUCKET_RATIO: f64 = 1.5;
/// Pixels with less skin coverage than this are left alone.
const MIN_COVERAGE: f32 = 1.0 / 256.0;
/// Skin cells whose logit is above this bound the work (sigmoid ≈ 0.02).
const MIN_LOGIT: f32 = -4.0;
/// Rec. 2020 luminance weights (the pipeline's working space).
const KR: f32 = 0.2627;
const KG: f32 = 0.6780;
const KB: f32 = 0.0593;
/// Offset of the logs (deep shadows stay finite).
const EPS: f32 = 1.0 / 16384.0;

/// The sliders, as factors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    /// Middle-band softening, 0..1.
    pub amount: f32,
    /// Fine-band change, −1..1 (0 keeps it).
    pub texture: f32,
    /// Low-band evening, 0..1.
    pub even: f32,
    /// Redness reduction, 0..1.
    pub red: f32,
}

impl Params {
    pub fn of(s: &lcc_develop::SkinSmoothing) -> Params {
        let f = |v: f64, lo: f64| if v.is_finite() { (v / 100.0).clamp(lo, 1.0) as f32 } else { 0.0 };
        Params { amount: f(s.amount, 0.0), texture: f(s.texture, -1.0), even: f(s.even_tone, 0.0), red: f(s.redness, 0.0) }
    }

    fn is_identity(&self) -> bool {
        self.amount == 0.0 && self.texture == 0.0 && self.even == 0.0 && self.red == 0.0
    }
}

/// The analysis Smooth Skin renders `s` with, or `None` when it changes nothing (off, its section
/// switched off, or no skin found yet for its area, person and orientation).
pub fn analysis(s: &DevelopSettings) -> Option<&SkinAnalysis> {
    if !s.skin.active() || !s.section_enabled("skin") || Params::of(&s.skin).is_identity() {
        return None;
    }
    s.skin_analysis.as_ref().filter(|a| a.is_for(&s.skin, s.orientation))
}

/// What Smooth Skin's result depends on (`None` when it does nothing): a stage-cache key.
pub fn key(s: &DevelopSettings) -> Option<u64> {
    use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};
    let a = analysis(s)?;
    let k = &s.skin;
    let faces: Vec<[u64; 3]> = a.faces.iter().map(|f| [f.x.to_bits(), f.y.to_bits(), f.eyes.to_bits()]).collect();
    Some(BuildHasherDefault::<DefaultHasher>::default().hash_one((
        [k.amount, k.texture, k.even_tone, k.redness].map(f64::to_bits),
        a.seg.side,
        &a.seg.data,
        a.seg.rect.map(|r| r.map(f64::to_bits)),
        faces,
    )))
}

/// Eye distances of the analysis' faces in output pixels (`ppl`: output pixels per unit of the
/// long edge), at least one.
fn eye_px(a: &SkinAnalysis, ppl: f64) -> Vec<f64> {
    a.faces.iter().map(|f| f.eyes).filter(|e| e.is_finite() && *e > 0.0).map(|e| (e.min(1.0) * ppl).max(1.0)).collect()
}

/// How far (output pixels) Smooth Skin reads around a pixel it changes: a windowed render must
/// hold this much more around itself to match the whole render.
pub fn reach(s: &DevelopSettings, ppl: f64) -> f64 {
    let Some(a) = analysis(s) else { return 0.0 };
    let d = eye_px(a, ppl).into_iter().fold(NO_FACE_EYES * ppl, f64::max);
    (3.0 * LOW * d).ceil().min(1e6)
}

/// `log2(x)` for a positive, finite `x`, to about 1e-7 (a few times faster than `f32::log2`):
/// the exponent, and the mantissa's logarithm by the series of atanh.
#[inline]
pub(crate) fn fast_log2(x: f32) -> f32 {
    let bits = x.to_bits();
    let mut e = ((bits >> 23) & 0xff) as i32 - 127;
    let mut m = f32::from_bits((bits & 0x007f_ffff) | 0x3f80_0000);
    if m > std::f32::consts::SQRT_2 {
        m *= 0.5;
        e += 1;
    }
    let s = (m - 1.0) / (m + 1.0);
    let s2 = s * s;
    // 2 / ln 2 · (s + s³/3 + s⁵/5 + s⁷/7)
    e as f32 + 2.885_39 * s * (1.0 + s2 * (1.0 / 3.0 + s2 * (0.2 + s2 * (1.0 / 7.0))))
}

/// `2^x` to about 3e-6 (relative), exactly 1 at 0; 0 below −126, capped at 2^126.
#[inline]
pub(crate) fn fast_exp2(x: f32) -> f32 {
    if x.is_nan() {
        return 1.0;
    }
    if x < -126.0 {
        return 0.0;
    }
    let x = x.min(126.0);
    let i = x.round();
    let f = (x - i) * std::f32::consts::LN_2;
    let p = 1.0 + f * (1.0 + f * (0.5 + f * (1.0 / 6.0 + f * (1.0 / 24.0 + f * (1.0 / 120.0)))));
    f32::from_bits(((i as i32 + 127) as u32) << 23) * p
}

/// Log luminance and log colour ratios of a scene-linear pixel; `None` for pixels the bands
/// can't describe (not finite, or a channel at or below zero: out of gamut).
#[inline]
fn to_lab(p: [f32; 3]) -> Option<[f32; 3]> {
    let [r, g, b] = p;
    if !(r > 0.0 && g > 0.0 && b > 0.0 && r.is_finite() && g.is_finite() && b.is_finite()) {
        return None;
    }
    let y = KR * r + KG * g + KB * b;
    let lg = fast_log2(g + EPS);
    Some([fast_log2(y + EPS), fast_log2(r + EPS) - lg, fast_log2(b + EPS) - lg])
}

/// `p` with its log luminance moved by `dl` and its log colour ratios by `da`, `db` (all zero:
/// `p` exactly).
#[inline]
fn shifted(p: [f32; 3], dl: f32, da: f32, db: f32) -> [f32; 3] {
    let [r, g, b] = p;
    let y = KR * r + KG * g + KB * b;
    let (rg, bg) = (r / g * fast_exp2(da), b / g * fast_exp2(db));
    let y2 = y * fast_exp2(dl);
    let g2 = y2 / (KR * rg + KG + KB * bg);
    let out = [g2 * rg, g2, g2 * bg];
    if out.iter().all(|v| v.is_finite()) { out } else { p }
}

/// `exp(−x² / 2k²)`: 1 for small swings, 0 for strong ones.
#[inline]
fn small(x: f32, k: f32) -> f32 {
    fast_exp2(-(x * x) / (2.0 * k * k) * std::f32::consts::LOG2_E)
}

/// A reduced, blurred copy of a premultiplied band image: `factor` pixels of the full image per
/// pixel of `img`.
struct Reduced {
    img: Image<[f32; 4]>,
    factor: usize,
}

impl Reduced {
    /// `src` blurred by `sigma` (pixels of `src`), computed at a reduction that keeps the blur
    /// at least about 3 reduced pixels wide.
    fn of(src: &Image<[f32; 4]>, sigma: f64) -> Reduced {
        let factor = ((sigma / 3.0).floor() as usize).clamp(1, 32);
        let small = if factor == 1 { src.clone() } else { reduce(src, factor) };
        Reduced { img: gaussian(&small, (sigma / factor as f64) as f32), factor }
    }

    /// Bilinear sample at full-resolution pixel `(x, y)` of the source.
    #[inline]
    fn at(&self, x: usize, y: usize) -> [f32; 4] {
        if self.factor == 1 {
            return self.img.data.get(y * self.img.width + x).copied().unwrap_or_default();
        }
        let f = self.factor as f32;
        let (w, h) = (self.img.width, self.img.height);
        if w == 0 || h == 0 {
            return [0.0; 4];
        }
        let fx = ((x as f32 + 0.5) / f - 0.5).clamp(0.0, (w - 1) as f32);
        let fy = ((y as f32 + 0.5) / f - 0.5).clamp(0.0, (h - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let px = |x: usize, y: usize| self.img.data.get(y * w + x).copied().unwrap_or_default();
        let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
        std::array::from_fn(|k| (a[k] * (1.0 - tx) + b[k] * tx) * (1.0 - ty) + (c[k] * (1.0 - tx) + d[k] * tx) * ty)
    }
}

/// Box-average `src` by `f` (the last row and column average what they have).
fn reduce(src: &Image<[f32; 4]>, f: usize) -> Image<[f32; 4]> {
    let (w, h) = (src.width.div_ceil(f), src.height.div_ceil(f));
    let mut out = Image::<[f32; 4]>::new(w, h);
    par_rows(&mut out.data, w, |y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut acc = [0f32; 4];
            let mut n = 0f32;
            for sy in y * f..((y + 1) * f).min(src.height) {
                let line = src.data.get(sy * src.width..(sy + 1) * src.width).unwrap_or(&[]);
                for v in line.get(x * f..((x + 1) * f).min(src.width)).unwrap_or(&[]) {
                    for k in 0..4 {
                        acc[k] += v[k];
                    }
                    n += 1.0;
                }
            }
            if n > 0.0 {
                *o = acc.map(|a| a / n);
            }
        }
    });
    out
}

/// The normalized band value `v[..3] / v[3]`, or `fallback` where the skin weight is too thin.
#[inline]
fn norm(v: [f32; 4], fallback: [f32; 3]) -> [f32; 3] {
    if v[3] > 1e-3 {
        let r = 1.0 / v[3];
        [v[0] * r, v[1] * r, v[2] * r]
    } else {
        fallback
    }
}

/// A face in output pixels: centre and eye distance.
#[derive(Clone, Copy, Debug)]
struct FacePx {
    x: f64,
    y: f64,
    d: f64,
}

/// The face pixel `(x, y)` belongs to: the nearest in units of its own size (a big face's cheek
/// is its own, even where a small face's centre is nearer).
fn owner(faces: &[FacePx], x: f64, y: f64) -> Option<usize> {
    faces
        .iter()
        .enumerate()
        .map(|(i, f)| (i, ((x - f.x).powi(2) + (y - f.y).powi(2)).sqrt() / f.d))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// Faces grouped by size: each group's members are within [`BUCKET_RATIO`] of its smallest.
fn buckets(faces: &[FacePx]) -> Vec<Vec<usize>> {
    let mut order: Vec<usize> = (0..faces.len()).collect();
    order.sort_by(|a, b| faces[*a].d.total_cmp(&faces[*b].d));
    let mut out: Vec<Vec<usize>> = Vec::new();
    for i in order {
        match out.last_mut() {
            Some(g) if g.first().is_some_and(|f| faces[i].d <= faces[*f].d * BUCKET_RATIO) => g.push(i),
            _ => out.push(vec![i]),
        }
    }
    out
}

/// Apply Smooth Skin to `img` (the output-resolution, scene-linear image `frame` draws, before
/// exposure), `ppl` output pixels per unit of the long edge. Does nothing when [`analysis`] is
/// `None` or the stored skin is damaged.
pub fn apply(img: &mut Rgb32f, s: &DevelopSettings, frame: &Frame, ppl: f64) {
    let Some(a) = analysis(s) else { return };
    let Some(grid) = SegGrid::new(&a.seg) else { return };
    let p = Params::of(&s.skin);
    let (w, h) = (img.width, img.height);
    if w < 2 || h < 2 || !(ppl.is_finite() && ppl > 0.0) {
        return;
    }
    // where the skin is, in output pixels
    let Some(b) = grid.bounds_above(MIN_LOGIT) else { return };
    let to_out = frame.norm_to_out(w, h);
    let corners = [Point::new(b[0], b[1]), Point::new(b[2], b[1]), Point::new(b[0], b[3]), Point::new(b[2], b[3])].map(|c| to_out.apply(c));
    let (bx0, bx1) = corners.iter().fold((f64::MAX, f64::MIN), |(lo, hi), c| (lo.min(c.x), hi.max(c.x)));
    let (by0, by1) = corners.iter().fold((f64::MAX, f64::MIN), |(lo, hi), c| (lo.min(c.y), hi.max(c.y)));
    let clampx = |v: f64| if v.is_finite() { v.clamp(0.0, w as f64) as usize } else { 0 };
    let clampy = |v: f64| if v.is_finite() { v.clamp(0.0, h as f64) as usize } else { 0 };
    let (sx0, sx1, sy0, sy1) = (clampx(bx0.floor() - 1.0), clampx(bx1.ceil() + 1.0), clampy(by0.floor() - 1.0), clampy(by1.ceil() + 1.0));
    if sx1 <= sx0 || sy1 <= sy0 {
        return;
    }
    // the skin's coverage over that box
    let to_norm = frame.out_to_norm(w, h);
    let (mw, mh) = (sx1 - sx0, sy1 - sy0);
    let mut cover = Plane::new(mw, mh);
    let t = crate::profiling().then(std::time::Instant::now);
    par_rows(&mut cover.data, mw, |y, row| {
        // (the map to normalized coordinates is affine: step along the row)
        let p0 = to_norm.apply(Point::new(sx0 as f64 + 0.5, (sy0 + y) as f64 + 0.5));
        let p1 = to_norm.apply(Point::new(sx0 as f64 + 1.5, (sy0 + y) as f64 + 0.5));
        let (dx, dy) = (p1.x - p0.x, p1.y - p0.y);
        for (x, v) in row.iter_mut().enumerate() {
            let n = Point::new(p0.x + dx * x as f64, p0.y + dy * x as f64);
            let l = grid.logit(n);
            *v = 1.0 / (1.0 + fast_exp2(-l * std::f32::consts::LOG2_E));
        }
    });
    if let Some(t) = t {
        eprintln!("    skin coverage ({mw}×{mh}): {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
    }
    let mut faces: Vec<FacePx> = a
        .faces
        .iter()
        .filter(|f| f.eyes.is_finite() && f.eyes > 0.0 && f.x.is_finite() && f.y.is_finite())
        .map(|f| {
            let c = to_out.apply(Point::new(f.x, f.y));
            FacePx { x: c.x, y: c.y, d: (f.eyes.min(1.0) * ppl).max(1.0) }
        })
        .collect();
    if faces.is_empty() {
        // skin without a face: measured as an average portrait's
        faces.push(FacePx { x: w as f64 / 2.0, y: h as f64 / 2.0, d: (NO_FACE_EYES * ppl).max(1.0) });
    }
    for bucket in buckets(&faces) {
        smooth_bucket(img, &cover, (sx0, sy0), &faces, &bucket, p);
    }
}

/// Smooth the skin of the faces in `bucket` (indices into `faces`): `cover` is the skin's
/// coverage over the box at `origin` of `img`.
fn smooth_bucket(img: &mut Rgb32f, cover: &Plane, origin: (usize, usize), faces: &[FacePx], bucket: &[usize], p: Params) {
    let (w, h) = (img.width, img.height);
    let (ox, oy) = origin;
    let d = bucket.iter().filter_map(|i| faces.get(*i)).map(|f| f.d.ln()).sum::<f64>() / bucket.len().max(1) as f64;
    let d = d.exp();
    let all = bucket.len() >= faces.len();
    let mine = |x: usize, y: usize| all || owner(faces, x as f64 + 0.5, y as f64 + 0.5).is_some_and(|o| bucket.contains(&o));
    // the pixels this bucket changes, and the room its widest band reads around them
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for (k, c) in cover.data.iter().enumerate() {
        if *c < MIN_COVERAGE {
            continue;
        }
        let (x, y) = (ox + k % cover.width, oy + k / cover.width);
        if mine(x, y) {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
        }
    }
    if x0 == usize::MAX {
        return;
    }
    let room = (3.0 * LOW * d).ceil() as usize;
    let (rx0, ry0, rx1, ry1) = (x0.saturating_sub(room), y0.saturating_sub(room), (x1 + room).min(w), (y1 + room).min(h));
    let (rw, rh) = (rx1 - rx0, ry1 - ry0);
    if rw == 0 || rh == 0 {
        return;
    }
    let coverage = |x: usize, y: usize| -> f32 {
        if x < ox || y < oy {
            return 0.0;
        }
        cover.data.get((y - oy) * cover.width + (x - ox)).copied().filter(|_| x - ox < cover.width).unwrap_or(0.0)
    };
    // the bands' input: log luminance and colour ratios, premultiplied by the skin's coverage
    let mut lab = Image::<[f32; 4]>::new(rw, rh);
    let t = crate::profiling().then(std::time::Instant::now);
    {
        let src = &*img;
        par_rows(&mut lab.data, rw, |y, row| {
            let gy = ry0 + y;
            for (x, o) in row.iter_mut().enumerate() {
                let gx = rx0 + x;
                let m = coverage(gx, gy);
                if m < MIN_COVERAGE {
                    continue;
                }
                if let Some(v) = src.data.get(gy * w + gx).copied().and_then(to_lab) {
                    *o = [v[0] * m, v[1] * m, v[2] * m, m];
                }
            }
        });
    }
    if let Some(t) = t {
        eprintln!("    skin bands' input ({rw}×{rh}): {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
    }
    let fine = crate::timed("skin fine band", || gaussian(&lab, (FINE * d).max(0.0) as f32));
    let mid = crate::timed("skin middle band", || Reduced::of(&lab, MIDDLE * d));
    let low = crate::timed("skin low band", || Reduced::of(&lab, LOW * d));
    // how strong a swing still counts as small (log2 units): softening fades out above it
    let k_mid = 0.04 + 0.08 * p.amount;
    let k_low = 0.12;
    let tex = if p.texture < 0.0 { p.texture } else { 0.75 * p.texture };
    let even = 0.85 * p.even;
    let even_c = 0.8 * p.even;
    let rows = img.data.get_mut(ry0 * w..ry1 * w).unwrap_or(&mut []);
    let t = crate::profiling().then(std::time::Instant::now);
    par_rows(rows, w, |y, row| {
        let gy = ry0 + y;
        for x in 0..rw {
            let gx = rx0 + x;
            let m = coverage(gx, gy);
            if m < MIN_COVERAGE || !mine(gx, gy) {
                continue;
            }
            let Some(px) = row.get(gx).copied() else { continue };
            // (the pixel's own bands' input, premultiplied: zero weight where it had none)
            let v = lab.data.get(y * rw + x).copied().unwrap_or_default();
            if v[3] <= 0.0 {
                continue;
            }
            let r = 1.0 / v[3];
            let v0 = [v[0] * r, v[1] * r, v[2] * r];
            let v1 = norm(fine.data.get(y * rw + x).copied().unwrap_or_default(), v0);
            let v2 = norm(mid.at(x, y), v1);
            let v3 = norm(low.at(x, y), v2);
            // luminance: the middle band's small swings softened, the low band's evened, the fine band kept
            let (hi, md, lo) = (v0[0] - v1[0], v1[0] - v2[0], v2[0] - v3[0]);
            let dl = -p.amount * md * small(md, k_mid) - even * lo * small(lo, k_low) + tex * hi;
            // colour: blotches (middle + low bands) evened, redder patches reduced, the middle band softened
            let (va, vb) = (v1[1] - v3[1], v1[2] - v3[2]);
            let (ma, mb) = (v1[1] - v2[1], v1[2] - v2[2]);
            let da = -even_c * va - p.red * va.max(0.0) * (1.0 - even_c) - 0.5 * p.amount * ma;
            let db = -even_c * vb - 0.5 * p.amount * mb;
            let out = shifted(px, dl, da, db);
            if let Some(o) = row.get_mut(gx) {
                *o = std::array::from_fn(|k| px[k] + m * (out[k] - px[k]));
            }
        }
    });
    if let Some(t) = t {
        eprintln!("    skin combine: {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
    }
}
