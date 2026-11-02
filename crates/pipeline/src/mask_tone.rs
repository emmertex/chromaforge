//! The tones a mask's Whites and Blacks act on, fitted to that mask (as Lightroom's local Whites / Blacks adapt to
//! what is under the mask): Whites rises from the middle of the mask's tones to full at its brightest, Blacks falls
//! from full at its darkest to nothing at its middle. A white dress's highlights then move as much as a sky's, and
//! a dark suit's shadows as much as a night scene's.
//!
//! The tones are measured once per mask on a fixed-size render of the whole frame (white-balanced and exposed,
//! [`EDGE`] px on the long side), whatever is being rendered: the preview, a zoomed window and the export all
//! use the same ranges, so they agree. The last result is kept, keyed by everything it depends on.

use std::sync::{Mutex, PoisonError};

use lcc_develop::{DevelopSettings, Mask};
use lcc_raster::{Plane, Rgb32f};

use crate::SourceInfo;
use crate::finish::DEFAULT_MASK_TONE;
use crate::geometry::Frame;
use crate::local::{frame_proxy, log_lum, white_balance};
use crate::masks::{self, Mattes};


/// Long edge of the render the tones are measured on.
pub const EDGE: usize = 384;

/// The shares of the mask's weight below which its darkest and its middle tones lie, and above which its brightest.
const DARK_Q: f64 = 0.01;
const MID_Q: f64 = 0.5;
const BRIGHT_Q: f64 = 0.99;

/// The narrowest a range is (EV), so a mask of nearly one tone still has a ramp.
const MIN_SPAN: f32 = 1.0;
/// The widest a range is (EV), as wide as the fixed ranges were.
const WHITES_SPAN: f32 = 4.0;
const BLACKS_SPAN: f32 = 6.5;

/// A mask must hold at least this much of the measured render (in pixels' worth of weight) to be measured.
const MIN_WEIGHT: f64 = 16.0;

/// Histogram range (log2 luminance over middle grey) and its bins.
const LO: f32 = -12.0;
const HI: f32 = 6.0;
const BINS: usize = 360;

type Cached = (u64, Vec<[f32; 4]>);
static LAST: Mutex<Option<Cached>> = Mutex::new(None);

/// The masks whose Whites or Blacks do something.
fn wants(m: &Mask) -> bool {
    m.adjust.whites != 0.0 || m.adjust.blacks != 0.0
}

/// One range per visible mask (the order [`masks::evaluate`] uses), or nothing when no visible mask has Whites or
/// Blacks. A mask without them, or with too little under it to measure, keeps [`DEFAULT_MASK_TONE`].
pub(crate) fn ranges(src: &Rgb32f, info: &SourceInfo, s: &DevelopSettings, frame: &Frame, mattes: Option<&Mattes>) -> Vec<[f32; 4]> {
    let visible: Vec<&Mask> = s.masks.iter().filter(|m| m.visible && !m.components.is_empty()).collect();
    if !visible.iter().any(|m| wants(m)) {
        return Vec::new();
    }
    let (w, h) = frame.fit(EDGE, EDGE);
    if w == 0 || h == 0 || src.width == 0 || src.height == 0 {
        return vec![DEFAULT_MASK_TONE; visible.len()];
    }
    let ev = s.light.exposure as f32;
    let key = {
        use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};
        let exposures: Vec<(u64, u64, u64)> =
            visible.iter().map(|m| (m.adjust.exposure.to_bits(), u64::from(wants(m)), m.adjust.amount.to_bits())).collect();
        let (t, tint) = crate::local::effective_wb(info, s);
        // (what the masks' alphas depend on: their components and amounts, the frame, the mattes)
        let shapes: Vec<String> = visible.iter().map(|m| format!("{}{}{}{:?}", m.invert, m.refine, m.feather, m.components)).collect();
        let mattes_id = mattes.map(|m| (std::ptr::from_ref(m) as usize, m.bytes()));
        BuildHasherDefault::<DefaultHasher>::default().hash_one((
            shapes,
            mattes_id,
            (w, h),
            crate::optics::fingerprint(src),
            format!("{frame:?}"),
            [t.to_bits(), tint.to_bits(), info.as_shot_temp.to_bits(), info.as_shot_tint.to_bits()],
            ev.to_bits(),
            exposures,
        ))
    };
    if let Some((k, v)) = LAST.lock().unwrap_or_else(PoisonError::into_inner).as_ref()
        && *k == key
        && v.len() == visible.len()
    {
        return v.clone();
    }
    let mut img = Rgb32f::clone(&frame_proxy(src, frame, w, h));
    white_balance(&mut img, info, s);
    let log_l = img.map(log_lum);
    let out: Vec<[f32; 4]> = visible
        .iter()
        .map(|m| {
            if !wants(m) {
                return DEFAULT_MASK_TONE;
            }
            let alpha = masks::evaluate_one(m, frame, w, h, &img, &log_l, ev, mattes);
            fit(&alpha, &log_l, ev + m.adjust.exposure as f32).unwrap_or(DEFAULT_MASK_TONE)
        })
        .collect();
    *LAST.lock().unwrap_or_else(PoisonError::into_inner) = Some((key, out.clone()));
    out
}

/// The range of the tones under `alpha` (log luminance `log_l`, shifted by `ev`): `[whites from, whites full,
/// blacks full, blacks to]`; `None` when the mask holds too little to measure.
pub(crate) fn fit(alpha: &Plane, log_l: &Plane, ev: f32) -> Option<[f32; 4]> {
    let mut hist = [0.0f64; BINS];
    let mut total = 0.0f64;
    for (a, l) in alpha.data.iter().zip(&log_l.data) {
        let (a, l) = (*a, *l + ev);
        if !(a > 0.0 && a.is_finite() && l.is_finite()) {
            continue;
        }
        let b = (((l - LO) / (HI - LO)) * BINS as f32).clamp(0.0, (BINS - 1) as f32) as usize;
        if let Some(slot) = hist.get_mut(b) {
            *slot += f64::from(a);
            total += f64::from(a);
        }
    }
    if total < MIN_WEIGHT {
        return None;
    }
    let at = |q: f64| -> f32 {
        let want = q * total;
        let mut run = 0.0;
        for (i, v) in hist.iter().enumerate() {
            run += v;
            if run >= want {
                return LO + (i as f32 + 0.5) * (HI - LO) / BINS as f32;
            }
        }
        HI
    };
    let (dark, mid, bright) = (at(DARK_Q), at(MID_Q), at(BRIGHT_Q));
    let w_hi = bright.clamp(-3.0, 3.5);
    let w_lo = mid.min(w_hi - MIN_SPAN).max(w_hi - WHITES_SPAN);
    let b_lo = dark.clamp(-9.0, 2.0);
    let b_hi = mid.max(b_lo + MIN_SPAN).min(b_lo + BLACKS_SPAN);
    Some([w_lo, w_hi, b_lo, b_hi])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane(v: &[f32]) -> Plane {
        Plane { width: v.len(), height: 1, data: v.to_vec() }
    }

    #[test]
    fn a_bright_mask_s_whites_reach_full_at_its_own_brightest_tones() {
        // a white dress: everything under the mask between +0.5 and +1.5 EV
        let l: Vec<f32> = (0..400).map(|i| 0.5 + i as f32 / 400.0).collect();
        let [w_lo, w_hi, b_lo, b_hi] = fit(&plane(&vec![1.0; 400]), &plane(&l), 0.0).unwrap();
        assert!((w_hi - 1.5).abs() < 0.1, "{w_hi}");
        assert!(w_lo < w_hi - 0.99 && w_lo >= w_hi - WHITES_SPAN, "{w_lo}");
        assert!((b_lo - 0.5).abs() < 0.1 && b_hi >= b_lo + MIN_SPAN, "{b_lo} {b_hi}");
        // the fixed range barely reached the dress's top: full only at +2.6
        assert!(w_hi < DEFAULT_MASK_TONE[1]);
    }

    #[test]
    fn the_exposure_shifts_the_range_and_an_empty_mask_has_none() {
        let l: Vec<f32> = (0..100).map(|i| -4.0 + i as f32 / 25.0).collect();
        let a = fit(&plane(&vec![1.0; 100]), &plane(&l), 0.0).unwrap();
        let b = fit(&plane(&vec![1.0; 100]), &plane(&l), 1.0).unwrap();
        assert!((b[1] - a[1] - 1.0).abs() < 0.11 && (b[2] - a[2] - 1.0).abs() < 0.11, "{a:?} {b:?}");
        assert_eq!(fit(&plane(&vec![0.0; 100]), &plane(&l), 0.0), None);
        assert_eq!(fit(&plane(&[1.0, f32::NAN]), &plane(&[f32::NAN, 1.0]), 0.0), None, "nothing measurable");
    }
}
