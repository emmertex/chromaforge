//! Recolor: a mask's area takes a new colour and keeps its own shading, folds and light (red
//! boots to purple boots with the same contrast).
//!
//! The maths works in OkLab on display-linear colour (after the tone map, local tone and the
//! colour stage, where the colour overlay and colour grading also work). It needs the area's
//! **main colour**, its [`Reference`]: the dominant hue cluster of the area's coloured pixels (a
//! hue histogram weighted by chroma × alpha, its peak and spread), with that cluster's lightness
//! and chroma. Dust, laces, a white lining or a logo inside the mask don't pull it. It is measured
//! once per render on a fixed-size proxy of the whole frame ([`measure`]), so a preview, an export
//! and the GPU all recolor from the same numbers. "Colour to Replace" picks the main hue instead.
//!
//! Per pixel ([`apply`]):
//! - **Membership**: only pixels of the main colour change. A pixel's weight falls smoothly with
//!   its hue distance from the main hue and with its saturation (chroma over lightness) relative
//!   to the main colour's, so grey dust, a white lining, black rubber or a blue star inside the
//!   mask keep their colour. The Range setting widens or narrows that window. A grey area (a black
//!   shirt) has no main hue: all of it changes.
//! - **Lightness** keeps its proportion to the main colour's below it (a fold half as light stays
//!   half as light), and above it the room left up to white is shared out so nothing passes white.
//!   A target as light as the main colour leaves lightness exactly as it was.
//! - **Chroma** is the target's times the pixel's chroma over the main chroma, softly limited
//!   above, and faded out at very low and very high lightness so dark edges and highlights don't
//!   take saturated colour. A grey area takes its chroma from its shading, at most the target's,
//!   fading out above the area's own lightness.
//! - **Hue** is the target's, plus the pixel's own offset from the main hue (limited), so coloured
//!   light stays.
//!
//! The GPU kernel (`finish.wgsl`, `recolor_one`) repeats these steps and constants.
//!
//! Ported from upstream (branch `feat/83-recolour`,
//! `crates/pipeline/src/recolor/`, MIT OR Apache-2.0), adapted to this pipeline's finish stage;
//! see `fork/recolour.md` for the behaviour notes.

pub(crate) mod measure;

use lcc_color::perceptual::{lab_to_lch, oklab_from_2020};
use lcc_color::transfer::srgb_to_linear;
use lcc_color::{REC2020, SRGB};
use lcc_develop::LocalAdjustments;

/// Below this main chroma an area counts as grey: all of it changes, and its chroma comes from its
/// shading.
pub const GREY_CHROMA: [f32; 2] = [0.02, 0.06];
/// Most a pixel's chroma may exceed the main chroma, as a factor: a ratio above 1 approaches it
/// softly ([`chroma_ratio`]), so a bright streak doesn't turn neon.
pub const MAX_CHROMA_RATIO: f32 = 1.75;
/// Most a pixel's hue keeps of its offset from the main hue (radians).
pub const MAX_HUE_OFFSET: f32 = 0.5;
/// New chroma fades in over this lightness range (dark edges stay dark and nearly grey) ...
pub const DARK_FADE: [f32; 2] = [0.04, 0.2];
/// ... and out over this one (highlights stay near white).
pub const LIGHT_FADE: [f32; 2] = [0.86, 0.99];
/// Smallest reference lightness (an all-black area still has a defined proportion).
pub const MIN_REF_L: f32 = 0.02;
/// Hue half-width (radians) of the membership window at Range 0, from the main colour's spread:
/// `spread × k + c`, clamped.
pub const HUE_WINDOW: [f32; 4] = [1.5, 0.22, 0.28, 0.9];
/// A pixel whose saturation is below 0.6 × this share of the main colour's is not of it (at Range
/// 0); from 2.5 × it is, with a wide, smooth step between (JPEG noise must not show as blocks).
pub const SAT_FLOOR: f32 = 0.15;
/// Below this chroma a pixel's hue is too weak to count (black rubber, dust with a tint): it is
/// not of the main colour; above the second value its chroma no longer holds it back.
pub const MIN_CHROMA: [f32; 2] = [0.01, 0.025];
/// Weakly coloured pixels near the main hue (the greyish sheen of a glossy object) take the new
/// hue at their own chroma: from none at the first chroma to all at the second, and fading out
/// again between the last two (a clearly coloured pixel either belongs or keeps its colour).
pub const HUE_ONLY_CHROMA: [f32; 4] = [0.015, 0.03, 0.04, 0.08];
/// Number of terms a mask's recolor carries in the per-mask term block
/// ([`crate::finish::MASK_RECOLOR`]): strength, target OkLab (3), main lightness, chroma, the
/// cosine and sine of its hue, the hue window and the saturation floor.
pub const TERMS: usize = 10;

/// The main colour of a masked area before recoloring.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reference {
    /// Lightness of the main colour (OkLab L).
    pub l: f32,
    /// Chroma of the main colour (OkLab).
    pub c: f32,
    /// Cosine and sine of the main hue.
    pub h: [f32; 2],
    /// Spread of the main hue cluster (radians, weighted RMS distance from the main hue).
    pub spread: f32,
}

/// The membership window: hue half-width (radians) and saturation floor (a share of the main
/// colour's).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Window {
    pub hue: f32,
    pub sat: f32,
}

impl Window {
    /// The window of reference `r` at Range `range` (−100..100; 0 = the default, each 50 doubles
    /// or halves it).
    pub fn new(r: &Reference, range: f64) -> Window {
        let f = if range.is_finite() { 2f32.powf((range.clamp(-100.0, 100.0) / 50.0) as f32) } else { 1.0 };
        let [k, c, lo, hi] = HUE_WINDOW;
        let base = (r.spread.max(0.0) * k + c).clamp(lo, hi);
        Window { hue: (base * f).min(std::f32::consts::PI), sat: SAT_FLOOR / f }
    }
}

/// Is the recolor of `j` on (an amount above 0)?
pub fn is_on(j: &LocalAdjustments) -> bool {
    j.recolor.is_finite() && j.recolor > 0.0
}

/// The target colour of `j` (sRGB-encoded 0..1 channels) as OkLab of display-linear Rec.2020.
pub fn target_lab(j: &LocalAdjustments) -> [f32; 3] {
    srgb_to_oklab([j.recolor_r, j.recolor_g, j.recolor_b])
}

/// The Colour to Replace of `j` (`None`: automatic, the area's main colour). Black means unset.
pub fn source_rgb(j: &LocalAdjustments) -> Option<[f64; 3]> {
    let rgb = [j.recolor_from_r, j.recolor_from_g, j.recolor_from_b];
    rgb.iter().any(|v| v.is_finite() && *v > 0.0).then_some(rgb)
}

/// OkLab of an sRGB-encoded colour (channels clamped to 0..1, NaN as 0).
pub fn srgb_to_oklab(rgb: [f64; 3]) -> [f32; 3] {
    let lin = rgb.map(|v| srgb_to_linear(if v.is_finite() { v.clamp(0.0, 1.0) as f32 } else { 0.0 }));
    oklab_from_2020(SRGB.to_space(&REC2020).apply_f32(lin))
}

/// `"#7a3fb0"` (or `"7a3fb0"`, `"#73b"`) as sRGB-encoded 0..1 channels.
pub fn parse_hex(s: &str) -> Option<[f64; 3]> {
    let h = s.trim().trim_start_matches('#');
    if !h.is_ascii() {
        return None;
    }
    let digits: Vec<u8> = match h.len() {
        3 => h.chars().map(|c| c.to_digit(16).map(|d| (d * 17) as u8)).collect::<Option<_>>()?,
        6 => (0..3).map(|i| h.get(2 * i..2 * i + 2).and_then(|p| u8::from_str_radix(p, 16).ok())).collect::<Option<_>>()?,
        _ => return None,
    };
    let [r, g, b] = digits.as_slice() else { return None };
    Some([*r, *g, *b].map(|v| f64::from(v) / 255.0))
}

/// sRGB-encoded 0..1 channels as `"#rrggbb"` (out-of-range and NaN channels clamped).
pub fn to_hex(rgb: [f64; 3]) -> String {
    let [r, g, b] = rgb.map(|v| (if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 } * 255.0).round() as u8);
    format!("#{r:02x}{g:02x}{b:02x}")
}

#[inline]
fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// `a` wrapped to −π..π.
#[inline]
pub(crate) fn wrap(a: f32) -> f32 {
    let t = std::f32::consts::TAU;
    (a + std::f32::consts::PI).rem_euclid(t) - std::f32::consts::PI
}

/// Hue of OkLab `(a, b)` (0 for a neutral one).
#[inline]
pub(crate) fn hue(a: f32, b: f32) -> f32 {
    if a == 0.0 && b == 0.0 { 0.0 } else { b.atan2(a) }
}

/// Saturation as membership reads it: chroma over lightness.
#[inline]
pub(crate) fn saturation(c: f32, l: f32) -> f32 {
    c / l.max(0.05)
}

/// The new lightness of a pixel of lightness `l` in an area whose main colour has lightness `lr`,
/// recolored to lightness `lt`.
#[inline]
pub fn lightness(l: f32, lr: f32, lt: f32) -> f32 {
    let lr = lr.max(MIN_REF_L);
    let lt = lt.max(0.0);
    if l <= lr {
        return lt * l / lr;
    }
    // above the main colour: proportional, but never so steep that white passes white
    let s = (lt / lr).min((1.0 - lt).max(0.0) / (1.0 - lr).max(1e-3));
    lt + (l - lr) * s
}

/// A pixel's chroma `c` over the main chroma `rc`, softly limited to [`MAX_CHROMA_RATIO`] above 1.
#[inline]
pub fn chroma_ratio(c: f32, rc: f32) -> f32 {
    if rc <= 1e-6 {
        return 0.0;
    }
    let r = (c / rc).max(0.0);
    let span = MAX_CHROMA_RATIO - 1.0;
    if r > 1.0 { 1.0 + span * (1.0 - (-(r - 1.0) / span).exp()) } else { r }
}

/// How much a pixel (lightness `l`, chroma `c`, hue `h`) belongs to the main colour of `r` under
/// window `w` (0..1, smooth): `[full, hue]`, the weight of the whole change and the weight of a
/// change of hue alone (at the pixel's own chroma, for weakly coloured pixels; never less than
/// `full`). A grey area (no main hue) takes every pixel.
pub fn membership(l: f32, c: f32, h: f32, r: &Reference, w: &Window) -> [f32; 2] {
    let coloured = smooth(GREY_CHROMA[0], GREY_CHROMA[1], r.c);
    let dh = wrap(h - hue(r.h[0], r.h[1])).abs();
    let by_hue = 1.0 - smooth(w.hue, w.hue * 2.0 + 0.1, dh);
    let q = saturation(c, l) / saturation(r.c, r.l).max(1e-4);
    let by_sat = smooth(w.sat * 0.6, w.sat * 2.5, q) * smooth(MIN_CHROMA[0], MIN_CHROMA[1], c);
    let full = 1.0 + (by_hue * by_sat - 1.0) * coloured;
    // a weak colour's hue is unreliable: a wider hue window, for the hue alone
    let [c0, c1, c2, c3] = HUE_ONLY_CHROMA;
    let loose = 1.0 - smooth(w.hue * 2.0, w.hue * 3.2 + 0.1, dh);
    let weak = smooth(c0, c1, c) * (1.0 - smooth(c2, c3, c));
    [full, full.max(loose * weak * coloured)]
}

/// The pixel `lab` (OkLab) of an area with main colour `r`, recolored to `target` (OkLab) at
/// strength `k` (0..1; 0 or less returns `lab` unchanged, exactly) inside window `w`.
pub fn apply(lab: [f32; 3], target: [f32; 3], r: &Reference, w: &Window, k: f32) -> [f32; 3] {
    if k.is_nan() || k <= 0.0 {
        return lab;
    }
    let [l, c, h] = lab_to_lch(lab);
    let [full, hue_w] = membership(l, c, h, r, w);
    let k0 = k.min(1.0);
    let (k, k_hue) = (k0 * full, k0 * (hue_w - full).max(0.0));
    if k.is_nan() || k_hue.is_nan() || (k <= 0.0 && k_hue <= 0.0) {
        return lab;
    }
    let [lt, ct, _] = lab_to_lch(target);
    let ht = hue(target[1], target[2]);
    let l1 = lightness(l, r.l, lt);
    let coloured = smooth(GREY_CHROMA[0], GREY_CHROMA[1], r.c);
    // a grey area: chroma from its shading, never above the target's, fading out above its own lightness
    let grey = if lt > 1e-6 { (l1 / lt).clamp(0.0, 1.0) } else { 1.0 } * (1.0 - smooth(lt, lt.max(0.95) + 0.05, l1));
    let ends = smooth(DARK_FADE[0], DARK_FADE[1], l1) * (1.0 - smooth(LIGHT_FADE[0], LIGHT_FADE[1], l1));
    let c1 = ct * (grey + (chroma_ratio(c, r.c) - grey) * coloured) * ends;
    // hue: the target's, keeping the pixel's own offset from the main hue where both are clearly coloured
    let keep = coloured * smooth(GREY_CHROMA[0], GREY_CHROMA[1], c);
    let h1 = ht + (wrap(h - hue(r.h[0], r.h[1])) * keep).clamp(-MAX_HUE_OFFSET, MAX_HUE_OFFSET);
    let (s, co) = h1.sin_cos();
    let new = [l1, c1 * co, c1 * s];
    let turned = [l1, c * co, c * s];
    let out: [f32; 3] = std::array::from_fn(|i| lab[i] + (new[i] - lab[i]) * k + (turned[i] - lab[i]) * k_hue);
    if out.iter().all(|v| v.is_finite()) { out } else { lab }
}

/// The recolor terms of mask adjustments `j` at Amount `amount` (1 = 100 %) with main colour `r`:
/// zeros (off) without a reference or with nothing to do. Layout: [`TERMS`].
pub fn terms(j: &LocalAdjustments, amount: f32, r: Option<&Reference>) -> [f32; TERMS] {
    let (Some(r), true) = (r, is_on(j) && amount > 0.0) else { return [0.0; TERMS] };
    let k = ((j.recolor / 100.0) as f32).clamp(0.0, 1.0) * amount;
    let t = target_lab(j);
    let w = Window::new(r, j.recolor_range);
    [k, t[0], t[1], t[2], r.l, r.c, r.h[0], r.h[1], w.hue, w.sat]
}

/// Recolors `lab` with the terms `t` ([`terms`]) at mask alpha `alpha`.
#[inline]
pub fn apply_terms(lab: [f32; 3], t: &[f32], alpha: f32) -> [f32; 3] {
    let &[k, tl, ta, tb, rl, rc, hc, hs, wh, ws] = t else { return lab };
    if k.is_nan() || k <= 0.0 || alpha.is_nan() || alpha <= 0.0 {
        return lab;
    }
    let r = Reference { l: rl, c: rc, h: [hc, hs], spread: 0.0 };
    apply(lab, [tl, ta, tb], &r, &Window { hue: wh, sat: ws }, k * alpha)
}

/// Recolors every pixel of `img` (sRGB-encoded 0..1, row-major RGB) under `alpha` (0..1 per pixel)
/// to `target` (sRGB-encoded 0..1) at `amount` (0..1) and Range `range`, finding the main colour on
/// the image itself (or around `from`, the Colour to Replace). What the `recolor` example runs on a
/// photo and a mask, outside the app.
pub fn recolor_srgb(img: &mut [[f32; 3]], alpha: &[f32], target: [f64; 3], amount: f32, range: f64, from: Option<[f64; 3]>) {
    let fwd = SRGB.to_space(&REC2020);
    let to_lab = |c: [f32; 3]| oklab_from_2020(fwd.apply_f32(c.map(srgb_to_linear)));
    let samples: Vec<([f32; 3], f32)> = img.iter().zip(alpha).map(|(px, a)| (to_lab(*px), *a)).collect();
    let Some(r) = measure::main_colour(&samples, from.map(srgb_to_oklab)) else { return };
    let w = Window::new(&r, range);
    let t = srgb_to_oklab(target);
    let back = REC2020.to_space(&SRGB);
    for ((px, a), (lab, _)) in img.iter_mut().zip(alpha).zip(&samples) {
        if a.is_nan() || *a <= 0.0 {
            continue;
        }
        let out = apply(*lab, t, &r, &w, amount * a);
        let lin = back.apply_f32(lcc_color::perceptual::oklab_to_2020(out));
        *px = lin.map(|v| lcc_color::transfer::linear_to_srgb(v.clamp(0.0, 1.0)));
    }
}

/// Patches `terms` (one per mask, in `ids`' order, [`lcc_pipeline::finish::MASK_TERMS`] each) with
/// each mask's recolor terms ([`TERMS`] at [`lcc_pipeline::finish::MASK_RECOLOR`]) from `refs`
/// (main colours by mask id, [`measure::references`]); a mask without a reference stays off.
pub fn with_recolor(terms: &mut [[f32; crate::finish::MASK_TERMS]], ids: &[u32], adjusts: &[LocalAdjustments], refs: &[(u32, Reference)]) {
    if terms.len() != ids.len() || adjusts.len() != ids.len() {
        return;
    }
    for ((t, id), j) in terms.iter_mut().zip(ids).zip(adjusts) {
        let r = refs.iter().find(|(rid, _)| rid == id).map(|(_, r)| r);
        let block = recolor_terms(j, r);
        if let Some(dst) = t.get_mut(crate::finish::MASK_RECOLOR..crate::finish::MASK_RECOLOR + TERMS) {
            dst.copy_from_slice(&block);
        }
    }
}

/// The term block of one mask ([`terms`], at the mask's own Amount).
fn recolor_terms(j: &LocalAdjustments, r: Option<&Reference>) -> [f32; TERMS] {
    terms(j, (j.amount / 100.0) as f32, r)
}

#[cfg(test)]
mod tests;
