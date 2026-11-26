//! The base curve base tone (`profile.base_curve`) and `ToneMap::adjust`. Synthetic curves only.

use crate::SourceInfo;
use crate::finish::{base_curve_problem, base_tone};
use crate::tone::{CameraTone, GREY, LUT_MAX_EV, LUT_MIN_EV, LUT_N, ToneMap};
use lcc_develop::{DevelopSettings, base_curve};
use serde_json::json;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// Tests that install curves hold this: the library is process-wide.
fn lock() -> MutexGuard<'static, ()> {
    static L: Mutex<()> = Mutex::new(());
    L.lock().unwrap_or_else(PoisonError::into_inner)
}

fn install(id: &str, doc: serde_json::Value) {
    let bytes = doc.to_string().into_bytes();
    let c = base_curve::parse(&bytes).expect("a valid synthetic curve");
    base_curve::install_library(vec![(id.to_string(), c, base_curve::hash_bytes(&bytes))]);
}

fn curve_doc(points: serde_json::Value) -> serde_json::Value {
    json!({
        "format": "base-curve", "version": 1,
        "curves": { "a": { "interpolation": "monotone-cubic", "points": points } },
        "apply": [ { "op": "curve", "curve": "a", "on": "luminance" } ]
    })
}

fn default_doc() -> serde_json::Value {
    curve_doc(json!([[0, 0], [0.05, 0.12], [0.2, 0.45], [0.5, 0.8], [0.8, 0.95], [1, 1]]))
}

fn raw_info() -> SourceInfo {
    SourceInfo { raw: true, ..Default::default() }
}

fn settings(id: &str) -> DevelopSettings {
    let mut s = DevelopSettings::default();
    s.profile.base_curve = id.into();
    s
}

fn ev(i: usize) -> f32 {
    LUT_MIN_EV + (LUT_MAX_EV - LUT_MIN_EV) * i as f32 / (LUT_N - 1) as f32
}

fn camera_tone() -> CameraTone {
    let knots = std::array::from_fn(|i| {
        let x = GREY * 2f32.powf(-8.0 + 10.0 * i as f32 / 31.0);
        [x, (x / (x + 0.18)).min(0.99)]
    });
    CameraTone::new(knots).expect("a valid synthetic camera tone")
}

#[test]
fn adjust_is_the_identity_at_zero_and_continuous_there() {
    for y in [0.0f32, 0.001, 0.05, 0.18, 0.5, 0.9, 1.0] {
        assert_eq!(ToneMap::adjust(y, 0.0, 0.0, 0.0).to_bits(), y.to_bits(), "{y}");
    }
    // each slider on its own: the change scales with the slider, no jump at the first step off zero
    for (c, w, b) in [(1.0, 0.0, 0.0), (0.0, 1.0, 0.0), (0.0, 0.0, 1.0), (-1.0, 0.0, 0.0), (0.0, -1.0, 0.0), (0.0, 0.0, -1.0)] {
        for y in [0.001f32, 0.02, 0.18, 0.5, 0.9, 0.999, 1.0] {
            let small = (ToneMap::adjust(y, c * 0.01, w * 0.01, b * 0.01) - y).abs();
            let step = (ToneMap::adjust(y, c, w, b) - y).abs();
            assert!(small <= 1e-3, "no jump at 0 for {y} ({c},{w},{b}): {small}");
            assert!(small <= step * 0.05 + 2e-6, "{y} ({c},{w},{b}): {small} against {step}");
        }
    }
}

#[test]
fn adjust_is_monotone_and_bounded_on_a_slider_grid() {
    let grid = [-100.0, -50.0, 0.0, 50.0, 100.0];
    for c in grid {
        for w in grid {
            for b in grid {
                let mut prev = -1.0f32;
                for i in 0..=400 {
                    let y = i as f32 / 400.0;
                    let o = ToneMap::adjust(y, c, w, b);
                    assert!((0.0..=1.0).contains(&o), "({c},{w},{b}) y {y}: {o}");
                    prev = prev.max(o);
                }
                // non-decreasing once the table builder's running maximum is taken: here, the end points stay in order
                assert!(ToneMap::adjust(1.0, c, 0.0, 0.0) >= ToneMap::adjust(0.0, c, 0.0, 0.0), "contrast {c}");
                assert!(prev <= 1.0);
            }
        }
    }
    // Contrast alone keeps 0 and 1 fixed and has no shoulder: 1.0 stays 1.0 for any Contrast
    for c in [-100.0, -30.0, 30.0, 100.0] {
        assert_eq!(ToneMap::adjust(1.0, c, 0.0, 0.0), 1.0, "{c}");
        assert!(ToneMap::adjust(0.0, c, 0.0, 0.0) < 1e-6, "{c}");
    }
}

#[test]
fn the_existing_base_tones_are_untouched() {
    let _g = lock();
    base_curve::install_library(vec![]);
    // no curve selected: the tone table is the one of the default raw curve (V10 rolls off), exactly as before
    let s = DevelopSettings::default();
    let t = base_tone(&s, &raw_info(), 1.0);
    let expected = ToneMap::rolloff_hdr(0.0, 0.0, 0.0, 1.0);
    assert_eq!(t.lut(), expected.lut());
    // with a camera look: the camera tone
    let info = SourceInfo { raw: true, camera_tone: Some(camera_tone()), ..Default::default() };
    assert_eq!(base_tone(&s, &info, 1.0).lut(), ToneMap::camera_rolloff_hdr(&camera_tone(), 0.0, 0.0, 0.0, 1.0).lut());
    // the display-referred tone of a JPEG
    assert_eq!(base_tone(&s, &SourceInfo::default(), 1.0).lut(), ToneMap::display_hdr(0.0, 0.0, 0.0, 1.0).lut());
    // `adjust` leaves `display_hdr` alone: its shoulder still starts at 0.95 for an edited curve
    let edited = ToneMap::display_hdr(5.0, 0.0, 0.0, 1.0);
    assert!(edited.apply(GREY * 2f32.powf(2.6)) < 1.0);
}

#[test]
fn zero_sliders_render_the_bare_curve_bit_for_bit() {
    let _g = lock();
    install("base:Bare", default_doc());
    let s = settings("base:Bare");
    let t = base_tone(&s, &raw_info(), 1.0);
    let curve = base_curve::get("base:Bare").expect("installed").curve;
    for (i, got) in t.lut().iter().enumerate() {
        let want = curve.eval_luminance(GREY * 2f32.powf(ev(i))).clamp(0.0, 1.0);
        assert_eq!(got.to_bits(), want.to_bits(), "entry {i}");
    }
    base_curve::install_library(vec![]);
}

#[test]
fn the_curve_replaces_the_camera_tone_instead_of_stacking_on_it() {
    let _g = lock();
    install("base:Replace", default_doc());
    let s = settings("base:Replace");
    let plain = base_tone(&s, &raw_info(), 1.0);
    let with_look = base_tone(&s, &SourceInfo { raw: true, camera_tone: Some(camera_tone()), ..Default::default() }, 1.0);
    assert_eq!(plain.lut(), with_look.lut(), "a camera look changes nothing under a base curve");
    assert_ne!(plain.lut(), base_tone(&DevelopSettings::default(), &raw_info(), 1.0).lut());
    // the chroma curve of a camera look goes with it
    assert!(with_look.chroma_lut().iter().all(|c| *c == 1.0));
    base_curve::install_library(vec![]);
}

#[test]
fn a_display_referred_source_ignores_the_curve_and_a_missing_or_unsupported_one_renders_the_default() {
    let _g = lock();
    install("base:Present", default_doc());
    let jpeg = SourceInfo::default();
    assert_eq!(base_tone(&settings("base:Present"), &jpeg, 1.0).lut(), base_tone(&DevelopSettings::default(), &jpeg, 1.0).lut());
    assert!(base_curve_problem(&settings("base:Present"), false).is_none(), "not a raw: nothing to report");
    // missing: the default tone exactly, and a reason
    let missing = settings("base:NotThere");
    assert_eq!(base_tone(&missing, &raw_info(), 1.0).lut(), base_tone(&DevelopSettings::default(), &raw_info(), 1.0).lut());
    let why = base_curve_problem(&missing, true).expect("a reason");
    assert!(why.contains("NotThere") && why.contains("not found"), "{why}");
    // unsupported: loads, is listed, renders the default tone
    let mut doc = default_doc();
    doc["apply"] = json!([{ "op": "curve", "curve": "a", "on": "each-channel" }]);
    install("base:PerChannel", doc);
    let s = settings("base:PerChannel");
    assert_eq!(base_tone(&s, &raw_info(), 1.0).lut(), base_tone(&DevelopSettings::default(), &raw_info(), 1.0).lut());
    assert!(base_curve_problem(&s, true).expect("a reason").contains("per-channel"));
    // in use: no problem
    install("base:Present", default_doc());
    assert!(base_curve_problem(&settings("base:Present"), true).is_none());
    base_curve::install_library(vec![]);
}

#[test]
fn the_white_scale_moves_the_curves_input() {
    let _g = lock();
    install("base:Scale", default_doc());
    let s = settings("base:Scale");
    let one = base_tone(&s, &raw_info(), 1.0);
    let two = base_tone(&s, &SourceInfo { raw: true, white_scale: Some(2.0), ..Default::default() }, 1.0);
    // a source whose colour model carries a gain of 2 reads each scene value twice as large: the table is the same
    // curve at half the input
    for e in [-4.0f32, -2.0, 0.0, 1.0] {
        let v = GREY * 2f32.powf(e);
        assert!((two.apply(v * 2.0) - one.apply(v)).abs() < 2e-4, "{e}");
    }
    // nonsense scales are 1
    for bad in [0.0f32, -1.0, f32::NAN, f32::INFINITY] {
        let t = base_tone(&s, &SourceInfo { raw: true, white_scale: Some(bad), ..Default::default() }, 1.0);
        assert_eq!(t.lut(), one.lut(), "{bad}");
    }
    base_curve::install_library(vec![]);
}

#[test]
fn sdr_is_the_curve_and_hdr_matches_it_below_the_lift_and_reaches_the_peak() {
    let _g = lock();
    install("base:Hdr", default_doc());
    let s = settings("base:Hdr");
    let sdr = base_tone(&s, &raw_info(), 1.0);
    let peak = 4.0f32;
    let hdr = base_tone(&s, &raw_info(), peak);
    for (i, (a, b)) in sdr.lut().iter().zip(hdr.lut()).enumerate() {
        if ev(i) <= 1.5 {
            assert_eq!(a.to_bits(), b.to_bits(), "entry {i} (ev {})", ev(i));
        }
        assert!(b >= a, "HDR never darker than SDR: {i}");
    }
    assert!(hdr.lut().windows(2).all(|w| w[1] >= w[0]), "monotone");
    assert!(hdr.lut().iter().all(|v| *v <= peak));
    assert!((hdr.lut()[LUT_N - 1] - peak).abs() < 1e-4, "reaches the peak at the top of the table");
    assert!(sdr.lut().iter().all(|v| *v <= 1.0));
    // peak 1 with the HDR rolloff rendition is still a table of the same shape: sdr_rolloff folds it into SDR
    let folded = hdr.clone().sdr_rolloff(peak);
    assert!(folded.lut().iter().all(|v| *v <= 1.0 + 1e-6));
    base_curve::install_library(vec![]);
}

#[test]
fn contrast_whites_and_blacks_act_after_the_curve_without_a_second_shoulder() {
    let _g = lock();
    install("base:Order", default_doc());
    let neutral = base_tone(&settings("base:Order"), &raw_info(), 1.0);
    let mut s = settings("base:Order");
    s.light.contrast = 0.5;
    let nudged = base_tone(&s, &raw_info(), 1.0);
    // no jump between Contrast 0 and a tiny Contrast anywhere on the table (the old display shoulder gave 1.0 → 0.9875)
    let worst = neutral.lut().iter().zip(nudged.lut()).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(worst < 5e-3, "{worst}");
    assert!((nudged.lut()[LUT_N - 1] - 1.0).abs() < 1e-6, "white stays white");
    // the order: the curve first, then the adjustment on its output
    s.light.contrast = 40.0;
    let t = base_tone(&s, &raw_info(), 1.0);
    let curve = base_curve::get("base:Order").expect("installed").curve;
    for i in (0..LUT_N).step_by(97) {
        let y = curve.eval_luminance(GREY * 2f32.powf(ev(i))).clamp(0.0, 1.0);
        assert!((t.lut()[i] - ToneMap::adjust(y, 40.0, 0.0, 0.0)).abs() < 1e-6, "entry {i}");
    }
    base_curve::install_library(vec![]);
}

/// Format version 2 file: a natural spline curve, then the encode and decode steps (synthetic numbers).
fn neutral_transfer_doc() -> serde_json::Value {
    json!({
        "format": "base-curve", "version": 2,
        "input": { "gain": 0.9 },
        "curves": { "a": { "interpolation": "natural-cubic-spline", "points": [[0, 0], [0.1, 0.25], [0.5, 0.7], [1, 1]] } },
        "apply": [
            { "op": "curve", "curve": "a", "on": "luminance" },
            { "op": "encode", "gamma": 2.4, "offset": 0.1 },
            { "op": "decode", "gamma": 1.6, "knee": 0.02 }
        ]
    })
}

#[test]
fn the_table_carries_the_whole_neutral_transfer_curve_encode_and_decode() {
    let _g = lock();
    install("base:Transfer", neutral_transfer_doc());
    let t = base_tone(&settings("base:Transfer"), &raw_info(), 1.0);
    let curve = base_curve::get("base:Transfer").expect("installed").curve;
    let spline = &curve.curves["a"];
    // the same transfer worked out here from its definitions
    let (gamma, offset, dgamma, knee): (f64, f64, f64, f64) = (2.4, 0.1, 1.6, 0.02);
    let p = 1.0 / gamma;
    let enc_knee = (offset / ((1.0 + offset) * (1.0 - p))).powf(gamma);
    let by_hand = |v: f64| -> f64 {
        let x = 0.9 * v;
        let c = spline.eval(x);
        let e = if c < enc_knee { (1.0 + offset) * p * enc_knee.powf(p - 1.0) * c } else { (1.0 + offset) * c.powf(p) - offset };
        if e < knee { knee.powf(dgamma - 1.0) * e } else { e.powf(dgamma) }
    };
    for i in (0..LUT_N).step_by(53) {
        let v = f64::from(GREY * 2f32.powf(ev(i)));
        assert!((f64::from(t.lut()[i]) - by_hand(v).clamp(0.0, 1.0)).abs() < 2e-6, "entry {i} (ev {}): {} against {}", ev(i), t.lut()[i], by_hand(v));
    }
    assert!(t.lut().windows(2).all(|w| w[1] >= w[0]), "monotone");
    assert!(t.lut()[0] < 1e-4 && (t.lut()[LUT_N - 1] - 1.0).abs() < 1e-6);
    // Contrast, Whites and Blacks still come after the whole transfer, and 0 leaves it exactly alone
    let mut s = settings("base:Transfer");
    s.light.contrast = 30.0;
    let c = base_tone(&s, &raw_info(), 1.0);
    for i in (0..LUT_N).step_by(131) {
        assert!((c.lut()[i] - ToneMap::adjust(t.lut()[i], 30.0, 0.0, 0.0)).abs() < 1e-6, "entry {i}");
    }
    base_curve::install_library(vec![]);
}

#[test]
fn scene_values_at_or_above_the_curves_white_render_white() {
    let _g = lock();
    install("base:Clip", neutral_transfer_doc());
    let t = base_tone(&settings("base:Clip"), &raw_info(), 1.0);
    // the file's gain is 0.9: a scene value of 1/0.9 (and everything above it) reaches x = 1, where the clamped curve
    // holds 1 and the encode and decode keep it: exactly white. A fully clipped highlight, which the highlight
    // reconstruction writes at the brightest white-balanced channel's clip (for a red multiplier of 1.8 that is about
    // 1.78 of the sensor's white, 1.6 in x), is above it.
    for v in [1.2f32, 1.78, 3.0, 20.0] {
        assert!((t.apply(v) - 1.0).abs() < 1e-6, "scene value {v}: {}", t.apply(v));
    }
    assert!(t.apply(0.9) < 1.0, "just below x = 1 the output has not reached white");
    // a source whose white-balance multipliers are all near 1 puts its clipped highlights at x of about 0.9: the table
    // does not reach white there, which is the curve's own value at that x
    assert!(t.apply(0.99 * 1.0) < 1.0);
    base_curve::install_library(vec![]);
}

/// A synthetic colour model: the matrix has unit row sums, the multipliers are the as-shot ones (red and blue above the
/// green, which is not the smallest here: the case the old, as-shot scale got wrong). `green_probe` is the green row of
/// the inverse of matrix · multipliers.
fn film_info(wb: [f64; 3]) -> (SourceInfo, lcc_color::Mat3) {
    let m = lcc_color::Mat3([[0.8, 0.3, -0.1], [-0.1, 1.2, -0.1], [0.0, -0.2, 1.2]]);
    let dev = m.mul(&lcc_color::Mat3::diag(wb[0], wb[1], wb[2]));
    let p = dev.inverse().unwrap().0[1].map(|v| v as f32);
    (SourceInfo { raw: true, as_shot_temp: 5200.0, as_shot_tint: 4.0, white_scale: Some(1.0), green_probe: Some(p), ..Default::default() }, dev)
}

#[test]
fn a_neutral_of_the_same_raw_green_meets_the_curve_at_the_same_input_under_any_white_balance() {
    use crate::local::{film_white_scale, wb_matrix_for};
    let (info, dev) = film_info([1.9, 1.35, 1.0]);
    let lum = |c: [f64; 3]| 0.2627 * c[0] + 0.6780 * c[1] + 0.0593 * c[2];
    let mut cases = vec![];
    for (t, tint) in [(5200.0, 4.0), (3200.0, 0.0), (8474.0, 8.0), (6500.0, -20.0), (10000.0, 30.0)] {
        let mut s = DevelopSettings::default();
        s.wb.mode = lcc_develop::WbMode::Custom;
        s.wb.temp = t;
        s.wb.tint = tint;
        // the raw neutral of this white balance: what the white-balance change turns into grey (c, c, c)
        let m = wb_matrix_for(&info, &s).map_or(lcc_color::Mat3::IDENTITY, |m| lcc_color::Mat3(m.map(|r| r.map(f64::from))));
        let pixel = m.inverse().unwrap().apply([1.0; 3]);
        let raw = dev.inverse().unwrap().apply(pixel);
        // the developed value the tone stage sees, over the scale the curve divides it by
        let x = lum(m.apply(pixel)) / f64::from(film_white_scale(&info, &s));
        cases.push((t, x / raw[1]));
    }
    for (t, r) in &cases {
        assert!((r - 1.0).abs() < 1e-4, "{t} K: x / raw green = {r}");
    }
}

#[test]
fn a_source_without_the_probe_keeps_the_as_shot_scale_and_the_default_render_is_untouched() {
    use crate::local::film_white_scale;
    let mut s = DevelopSettings::default();
    s.wb.mode = lcc_develop::WbMode::Custom;
    s.wb.temp = 3300.0;
    let known = SourceInfo { raw: true, white_scale: Some(1.7), ..Default::default() };
    assert_eq!(film_white_scale(&known, &s), 1.7);
    assert_eq!(film_white_scale(&SourceInfo::default(), &s), 1.0);
    // a probe that gives no positive green (nonsense) falls back too
    let bad = SourceInfo { green_probe: Some([0.0; 3]), ..known.clone() };
    assert_eq!(film_white_scale(&bad, &s), 1.7);
    // without a base curve nothing reads the probe: the table is the camera/generic one whatever the probe says
    let _g = lock();
    let plain = base_tone(&DevelopSettings::default(), &raw_info(), 1.0);
    let probed = base_tone(&DevelopSettings::default(), &SourceInfo { green_probe: Some([0.1, 0.9, 0.2]), ..raw_info() }, 1.0);
    assert_eq!(plain.lut(), probed.lut());
}
