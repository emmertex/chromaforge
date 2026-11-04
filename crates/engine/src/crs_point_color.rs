//! Interchange: Lightroom's Point Color samples (`crs:PointColors` globally, `crs:LocalPointColors` in a
//! mask correction) ↔ our [`PointColor`].
//!
//! The format, as Lightroom Classic writes it in presets (read from presets made with it; no public
//! specification): each `rdf:li` is one sample of 19 comma-separated numbers,
//!
//! | # | meaning |
//! |---|---|
//! | 0–2 | the sampled colour: hue (radians, 0..2π), saturation 0..1, luminance 0..1 (linear) |
//! | 3–5 | hue, saturation and luminance shift, −1..1 |
//! | 6 | range 0..1 (0.5 by default) |
//! | 7–10 | hue range: a trapezoid (fade start, full start, full end, fade end) on 0..1 across a window centred on the sample, (0, ⅓, ⅔, 1) by default |
//! | 11–14 | saturation range, a trapezoid on the saturation scale (sample ± 0.18, fading over 0.55, by default) |
//! | 15–18 | luminance range, a trapezoid on the luminance scale encoded with gamma 2.2 (the sample's luminance ^ 1/2.2 ± 0.18 by default) |
//!
//! An unused slot is nineteen −1. Variance is stored apart, one number (−100..100) per sample:
//! `crs:ColorVariance` / `crs:LocalColorVariance`.
//!
//! Best effort, since the two colour models differ: the sampled colour is read as a hue / saturation in
//! sRGB (the way a dry-grass sample reads yellow, not orange) at the stated luminance, and converted to the
//! OkLCh sample our Point Color keeps; the trapezoids' full-strength widths, relative to Lightroom's defaults,
//! set our range sliders (default ↔ default), and their centres move the sample (our ranges are centred on
//! it). Writing does the reverse, with symmetric trapezoids, so a sample we write reads back as it
//! was.

use lcc_color::perceptual::{lab_to_lch, lch_to_lab, oklab_from_2020, oklab_to_2020};
use lcc_color::transfer::{linear_to_srgb, srgb_to_linear};
use lcc_color::{REC2020, SRGB};
use lcc_develop::{MAX_POINT_COLORS, PointColor};

/// Numbers per sample.
const WORDS: usize = 19;
/// Full-strength width of Lightroom's default hue range, relative to its window.
const HUE_INNER: f64 = 1.0 / 3.0;
/// Full-strength width of Lightroom's default saturation and luminance ranges.
const INNER: f64 = 0.36;
/// How far Lightroom's default saturation and luminance ranges fade out on each side.
const FADE: f64 = 0.55;
/// Our range box's outer half-widths at the default slider (50): hue (radians), chroma, lightness.
const OUR_HUE: f64 = 0.4;
const OUR_CHROMA: f64 = 0.08;
const OUR_LUM: f64 = 0.25;
/// Lightroom's hue window (0..1) in radians: our hue box at the default slider.
const HUE_WINDOW: f64 = 2.0 * OUR_HUE;

/// One Lightroom sample, as stored.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LrPoint {
    pub hue: f64,
    pub sat: f64,
    pub lum: f64,
    pub shift: [f64; 3],
    pub range: f64,
    pub hue_range: [f64; 4],
    pub sat_range: [f64; 4],
    pub lum_range: [f64; 4],
}

/// A stored sample line; `None` for an unused slot (all −1) or anything that isn't 19 finite numbers.
pub fn parse(line: &str) -> Option<LrPoint> {
    let v: Vec<f64> = line.split(',').map(|t| t.trim().parse::<f64>().ok().filter(|x| x.is_finite())).collect::<Option<Vec<_>>>()?;
    if v.len() != WORDS || v.iter().all(|x| *x == -1.0) {
        return None;
    }
    let q = |i: usize| -> [f64; 4] { std::array::from_fn(|k| v.get(i + k).copied().unwrap_or(0.0)) };
    Some(LrPoint {
        hue: v.first().copied()?,
        sat: v.get(1).copied()?,
        lum: v.get(2).copied()?,
        shift: [v.get(3).copied()?, v.get(4).copied()?, v.get(5).copied()?],
        range: v.get(6).copied()?,
        hue_range: q(7),
        sat_range: q(11),
        lum_range: q(15),
    })
}

/// The line for a sample (19 numbers, six decimals).
pub fn format(p: &LrPoint) -> String {
    let mut v = vec![p.hue, p.sat, p.lum, p.shift[0], p.shift[1], p.shift[2], p.range];
    v.extend(p.hue_range);
    v.extend(p.sat_range);
    v.extend(p.lum_range);
    v.iter().map(|x| format!("{:.6}", if x.is_finite() { *x } else { 0.0 })).collect::<Vec<_>>().join(", ")
}

/// A trapezoid's full-strength part (start, end), when it is one (finite, ordered, on 0..1).
fn full(q: [f64; 4]) -> Option<(f64, f64)> {
    let ok = q.iter().all(|x| x.is_finite() && (-1e-6..=1.0 + 1e-6).contains(x)) && q[0] <= q[1] + 1e-6 && q[1] <= q[2] + 1e-6 && q[2] <= q[3] + 1e-6;
    ok.then(|| (q[1].clamp(0.0, 1.0), q[2].clamp(0.0, 1.0)))
}

/// Our range slider (0..100) for a full-strength width `ratio` × Lightroom's default; `base` and `per` are
/// our box's width at slider 0 and its growth to slider 100 (`PointK::new`).
fn slider(ratio: f64, default: f64, base: f64, per: f64) -> f64 {
    (((default * ratio - base) / per) * 100.0).clamp(0.0, 100.0)
}

/// The width ratio (× Lightroom's default) of our range slider `v`.
fn ratio(v: f64, default: f64, base: f64, per: f64) -> f64 {
    (base + per * (v.clamp(0.0, 100.0) / 100.0)) / default
}

/// sRGB hue (radians) / saturation (HSV, 0..1) at linear luminance `y` → OkLCh (lightness, chroma, hue°).
fn hsv_to_oklch(hue: f64, sat: f64, y: f64) -> (f64, f64, f64) {
    let h = (hue.to_degrees().rem_euclid(360.0)) / 60.0;
    let s = sat.clamp(0.0, 1.0);
    let x = s * (1.0 - ((h % 2.0) - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (s, x, 0.0),
        1 => (x, s, 0.0),
        2 => (0.0, s, x),
        3 => (0.0, x, s),
        4 => (x, 0.0, s),
        _ => (s, 0.0, x),
    };
    // value 1: max channel 1, the others 1 − chroma
    let e = [r + 1.0 - s, g + 1.0 - s, b + 1.0 - s].map(|v| v.clamp(0.0, 1.0) as f32);
    let lin = e.map(srgb_to_linear);
    let yl = 0.2126 * lin[0] + 0.7152 * lin[1] + 0.0722 * lin[2];
    let k = if yl > 1e-9 { (y.clamp(0.0, 1.0) as f32) / yl } else { 0.0 };
    let rec = SRGB.to_space(&REC2020).apply_f32(lin.map(|v| v * k));
    let [l, c, h] = lab_to_lch(oklab_from_2020(rec));
    (l as f64, c as f64, (h as f64).to_degrees().rem_euclid(360.0))
}

/// [`hsv_to_oklch`] reversed: OkLCh → (sRGB hue radians, HSV saturation, linear luminance).
fn oklch_to_hsv(l: f64, c: f64, hue_deg: f64) -> (f64, f64, f64) {
    let rec = oklab_to_2020(lch_to_lab([l as f32, c.max(0.0) as f32, (hue_deg as f32).to_radians()]));
    let lin = REC2020.to_space(&SRGB).apply_f32(rec).map(|v| v.max(0.0));
    let y = (0.2126 * lin[0] + 0.7152 * lin[1] + 0.0722 * lin[2]) as f64;
    let mx = lin[0].max(lin[1]).max(lin[2]);
    if mx <= 1e-9 {
        return (0.0, 0.0, 0.0);
    }
    let e = lin.map(|v| linear_to_srgb((v / mx).clamp(0.0, 1.0)) as f64);
    let (hi, lo) = (e[0].max(e[1]).max(e[2]), e[0].min(e[1]).min(e[2]));
    let d = hi - lo;
    let s = if hi > 0.0 { d / hi } else { 0.0 };
    let h = if d < 1e-9 {
        0.0
    } else if hi == e[0] {
        ((e[1] - e[2]) / d).rem_euclid(6.0)
    } else if hi == e[1] {
        (e[2] - e[0]) / d + 2.0
    } else {
        (e[0] - e[1]) / d + 4.0
    };
    ((h * 60.0).to_radians(), s.clamp(0.0, 1.0), y.clamp(0.0, 1.0))
}

/// A Lightroom sample with its `variance` (−100..100) → ours.
pub fn to_ours(p: &LrPoint, variance: f64) -> PointColor {
    let pct = |v: f64| (v * 100.0).clamp(-100.0, 100.0);
    // the hue window's centre moves the sample's hue; the full-strength width sets the hue range
    let (hue_off, hue_range) = match full(p.hue_range) {
        Some((b, c)) => (((b + c) / 2.0 - 0.5) * HUE_WINDOW, slider((c - b) / HUE_INNER, OUR_HUE, 0.1, 0.6)),
        None => (0.0, 50.0),
    };
    let (sat, sat_range) = match full(p.sat_range) {
        Some((b, c)) => ((b + c) / 2.0, slider((c - b) / INNER, OUR_CHROMA, 0.02, 0.12)),
        None => (p.sat, 50.0),
    };
    let (y, lum_range) = match full(p.lum_range) {
        Some((b, c)) => (((b + c) / 2.0).powf(2.2), slider((c - b) / INNER, OUR_LUM, 0.05, 0.4)),
        None => (p.lum, 50.0),
    };
    let (l, c, h) = hsv_to_oklch(p.hue + hue_off, sat, y);
    PointColor {
        lum: l.clamp(0.0, 1.0),
        chroma: c.clamp(0.0, 0.5),
        hue: h,
        hue_shift: pct(p.shift[0]),
        sat_shift: pct(p.shift[1]),
        lum_shift: pct(p.shift[2]),
        variance: if variance.is_finite() { variance.clamp(-100.0, 100.0) } else { 0.0 },
        range: (p.range * 100.0).clamp(0.0, 100.0),
        hue_range,
        sat_range,
        lum_range,
    }
}

/// Our sample → Lightroom's, and its variance.
pub fn from_ours(p: &PointColor) -> (LrPoint, f64) {
    let (hue, sat, y) = oklch_to_hsv(p.lum, p.chroma, p.hue);
    let around = |centre: f64, inner: f64, fade: f64| -> [f64; 4] {
        let (b, c) = (centre - inner / 2.0, centre + inner / 2.0);
        [b - fade, b, c, c + fade].map(|v| v.clamp(0.0, 1.0))
    };
    let hue_inner = HUE_INNER * ratio(p.hue_range, OUR_HUE, 0.1, 0.6);
    let lr = LrPoint {
        hue: hue.rem_euclid(std::f64::consts::TAU),
        sat,
        lum: y,
        shift: [p.hue_shift, p.sat_shift, p.lum_shift].map(|v| (v / 100.0).clamp(-1.0, 1.0)),
        range: (p.range / 100.0).clamp(0.0, 1.0),
        hue_range: around(0.5, hue_inner, hue_inner),
        sat_range: around(sat, INNER * ratio(p.sat_range, OUR_CHROMA, 0.02, 0.12), FADE),
        lum_range: around(y.powf(1.0 / 2.2), INNER * ratio(p.lum_range, OUR_LUM, 0.05, 0.4), FADE),
    };
    (lr, if p.variance.is_finite() { p.variance.clamp(-100.0, 100.0) } else { 0.0 })
}

/// The samples in `lines` (one per sample) with their `variances` (by position; missing ones are 0), at most
/// [`MAX_POINT_COLORS`]; unused slots and unreadable lines are skipped.
pub fn read<'a>(lines: impl IntoIterator<Item = &'a str>, variances: &[f64]) -> Vec<PointColor> {
    lines
        .into_iter()
        .enumerate()
        .filter_map(|(i, l)| parse(l).map(|p| to_ours(&p, variances.get(i).copied().unwrap_or(0.0))))
        .take(MAX_POINT_COLORS)
        .collect()
}

/// A variance list's numbers (unparsable entries are 0).
pub fn variances<'a>(items: impl IntoIterator<Item = &'a str>) -> Vec<f64> {
    items.into_iter().map(|s| s.trim().parse::<f64>().ok().filter(|x| x.is_finite()).unwrap_or(0.0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sample as Lightroom Classic writes it (the defaults it gives a picked colour: range 0.5,
    /// hue (0, ⅓, ⅔, 1), saturation and luminance ± 0.18 fading over 0.55).
    const PICKED: &str = "0.558493, 0.255117, 0.846338, 0.000000, 0.836360, 0.000000, 0.500000, 0.000000, 0.333333, 0.666667, 1.000000, 0.000000, 0.075117, 0.435117, 0.985117, 0.380000, 0.930000, 1.000000, 1.000000";

    #[test]
    fn a_lightroom_sample_reads_as_a_warm_colour_with_default_ranges() {
        let p = parse(PICKED).expect("19 numbers");
        let ours = to_ours(&p, -20.0);
        assert!((ours.sat_shift - 83.636).abs() < 1e-3 && ours.hue_shift == 0.0 && ours.variance == -20.0);
        assert!((ours.range - 50.0).abs() < 1e-9);
        for r in [ours.hue_range, ours.sat_range] {
            assert!((r - 50.0).abs() < 0.5, "default widths are our defaults: {ours:?}");
        }
        // 32° in sRGB hue: an orange, in OkLCh around 60°
        assert!((40.0..80.0).contains(&ours.hue), "{ours:?}");
        assert!(ours.chroma > 0.02 && ours.chroma < 0.12 && ours.lum > 0.8, "{ours:?}");
    }

    #[test]
    fn our_samples_write_and_read_back() {
        let samples = [
            PointColor {
                lum: 0.7,
                chroma: 0.08,
                hue: 75.0,
                hue_shift: 30.0,
                sat_shift: -40.0,
                lum_shift: 10.0,
                variance: -35.0,
                range: 60.0,
                ..Default::default()
            },
            PointColor {
                lum: 0.55,
                chroma: 0.12,
                hue: 250.0,
                sat_shift: -100.0,
                hue_range: 80.0,
                sat_range: 30.0,
                lum_range: 70.0,
                ..Default::default()
            },
        ];
        for p in samples {
            let (lr, var) = from_ours(&p);
            let line = format(&lr);
            let back = to_ours(&parse(&line).expect("readable"), var);
            for (a, b, tol, what) in [
                (back.lum, p.lum, 2e-3, "lum"),
                (back.chroma, p.chroma, 2e-3, "chroma"),
                (back.hue, p.hue, 0.5, "hue"),
                (back.hue_shift, p.hue_shift, 1e-3, "hue shift"),
                (back.sat_shift, p.sat_shift, 1e-3, "sat shift"),
                (back.lum_shift, p.lum_shift, 1e-3, "lum shift"),
                (back.variance, p.variance, 1e-3, "variance"),
                (back.range, p.range, 1e-3, "range"),
                (back.hue_range, p.hue_range, 0.01, "hue range"),
                (back.sat_range, p.sat_range, 0.01, "sat range"),
                (back.lum_range, p.lum_range, 0.01, "lum range"),
            ] {
                assert!((a - b).abs() < tol, "{what}: {a} vs {b} ({line})");
            }
        }
    }

    #[test]
    fn unused_slots_and_hostile_lines_are_skipped() {
        let empty = vec!["-1.000000"; 19].join(", ");
        assert!(parse(&empty).is_none());
        assert!(parse("1, 2, 3").is_none(), "too short");
        assert!(parse(&PICKED.replace("0.255117", "NaN")).is_none());
        assert!(parse(&format!("{PICKED}, 4")).is_none(), "too long");
        // out-of-range numbers are clamped, never panic
        let wild = "1e30, -5, 99, 7, -7, 3, 9, -1, 5, 0.2, 0.1, 2, 0.4, 0.3, 0, 0, 0, 0, 0";
        let p = to_ours(&parse(wild).expect("19 finite numbers"), f64::NAN);
        assert!(p.lum.is_finite() && p.chroma.is_finite() && p.hue.is_finite() && p.variance == 0.0);
        assert!([p.hue_shift, p.sat_shift, p.lum_shift].iter().all(|v| (-100.0..=100.0).contains(v)));
        // at most eight samples
        let many: Vec<&str> = vec![PICKED; 12];
        assert_eq!(read(many, &[]).len(), MAX_POINT_COLORS);
    }
}
