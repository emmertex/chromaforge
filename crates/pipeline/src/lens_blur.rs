//! Lens Blur: a shallow depth of field added after the fact.
//!
//! Every pixel gets a circle of confusion from its distance to the sharp band
//! ([`lcc_develop::LensBlur`]: a focal distance, or the distance under the focus picker, and a
//! focal range). The distance comes from the depth model ([`crate::depth_model`]) when one is
//! installed; without it, from the Subject selection of the photo analysis (the subject near, the
//! rest far), and the render says so in a warning.
//!
//! The blur gathers in scene-linear light (bright highlights become shaped discs, as through a
//! lens) with an aperture-shaped kernel (circle, five or six blades, ring), in a few layers of
//! growing radius that each pixel blends between. A layer's large kernel runs on a reduced copy
//! (≤ 5 px kernel radius there), so its cost doesn't grow with the blur. A tap from a pixel that is
//! sharper than the layer counts for little, so a sharp subject doesn't smear into the blurred
//! background around it. Boost brightens highlights before they are spread, so they turn into
//! bright discs (bokeh) while the rest of the blur keeps its level. Radii are relative to the photo's long edge: previews match exports.
//! Both renderers run this CPU code on the scene-linear image after noise reduction.
//!
//! Brush Refinement paints over the depth by hand: Focus brush strokes stay sharp, Blur brush
//! strokes get the whole blur, whatever the distance says.
//!
//! A mask can blur too (Lens Blur among its local adjustments), with or without the global blur:
//! there the mask's strength is the circle of confusion, so what it fully selects gets the whole
//! blur, its feathered edge less, and what it leaves out stays sharp (or the other way round,
//! blurring what it leaves out). It uses the same gather and the global bokeh shape.
//!
//! Sources: own design (layered gather of a depth-of-field blur, as commonly described for
//! real-time renderers). Boost (the highlight gain before the spread, and its constants) is
//! translated from upstream PR #2661's `lens_blur.rs` (MIT); Brush Refinement and the per-mask
//! blur are ported from the same PR (how strokes and a mask's strength become the circle of
//! confusion), onto this module's gather.

use std::sync::Arc;

use lcc_develop::{BokehShape, BrushStroke, DevelopSettings, LensBlur, Mask, MaskShape};
use lcc_raster::{Plane, Rgb32f};

use crate::Plan;
use crate::geometry::Frame;
use crate::masks::{self, Mattes};

/// The largest blur radius at Blur Amount 100, as a fraction of the photo's long edge.
pub const MAX_RADIUS: f64 = 0.03;
/// How many blur layers (of radius `k / LAYERS` of the largest) a pixel blends between.
const LAYERS: usize = 6;
/// A layer's kernel radius on its reduced copy, at most (px).
const REDUCED_RADIUS: f64 = 5.0;
/// How far past the sharp band (in `0..1` distance) the blur reaches its full size.
const FALLOFF: f32 = 0.35;
/// The brightest highlights grow this much brighter at Boost 100 before they are spread.
const BOOST_GAIN: f32 = 4.0;

/// Where the distance grid came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DepthSource {
    /// The depth model's estimate.
    Model,
    /// The Subject selection (no depth model installed): the subject near, the rest far.
    Subject,
}

/// The distance grid Lens Blur works from (0 = nearest, 1 = farthest), over the uncropped,
/// oriented photo without its flips (as the other automatic grids, [`crate::auto_mask::sample`]).
#[derive(Clone, Debug)]
pub struct Depth {
    pub grid: Arc<Plane>,
    pub source: DepthSource,
}

/// The warning a render without the depth model carries.
pub const NO_MODEL_WARNING: &str =
    "Lens Blur: no depth model is installed, so depth is estimated from the subject (download the depth model in AI Models for a real depth map)";

/// The distance grid for `s`'s Lens Blur, when it is on: the depth model's, else one from the
/// Subject selection.
pub fn resolve(src: &Rgb32f, info: &crate::SourceInfo, s: &lcc_develop::DevelopSettings) -> Option<Arc<Depth>> {
    if !s.lens_blur.active() {
        return None;
    }
    if let Some(grid) = crate::depth_model::depth(src, info, s) {
        return Some(Arc::new(Depth { grid: Arc::new(grid), source: DepthSource::Model }));
    }
    let a = crate::auto_mask::for_source(src, info, s);
    Some(Arc::new(Depth { grid: Arc::new(subject_depth(&a.subject)), source: DepthSource::Subject }))
}

/// A distance grid from a Subject selection: the subject at 0, the rest at 1.
pub fn subject_depth(subject: &Plane) -> Plane {
    let mut p = subject.clone();
    p.map_in_place(|v| if v.is_finite() { 1.0 - v.clamp(0.0, 1.0) } else { 1.0 });
    p
}

/// The masks that blur what they select, or what they leave out (the Lens Blur block among a
/// mask's local adjustments): shown, with components, the block on and some blur.
fn blurring_masks(s: &DevelopSettings) -> impl Iterator<Item = &Mask> {
    s.masks
        .iter()
        .filter(|m| m.visible && !m.components.is_empty() && !m.adjust.lens_blur_off && m.adjust.lens_blur.is_finite() && m.adjust.lens_blur > 0.0)
}

/// Whether a mask of `s` blurs (so [`apply`] has work without the global Lens Blur).
pub fn masks_blur(s: &DevelopSettings) -> bool {
    blurring_masks(s).next().is_some()
}

/// Whether [`apply`] changes anything for `s`: the global Lens Blur (with its distance grid
/// `depth`) or a mask's.
pub fn renders(s: &DevelopSettings, depth: Option<&Depth>) -> bool {
    (depth.is_some() && s.lens_blur.active()) || masks_blur(s)
}

/// What makes the blurred image (for the render caches' keys): the global blur with its distance
/// grid, and the masks that blur.
pub fn key(s: &DevelopSettings, depth: Option<&Depth>) -> Option<String> {
    let lb = &s.lens_blur;
    let mut k = match depth {
        Some(d) if lb.active() => format!("{lb:?}{:?}{}x{}", d.source, d.grid.width, d.grid.height),
        _ => String::new(),
    };
    for m in blurring_masks(s) {
        // (the bokeh shape is the global one's; a mask's luminance / colour ranges select on the
        // exposed image)
        k.push_str(&format!("|{m:?}{:?}{}", lb.bokeh, s.light.exposure.to_bits()));
    }
    (!k.is_empty()).then_some(k)
}

/// How far (px) the blur reads around a pixel in a render of `px_per_long` px per long edge: the
/// global blur's reach or a mask's, whichever is farther.
pub fn reach(s: &DevelopSettings, px_per_long: f64) -> f64 {
    let lb = &s.lens_blur;
    let global = if lb.active() { max_radius(lb, px_per_long).ceil() + 1.0 } else { 0.0 };
    blurring_masks(s).map(|m| amount_radius(m.adjust.lens_blur, px_per_long).ceil() + 1.0).fold(global, f64::max)
}

fn max_radius(lb: &LensBlur, px_per_long: f64) -> f64 {
    amount_radius(lb.amount, px_per_long)
}

/// The largest blur radius (px) for a Blur Amount (`0..100`).
fn amount_radius(amount: f64, px_per_long: f64) -> f64 {
    let a = if amount.is_finite() { amount.clamp(0.0, 100.0) } else { 0.0 };
    let p = if px_per_long.is_finite() { px_per_long.max(0.0) } else { 0.0 };
    a / 100.0 * MAX_RADIUS * p
}

/// The distance kept sharp: the median of the grid under the focus picker's area (its value at
/// the point for a point), else the focal distance.
pub fn focus_distance(lb: &LensBlur, grid: &Plane) -> f32 {
    let fallback = if lb.focal_distance.is_finite() { (lb.focal_distance.clamp(0.0, 100.0) / 100.0) as f32 } else { 0.0 };
    let Some(p) = lb.focus else { return fallback };
    if grid.width == 0 || grid.height == 0 || !p.x.is_finite() || !p.y.is_finite() {
        return fallback;
    }
    let half = if lb.focus_size.is_finite() { lb.focus_size.clamp(0.0, 0.5) } else { 0.0 };
    let (gw, gh) = (grid.width as f64, grid.height as f64);
    let cell = |v: f64, n: f64| ((v * n).floor().max(0.0) as usize).min(n as usize - 1);
    let (x0, x1) = (cell(p.x - half, gw), cell(p.x + half, gw));
    let (y0, y1) = (cell(p.y - half, gh), cell(p.y + half, gh));
    let mut vals: Vec<f32> = Vec::new();
    for y in y0..=y1 {
        for x in x0..=x1 {
            if let Some(v) = grid.data.get(y * grid.width + x).filter(|v| v.is_finite()) {
                vals.push(*v);
            }
        }
    }
    if vals.is_empty() {
        return fallback;
    }
    vals.sort_by(f32::total_cmp);
    vals.get(vals.len() / 2).copied().unwrap_or(fallback).clamp(0.0, 1.0)
}

/// How blurred a pixel at distance `d` is (0 = sharp, 1 = the full blur) for a sharp band of
/// depth `range` (`0..1`) around `focus`.
pub fn coc(d: f32, focus: f32, range: f32) -> f32 {
    if !d.is_finite() || !focus.is_finite() {
        return 0.0;
    }
    let half = if range.is_finite() { range.clamp(0.0, 1.0) / 2.0 } else { 0.0 };
    let t = (((d - focus).abs() - half) / FALLOFF).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// How much brighter a tap of colour `c` counts at `boost` (`0..1`): 1 up to mid-grey, rising to
/// `1 + BOOST_GAIN · boost` at scene white and above.
fn boost_gain(c: [f32; 3], boost: f32) -> f32 {
    if !boost.is_finite() || boost <= 0.0 {
        return 1.0;
    }
    let l = lcc_color::luminance_2020(c);
    if !l.is_finite() {
        return 1.0;
    }
    let t = ((l - 0.5) / 0.5).clamp(0.0, 1.0);
    1.0 + BOOST_GAIN * boost.min(1.0) * t * t * (3.0 - 2.0 * t)
}

/// Whether offset `(u, v)` (in kernel radii) is inside `shape`'s aperture.
pub fn inside(shape: BokehShape, u: f64, v: f64) -> bool {
    let r = (u * u + v * v).sqrt();
    match shape {
        BokehShape::Circle => r <= 1.0,
        BokehShape::Ring => (0.55..=1.0).contains(&r),
        BokehShape::Pentagon => polygon(5, u, v),
        BokehShape::Hexagon => polygon(6, u, v),
    }
}

/// Inside the regular `n`-gon with its corners on the unit circle (one corner up).
fn polygon(n: usize, u: f64, v: f64) -> bool {
    let apothem = (std::f64::consts::PI / n as f64).cos();
    (0..n).all(|i| {
        let a = -std::f64::consts::FRAC_PI_2 + (2 * i + 1) as f64 * std::f64::consts::PI / n as f64;
        u * a.cos() + v * a.sin() <= apothem + 1e-9
    })
}

/// The kernel's taps for radius `r` (px): offsets inside the aperture (the centre always).
fn taps(shape: BokehShape, r: f64) -> Vec<(isize, isize)> {
    let n = r.ceil().max(0.0) as isize;
    let mut out = vec![(0, 0)];
    for y in -n..=n {
        for x in -n..=n {
            if (x, y) != (0, 0) && r > 0.0 && inside(shape, x as f64 / r, y as f64 / r) {
                out.push((x, y));
            }
        }
    }
    out
}

/// Lens Blur on `img` (the scene-linear render of `plan`), in place: the global blur, then what
/// each mask with a Lens Blur adjustment selects (in mask order).
pub fn apply(img: &mut Rgb32f, plan: &Plan<'_>) {
    let s = &*plan.settings;
    let mattes = plan.mattes.as_deref();
    if let Some(depth) = plan.lens_blur.as_deref()
        && s.lens_blur.active()
    {
        blur_refined(img, &s.lens_blur, depth, &plan.frame, plan.px_per_long, mattes);
    }
    blur_masks(img, s, &plan.frame, plan.px_per_long, mattes);
}

/// The global blur without Brush Refinement's auto-mask mattes ([`blur_refined`] with none).
pub fn blur(img: &mut Rgb32f, lb: &LensBlur, depth: &Depth, frame: &Frame, px_per_long: f64) {
    blur_refined(img, lb, depth, frame, px_per_long, None);
}

/// The global blur with its inputs spelt out: `frame` is the render's frame (`img` is its output).
pub fn blur_refined(img: &mut Rgb32f, lb: &LensBlur, depth: &Depth, frame: &Frame, px_per_long: f64, mattes: Option<&Mattes>) {
    let (w, h) = (img.width, img.height);
    let rmax = max_radius(lb, px_per_long);
    if !lb.active() || w == 0 || h == 0 || img.data.len() != w * h || rmax < 0.5 || depth.grid.width == 0 || depth.grid.height == 0 {
        return;
    }
    let focus = focus_distance(lb, &depth.grid);
    let range = if lb.focal_range.is_finite() { (lb.focal_range.clamp(0.0, 100.0) / 100.0) as f32 } else { 0.0 };
    let mut c = crate::auto_mask::sample(&depth.grid, frame, w, h);
    c.map_in_place(|d| coc(d, focus, range));
    refine(&mut c, lb, img, frame, mattes);
    let boost = if lb.boost.is_finite() { (lb.boost.clamp(0.0, 100.0) / 100.0) as f32 } else { 0.0 };
    gather(img, &c, rmax, lb.bokeh, boost);
}

/// Brush Refinement on the circle of confusion `c` (0 sharp, 1 the full blur): the Focus brush
/// takes blur away, the Blur brush adds it (and has the last word).
fn refine(c: &mut Plane, lb: &LensBlur, img: &Rgb32f, frame: &Frame, mattes: Option<&Mattes>) {
    if lb.focus_strokes.is_empty() && lb.blur_strokes.is_empty() {
        return;
    }
    let (w, h) = (img.width, img.height);
    let log_l = img.map(crate::local::log_lum);
    let brush = |strokes: &[BrushStroke]| {
        (!strokes.is_empty())
            .then(|| masks::shape_alpha(&MaskShape::Brush { strokes: strokes.to_vec() }, frame, w, h, img, &log_l, 0.0, mattes))
            .filter(|a| a.data.len() == c.data.len())
    };
    let unit = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
    let (focus, blurred) = (brush(&lb.focus_strokes), brush(&lb.blur_strokes));
    if let Some(f) = focus {
        c.data.iter_mut().zip(&f.data).for_each(|(v, f)| *v *= 1.0 - unit(*f));
    }
    if let Some(b) = blurred {
        c.data.iter_mut().zip(&b.data).for_each(|(v, b)| *v = v.max(unit(*b)));
    }
}

/// The blur of each mask with a Lens Blur adjustment: the mask's strength (its Amount included)
/// is how far out of focus a pixel is; blurring the outside, how far the mask stays below its
/// Amount.
pub fn blur_masks(img: &mut Rgb32f, s: &DevelopSettings, frame: &Frame, px_per_long: f64, mattes: Option<&Mattes>) {
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 || img.data.len() != w.saturating_mul(h) {
        return;
    }
    let ev = if s.light.exposure.is_finite() { s.light.exposure as f32 } else { 0.0 };
    for m in blurring_masks(s) {
        let a = &m.adjust;
        let rmax = amount_radius(a.lens_blur, px_per_long);
        if rmax < 0.5 {
            continue;
        }
        let log_l = img.map(crate::local::log_lum);
        let mut c = masks::evaluate_one(m, frame, w, h, img, &log_l, ev, mattes);
        if c.data.len() != img.data.len() {
            continue;
        }
        let top = if a.amount.is_finite() { (a.amount / 100.0).clamp(0.0, 1.0) as f32 } else { 0.0 };
        let outside = a.lens_blur_outside;
        c.map_in_place(|v| {
            let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
            if outside { (top - v).max(0.0) } else { v }
        });
        let boost = if a.lens_blur_boost.is_finite() { (a.lens_blur_boost.clamp(0.0, 100.0) / 100.0) as f32 } else { 0.0 };
        gather(img, &c, rmax, s.lens_blur.bokeh, boost);
    }
}

/// Blur `img` by the circle of confusion `c` (0 sharp .. 1 a kernel of `rmax` px, one value per
/// pixel) with `shape`'s aperture, highlights brightened by `boost` (`0..1`) before the spread.
fn gather(img: &mut Rgb32f, c: &Plane, rmax: f64, shape: BokehShape, boost: f32) {
    let w = img.width;
    if w == 0 || c.data.len() != img.data.len() || !rmax.is_finite() || rmax < 0.5 || c.data.iter().all(|v| *v <= 0.0) {
        return;
    }
    let layers: Vec<Layer> = (1..=LAYERS).map(|k| Layer::new(img, c, rmax * k as f64 / LAYERS as f64, k, shape, boost)).collect();
    let src = &*img;
    let mut out = src.data.clone();
    crate::for_rows(&mut out, w, |y, row| {
        for (x, px) in row.iter_mut().enumerate() {
            let t = c.data.get(y * w + x).copied().unwrap_or(0.0) * LAYERS as f32;
            // (a NaN circle of confusion is no blur)
            if t.is_nan() || t <= 0.0 {
                continue;
            }
            let lo = (t.floor() as usize).min(LAYERS);
            let f = (t - lo as f32).clamp(0.0, 1.0);
            let at = |k: usize| if k == 0 { *px } else { layers.get(k - 1).map_or(*px, |l| l.at(x, y)) };
            let (a, b) = (at(lo), at((lo + 1).min(LAYERS)));
            *px = [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f];
        }
    });
    img.data = out;
}

/// One blur layer: the image blurred with a kernel of one radius, on a copy reduced `scale` times.
struct Layer {
    img: Rgb32f,
    scale: usize,
}

impl Layer {
    fn new(src: &Rgb32f, coc: &Plane, r: f64, k: usize, shape: BokehShape, boost: f32) -> Layer {
        let scale = ((r / REDUCED_RADIUS).ceil() as usize).max(1);
        let (small, small_coc) = reduce(src, coc, scale);
        let rr = r / scale as f64;
        let taps = taps(shape, rr);
        let (w, h) = (small.width, small.height);
        let mut out = Rgb32f::new(w, h);
        // a tap sharper than this layer counts for little (a sharp subject doesn't smear)
        let need = (k as f32 - 1.0) / LAYERS as f32;
        crate::for_rows(&mut out.data, w, |y, row| {
            for (x, px) in row.iter_mut().enumerate() {
                let mut sum = [0.0f32; 3];
                let mut wsum = 0.0f32;
                for &(dx, dy) in &taps {
                    let sx = (x as isize + dx).clamp(0, w as isize - 1) as usize;
                    let sy = (y as isize + dy).clamp(0, h as isize - 1) as usize;
                    let i = sy * w + sx;
                    let (Some(p), Some(cv)) = (small.data.get(i), small_coc.data.get(i)) else { continue };
                    let wt = if *cv >= need { 1.0 } else { 0.02 + 0.98 * (cv / need.max(1e-3)).clamp(0.0, 1.0).powi(2) };
                    // (Boost: a highlight counts brighter in what it is spread into, not in the weights)
                    let g = wt * boost_gain(*p, boost);
                    sum = [sum[0] + p[0] * g, sum[1] + p[1] * g, sum[2] + p[2] * g];
                    wsum += wt;
                }
                if wsum > 0.0 {
                    *px = [sum[0] / wsum, sum[1] / wsum, sum[2] / wsum];
                } else if let Some(p) = small.data.get(y * w + x) {
                    *px = *p;
                }
            }
        });
        Layer { img: out, scale }
    }

    /// The layer at full-size pixel `(x, y)` (bilinear).
    fn at(&self, x: usize, y: usize) -> [f32; 3] {
        let (w, h) = (self.img.width, self.img.height);
        let s = self.scale as f32;
        let fx = ((x as f32 + 0.5) / s - 0.5).clamp(0.0, (w - 1) as f32);
        let fy = ((y as f32 + 0.5) / s - 0.5).clamp(0.0, (h - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let g = |x: usize, y: usize| self.img.data.get(y * w + x).copied().unwrap_or_default();
        let (a, b, c, d) = (g(x0, y0), g(x1, y0), g(x0, y1), g(x1, y1));
        let mut o = [0.0; 3];
        for (i, v) in o.iter_mut().enumerate() {
            let top = a[i] + (b[i] - a[i]) * tx;
            let bot = c[i] + (d[i] - c[i]) * tx;
            *v = top + (bot - top) * ty;
        }
        o
    }
}

/// `img` and `coc` reduced `scale` times by box averages.
fn reduce(img: &Rgb32f, coc: &Plane, scale: usize) -> (Rgb32f, Plane) {
    if scale <= 1 {
        return (img.clone(), coc.clone());
    }
    let (w, h) = (img.width, img.height);
    let (sw, sh) = (w.div_ceil(scale).max(1), h.div_ceil(scale).max(1));
    let mut out = Rgb32f::new(sw, sh);
    let mut oc = Plane::new(sw, sh);
    for y in 0..sh {
        for x in 0..sw {
            let mut sum = [0.0f32; 3];
            let mut cs = 0.0f32;
            let mut n = 0.0f32;
            for yy in y * scale..((y + 1) * scale).min(h) {
                for xx in x * scale..((x + 1) * scale).min(w) {
                    let i = yy * w + xx;
                    if let (Some(p), Some(c)) = (img.data.get(i), coc.data.get(i)) {
                        sum = [sum[0] + p[0], sum[1] + p[1], sum[2] + p[2]];
                        cs += c;
                        n += 1.0;
                    }
                }
            }
            if n > 0.0
                && let (Some(o), Some(c)) = (out.data.get_mut(y * sw + x), oc.data.get_mut(y * sw + x))
            {
                *o = [sum[0] / n, sum[1] / n, sum[2] / n];
                *c = cs / n;
            }
        }
    }
    (out, oc)
}

#[cfg(test)]
mod tests {
    use lcc_geom::Point;

    use super::*;

    #[test]
    fn the_blur_grows_away_from_the_sharp_band() {
        assert_eq!(coc(0.5, 0.5, 0.2), 0.0);
        assert_eq!(coc(0.59, 0.5, 0.2), 0.0);
        let near = coc(0.75, 0.5, 0.2);
        let far = coc(0.95, 0.5, 0.2);
        assert!(near > 0.0 && near < far && far <= 1.0, "{near} {far}");
        for (d, f, r) in [(f32::NAN, 0.5, 0.2), (0.5, f32::INFINITY, 0.2), (0.5, 0.5, f32::NAN), (2.0, -1.0, -3.0)] {
            let v = coc(d, f, r);
            assert!((0.0..=1.0).contains(&v), "{v}");
        }
    }

    #[test]
    fn apertures_have_their_shape() {
        for s in BokehShape::ALL {
            assert!(!inside(s, 1.2, 0.0), "{s:?}");
            // every aperture reaches its top corner
            assert!(inside(s, 0.0, -0.99), "{s:?}");
        }
        assert!(inside(BokehShape::Circle, 0.0, 0.0));
        assert!(!inside(BokehShape::Ring, 0.0, 0.0));
        // the pentagon's bottom is an edge, the hexagon's a corner
        assert!(!inside(BokehShape::Pentagon, 0.0, 0.97));
        assert!(inside(BokehShape::Hexagon, 0.0, 0.97));
        assert!(taps(BokehShape::Hexagon, 4.0).len() < taps(BokehShape::Circle, 4.0).len());
        assert_eq!(taps(BokehShape::Circle, 0.0), vec![(0, 0)]);
    }

    #[test]
    fn the_focus_picker_reads_the_depth_under_it() {
        // left half near, right half far
        let grid = Plane::from_fn(10, 10, |x, _| if x < 5 { 0.1 } else { 0.9 });
        let mut lb = LensBlur { enabled: true, focal_distance: 40.0, ..LensBlur::default() };
        assert!((focus_distance(&lb, &grid) - 0.4).abs() < 1e-6);
        lb.focus = Some(Point::new(0.2, 0.5));
        assert!((focus_distance(&lb, &grid) - 0.1).abs() < 1e-6);
        lb.focus = Some(Point::new(0.8, 0.5));
        lb.focus_size = 0.1;
        assert!((focus_distance(&lb, &grid) - 0.9).abs() < 1e-6);
        // hostile picks fall back or clamp
        for p in [Point::new(f64::NAN, 0.5), Point::new(-5.0, 9.0), Point::new(1e300, -1e300)] {
            lb.focus = Some(p);
            lb.focus_size = f64::INFINITY;
            let f = focus_distance(&lb, &grid);
            assert!((0.0..=1.0).contains(&f), "{f}");
        }
        assert!((focus_distance(&lb, &Plane::new(0, 0)) - 0.4).abs() < 1e-6);
    }

    fn frame(w: usize, h: usize) -> Frame {
        Frame::new(w, h, &lcc_develop::DevelopSettings::default(), false)
    }

    /// A bright dot on black, sharp on the left (near), blurred on the right (far).
    #[test]
    fn far_pixels_blur_and_near_ones_stay_sharp() {
        let (w, h) = (64, 32);
        let mut img = Rgb32f::new(w, h);
        for (x, y) in [(16, 16), (48, 16)] {
            if let Some(p) = img.data.get_mut(y * w + x) {
                *p = [10.0; 3];
            }
        }
        let grid = Plane::from_fn(w, h, |x, _| if x < w / 2 { 0.0 } else { 1.0 });
        let depth = Depth { grid: Arc::new(grid), source: DepthSource::Model };
        let lb = LensBlur { enabled: true, amount: 100.0, focal_distance: 0.0, focal_range: 10.0, ..LensBlur::default() };
        let before: f32 = img.data.iter().map(|p| p[0]).sum();
        blur(&mut img, &lb, &depth, &frame(w, h), 100.0);
        let at = |x: usize, y: usize| img.data[y * w + x][0];
        assert!((at(16, 16) - 10.0).abs() < 1e-4, "the near dot stays sharp");
        assert!(at(48, 16) < 5.0, "the far dot spreads: {}", at(48, 16));
        assert!(at(49, 16) > 0.01 && at(48, 17) > 0.01, "its light lands around it");
        assert!(at(14, 16) == 0.0, "nothing bleeds into the sharp side");
        assert!(img.data.iter().all(|p| p.iter().all(|v| v.is_finite())));
        let after: f32 = img.data.iter().map(|p| p[0]).sum();
        assert!((after - before).abs() / before < 0.35, "light is roughly kept: {before} → {after}");
    }

    /// Boost brightens the bokeh of a highlight only: a mid-grey background blurs to the same level.
    #[test]
    fn boost_brightens_blurred_highlights_only() {
        let (w, h) = (64, 32);
        let mut img0 = Rgb32f::from_fn(w, h, |_, _| [0.2; 3]);
        if let Some(p) = img0.data.get_mut(16 * w + 16) {
            *p = [10.0; 3];
        }
        let depth = Depth { grid: Arc::new(Plane::filled(4, 4, 1.0)), source: DepthSource::Model };
        let run = |boost: f64| {
            let mut img = img0.clone();
            let lb = LensBlur { enabled: true, amount: 100.0, focal_distance: 0.0, focal_range: 10.0, boost, ..LensBlur::default() };
            blur(&mut img, &lb, &depth, &frame(w, h), 100.0);
            img
        };
        let (plain, boosted) = (run(0.0), run(100.0));
        let at = |i: &Rgb32f, x: usize, y: usize| i.data[y * w + x][0];
        assert!(at(&boosted, 17, 16) > at(&plain, 17, 16) * 2.0, "{} vs {}", at(&boosted, 17, 16), at(&plain, 17, 16));
        assert!((at(&boosted, 50, 16) - at(&plain, 50, 16)).abs() < 1e-4, "mid-grey keeps its level");
        // hostile Boost values and pixels: finite, and no brighter than Boost 100
        for b in [f64::NAN, f64::INFINITY, -50.0, 1e300] {
            let img = run(b);
            assert!(img.data.iter().all(|p| p.iter().all(|v| v.is_finite())), "{b}");
            assert!(at(&img, 17, 16) <= at(&boosted, 17, 16) + 1e-4, "{b}");
        }
        assert_eq!(boost_gain([f32::NAN; 3], 1.0), 1.0);
        assert_eq!(boost_gain([0.3; 3], 1.0), 1.0);
        assert_eq!(boost_gain([5.0; 3], f32::NAN), 1.0);
    }

    fn stroke(x: f64, y: f64) -> BrushStroke {
        BrushStroke { points: vec![Point::new(x, y)], size: 0.12, feather: 0.0, flow: 100.0, density: 100.0, ..BrushStroke::default() }
    }

    /// Brush Refinement: a Focus stroke keeps a far dot sharp, a Blur stroke blurs a near one.
    #[test]
    fn the_brushes_overrule_the_depth() {
        let (w, h) = (64, 32);
        let mut img0 = Rgb32f::new(w, h);
        for (x, y) in [(16, 16), (48, 16)] {
            img0.data[y * w + x] = [10.0; 3];
        }
        // left half near (sharp), right half far (blurred)
        let grid = Plane::from_fn(w, h, |x, _| if x < w / 2 { 0.0 } else { 1.0 });
        let depth = Depth { grid: Arc::new(grid), source: DepthSource::Model };
        let base = LensBlur { enabled: true, amount: 100.0, focal_distance: 0.0, focal_range: 10.0, ..LensBlur::default() };
        let run = |lb: &LensBlur| {
            let mut img = img0.clone();
            blur(&mut img, lb, &depth, &frame(w, h), 100.0);
            img
        };
        let at = |i: &Rgb32f, x: usize| i.data[16 * w + x][0];
        let plain = run(&base);
        assert!(at(&plain, 48) < 5.0 && (at(&plain, 16) - 10.0).abs() < 1e-4);
        let focused = run(&LensBlur { focus_strokes: vec![stroke(0.75, 0.5)], ..base.clone() });
        assert!(at(&focused, 48) > at(&plain, 48) * 1.5, "the Focus brush keeps it sharper: {} vs {}", at(&focused, 48), at(&plain, 48));
        let blurred = run(&LensBlur { blur_strokes: vec![stroke(0.25, 0.5)], ..base.clone() });
        assert!(at(&blurred, 16) < 5.0, "the Blur brush blurs the near dot: {}", at(&blurred, 16));
        // the Blur brush has the last word over the Focus brush
        let both = run(&LensBlur { focus_strokes: vec![stroke(0.25, 0.5)], blur_strokes: vec![stroke(0.25, 0.5)], ..base.clone() });
        assert!(at(&both, 16) < 5.0);
        // hostile strokes: finite, no panic
        let bad = BrushStroke {
            points: vec![Point::new(f64::NAN, 0.5), Point::new(1e300, -1e300)],
            size: f64::INFINITY,
            feather: f64::NAN,
            flow: -5.0,
            density: f64::NAN,
            ..BrushStroke::default()
        };
        let img = run(&LensBlur { focus_strokes: vec![bad.clone()], blur_strokes: vec![bad, BrushStroke::default()], ..base });
        assert!(img.data.iter().all(|p| p.iter().all(|v| v.is_finite())));
    }

    fn radial_mask(lens_blur: f64) -> Mask {
        let shape = MaskShape::Radial { center: Point::new(0.25, 0.5), rx: 0.15, ry: 0.3, angle: 0.0, feather: 0.0, invert: false };
        Mask {
            id: 1,
            components: vec![lcc_develop::MaskComponent { anchor: None, name: None, op: lcc_develop::MaskOp::Add, invert: false, shape }],
            adjust: lcc_develop::LocalAdjustments { lens_blur, ..Default::default() },
            ..Default::default()
        }
    }

    /// A mask's Lens Blur blurs what it selects, or what it leaves out, without the global blur.
    #[test]
    fn a_mask_blurs_its_selection_or_the_outside() {
        let (w, h) = (64, 32);
        let mut img0 = Rgb32f::new(w, h);
        for (x, y) in [(16, 16), (48, 16)] {
            img0.data[y * w + x] = [10.0; 3];
        }
        let run = |m: Mask| {
            let s = DevelopSettings { masks: vec![m], ..Default::default() };
            let mut img = img0.clone();
            blur_masks(&mut img, &s, &frame(w, h), 100.0, None);
            (img, s)
        };
        let at = |i: &Rgb32f, x: usize| i.data[16 * w + x][0];
        let (inside, s) = run(radial_mask(100.0));
        assert!(at(&inside, 16) < 5.0 && (at(&inside, 48) - 10.0).abs() < 1e-4, "{} {}", at(&inside, 16), at(&inside, 48));
        assert!(masks_blur(&s) && renders(&s, None) && key(&s, None).is_some() && reach(&s, 100.0) >= 3.0);
        let mut m = radial_mask(100.0);
        m.adjust.lens_blur_outside = true;
        let (outside, _) = run(m);
        assert!(at(&outside, 48) < 5.0 && (at(&outside, 16) - 10.0).abs() < 1e-4, "{} {}", at(&outside, 16), at(&outside, 48));
        // switched off, hidden, zero or hostile: nothing, or finite
        let mut off = radial_mask(100.0);
        off.adjust.lens_blur_off = true;
        let mut hidden = radial_mask(100.0);
        hidden.visible = false;
        for m in [off, hidden, radial_mask(0.0), radial_mask(f64::NAN), radial_mask(-40.0)] {
            let (img, s) = run(m);
            assert_eq!(img.data, img0.data);
            assert!(!masks_blur(&s) && key(&s, None).is_none() && reach(&s, 100.0) == 0.0);
        }
        let mut wild = radial_mask(1e300);
        wild.adjust.lens_blur_boost = f64::NAN;
        wild.adjust.amount = f64::INFINITY;
        let (img, _) = run(wild);
        assert!(img.data.iter().all(|p| p.iter().all(|v| v.is_finite())));
    }

    #[test]
    fn off_zero_or_degenerate_inputs_leave_the_image_alone() {
        let img0 = Rgb32f::from_fn(8, 8, |x, y| [x as f32, y as f32, 1.0]);
        let depth = Depth { grid: Arc::new(Plane::filled(4, 4, 1.0)), source: DepthSource::Subject };
        for lb in [
            LensBlur { enabled: false, ..LensBlur::default() },
            LensBlur { enabled: true, amount: 0.0, ..LensBlur::default() },
            LensBlur { enabled: true, amount: f64::NAN, ..LensBlur::default() },
        ] {
            let mut img = img0.clone();
            blur(&mut img, &lb, &depth, &frame(8, 8), 1000.0);
            if !lb.active() {
                assert_eq!(img.data, img0.data);
            }
        }
        // hostile scale and empty grids
        let lb = LensBlur { enabled: true, amount: 100.0, ..LensBlur::default() };
        for (ppl, grid) in [(f64::INFINITY, Plane::filled(4, 4, 1.0)), (-5.0, Plane::filled(4, 4, 1.0)), (100.0, Plane::new(0, 0))] {
            let mut img = img0.clone();
            blur(&mut img, &lb, &Depth { grid: Arc::new(grid), source: DepthSource::Model }, &frame(8, 8), ppl);
            assert!(img.data.iter().all(|p| p.iter().all(|v| v.is_finite())));
        }
        let mut empty = Rgb32f::new(0, 0);
        blur(&mut empty, &lb, &depth, &frame(1, 1), 100.0);
        assert!(subject_depth(&Plane::filled(2, 1, f32::NAN)).data.iter().all(|v| *v == 1.0));
    }

    fn on() -> lcc_develop::DevelopSettings {
        lcc_develop::DevelopSettings {
            lens_blur: LensBlur { enabled: true, amount: 100.0, focal_distance: 0.0, focal_range: 10.0, ..LensBlur::default() },
            ..Default::default()
        }
    }

    fn mean_diff(a: &lcc_raster::Rgba8, b: &lcc_raster::Rgba8) -> f32 {
        a.data.iter().zip(&b.data).map(|(p, q)| (p[1] as f32 - q[1] as f32).abs()).sum::<f32>() / a.len().max(1) as f32
    }

    /// Without the depth model, the render blurs from the subject and says so; the edit is cached by its settings.
    #[test]
    fn a_render_without_the_depth_model_blurs_and_warns() {
        use crate::{RenderRequest, SourceInfo, render};
        let src = lcc_scenes::demo_library()[0].render(240, 160);
        let info = SourceInfo::default();
        let req = RenderRequest::fit(240, 160);
        let plain = render(&src, &info, &lcc_develop::DevelopSettings::default(), &req);
        assert!(!plain.warnings.iter().any(|w| w.starts_with("Lens Blur")));
        let blurred = render(&src, &info, &on(), &req);
        assert!(blurred.warnings.iter().any(|w| w == NO_MODEL_WARNING), "{:?}", blurred.warnings);
        assert!(mean_diff(&plain.image, &blurred.image) > 0.5, "Lens Blur changed nothing");
        // switched off with the section's eye: as if it weren't there
        let mut off = on();
        off.set_section_enabled("lensBlur", false);
        assert!(mean_diff(&plain.image, &render(&src, &info, &off, &req).image) < 1e-3);
    }

    /// Previews match exports: the blur at two sizes agrees after downscaling.
    #[test]
    fn the_blur_is_resolution_independent() {
        use crate::{RenderRequest, SourceInfo, render};
        let src = lcc_scenes::demo_library()[3].render(480, 320);
        let s = on();
        let small = render(&src, &SourceInfo::default(), &s, &RenderRequest::fit(120, 120)).image;
        let big = render(&src, &SourceInfo::default(), &s, &RenderRequest::fit(480, 480)).image;
        let down = lcc_raster::resample::resize(&big.to_linear(), small.width, small.height, lcc_raster::resample::Filter::Box).to_srgb8();
        let plain = render(&src, &SourceInfo::default(), &lcc_develop::DevelopSettings::default(), &RenderRequest::fit(120, 120)).image;
        let err = mean_diff(&small, &down);
        assert!(err < 6.0, "mean abs error {err}");
        assert!(err < mean_diff(&small, &plain), "the sizes agree better than blurred vs sharp");
    }
}
