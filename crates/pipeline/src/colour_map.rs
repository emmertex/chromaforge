//! The post-tone colour map of a base curve (`BaseCurve::colour_map`, `docs/base-curve-format.md`): a chroma gain and a hue
//! shift on a grid of hue x lightness x chroma knots, applied in OkLab after the tone stage and the hue-preserving highlight
//! clip, before the colour tools.
//!
//! Each colour is taken to OkLab; its hue (the circle wraps), lightness and chroma are looked up in the grid by linear
//! interpolation (lightness and chroma hold their end values outside the knots). The chroma is multiplied by the gain and the
//! hue turned by the shift, both scaled by a fade that is 0 below chroma [`FADE_FROM`] and 1 from [`FADE_TO`], so a grey or a
//! near-grey passes untouched (not even the OkLab round trip touches it). The OkLab lightness is kept.
//! `finish.wgsl` (`colour_map_apply`) is the same function; keep them in step.

use lcc_color::perceptual::{oklab_from_2020, oklab_to_2020};
use lcc_develop::base_curve::ColourMap;

/// OkLab chroma below which the map does nothing.
pub const FADE_FROM: f32 = 0.008;
/// OkLab chroma from which the map acts in full.
pub const FADE_TO: f32 = 0.035;

/// A colour map as the renderer reads it: knots and per-knot values in `f32`, the shifts in radians. Built only for a map that
/// changes something ([`ColourMapK::new`]).
#[derive(Clone, Debug, PartialEq)]
pub struct ColourMapK {
    hues: usize,
    lightness: Vec<f32>,
    chroma: Vec<f32>,
    /// chroma x lightness x hues
    gain: Vec<f32>,
    /// the same order, radians
    shift: Vec<f32>,
}

/// The knot below `x` and how far `x` is towards the next one (0 at and below the first knot, 1 at and above the last).
#[inline]
fn locate(knots: &[f32], x: f32) -> (usize, f32) {
    let n = knots.len();
    if n < 2 {
        return (0, 0.0);
    }
    let mut i = 0;
    while i + 2 < n && x >= knots[i + 1] {
        i += 1;
    }
    (i, ((x - knots[i]) / (knots[i + 1] - knots[i])).clamp(0.0, 1.0))
}

impl ColourMapK {
    /// The renderer's form of a file's map; `None` when it changes nothing (so a map that changes nothing is exactly no map).
    pub fn new(m: &ColourMap) -> Option<ColourMapK> {
        if m.is_identity() {
            return None;
        }
        Some(ColourMapK {
            hues: m.hues,
            lightness: m.lightness.iter().map(|v| *v as f32).collect(),
            chroma: m.chroma.iter().map(|v| *v as f32).collect(),
            gain: m.gain.iter().map(|v| *v as f32).collect(),
            shift: m.shift.iter().map(|v| v.to_radians() as f32).collect(),
        })
    }

    pub fn hues(&self) -> usize {
        self.hues
    }

    pub fn lightness_knots(&self) -> usize {
        self.lightness.len()
    }

    pub fn chroma_knots(&self) -> usize {
        self.chroma.len()
    }

    /// What the GPU reads: the lightness knots, the chroma knots, the gains, then the shifts (radians).
    pub fn table(&self) -> Vec<f32> {
        let mut t = Vec::with_capacity(self.lightness.len() + self.chroma.len() + 2 * self.gain.len());
        t.extend_from_slice(&self.lightness);
        t.extend_from_slice(&self.chroma);
        t.extend_from_slice(&self.gain);
        t.extend_from_slice(&self.shift);
        t
    }

    /// The map on a display-linear Rec.2020 colour (non-finite colours pass unchanged).
    pub fn apply(&self, d: [f32; 3]) -> [f32; 3] {
        let lab = oklab_from_2020(d);
        let c = lab[1].hypot(lab[2]);
        let t = ((c - FADE_FROM) / (FADE_TO - FADE_FROM)).clamp(0.0, 1.0);
        let fade = t * t * (3.0 - 2.0 * t);
        if !fade.is_finite() || fade <= 0.0 || !lab[0].is_finite() {
            return d;
        }
        let nh = self.hues;
        let nl = self.lightness.len();
        let mut u = lab[2].atan2(lab[1]) / std::f32::consts::TAU * nh as f32;
        if u < 0.0 {
            u += nh as f32;
        }
        let fu = u.floor();
        let (h0, fh) = ((fu as usize) % nh, u - fu);
        let h1 = (h0 + 1) % nh;
        let (l0, fl) = locate(&self.lightness, lab[0]);
        let (c0, fc) = locate(&self.chroma, c);
        let l1 = (l0 + 1).min(nl - 1);
        let c1 = (c0 + 1).min(self.chroma.len() - 1);
        let (mut g, mut s) = (0.0f32, 0.0f32);
        for (ci, wc) in [(c0, 1.0 - fc), (c1, fc)] {
            for (li, wl) in [(l0, 1.0 - fl), (l1, fl)] {
                for (hi, wh) in [(h0, 1.0 - fh), (h1, fh)] {
                    let w = wc * wl * wh;
                    let i = (ci * nl + li) * nh + hi;
                    g += w * self.gain[i];
                    s += w * self.shift[i];
                }
            }
        }
        let g = 1.0 + (g - 1.0) * fade;
        let (sn, cs) = (s * fade).sin_cos();
        oklab_to_2020([lab[0], g * (lab[1] * cs - lab[2] * sn), g * (lab[1] * sn + lab[2] * cs)])
    }
}
