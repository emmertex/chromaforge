//! The post-tone colour map of a base curve (`crate::colour_map`). Synthetic maps only.

use crate::colour_map::{ColourMapK, FADE_FROM};
use lcc_color::perceptual::{oklab_from_2020, oklab_to_2020};
use lcc_develop::base_curve::{self, ColourMap};

const HUES: usize = 12;
const LIGHTNESS: [f64; 6] = [0.0, 0.3, 0.5, 0.65, 0.8, 1.0];
const CHROMA: [f64; 3] = [0.05, 0.12, 0.22];

/// A synthetic map with structure in every direction: the gain between 0.8 and 1.25, the shift within 8 degrees.
fn wavy() -> ColourMap {
    let (mut gain, mut shift) = (vec![], vec![]);
    for c in 0..CHROMA.len() {
        for l in 0..LIGHTNESS.len() {
            for h in 0..HUES {
                let (hf, lf, cf) = (h as f64, l as f64, c as f64);
                gain.push(1.0 + 0.12 * (hf * 0.9 + lf).sin() * (0.5 + 0.5 * cf) - 0.08 * (lf * 1.3 - cf).cos() * 0.5);
                shift.push(8.0 * (hf * 0.5 - lf * 0.7 + cf).cos());
            }
        }
    }
    ColourMap { hues: HUES, lightness: LIGHTNESS.to_vec(), chroma: CHROMA.to_vec(), gain, shift }
}

fn constant(gain: f64, shift: f64) -> ColourMap {
    let n = HUES * LIGHTNESS.len() * CHROMA.len();
    ColourMap { hues: HUES, lightness: LIGHTNESS.to_vec(), chroma: CHROMA.to_vec(), gain: vec![gain; n], shift: vec![shift; n] }
}

/// A Rec.2020 colour of the given OkLab lightness, chroma and hue (degrees).
fn colour(l: f32, c: f32, h: f32) -> [f32; 3] {
    let (s, k) = h.to_radians().sin_cos();
    oklab_to_2020([l, c * k, c * s])
}

fn lch(d: [f32; 3]) -> (f32, f32, f32) {
    let lab = oklab_from_2020(d);
    (lab[0], lab[1].hypot(lab[2]), lab[2].atan2(lab[1]).to_degrees().rem_euclid(360.0))
}

#[test]
fn a_map_that_changes_nothing_is_no_map_and_greys_pass_untouched() {
    assert!(ColourMapK::new(&constant(1.0, 0.0)).is_none());
    let m = ColourMapK::new(&wavy()).expect("a map that changes something");
    // greys and tints below the fade pass bit for bit, even though the OkLab round trip would move them
    let mut tested = 0;
    for i in 0..80 {
        let k = 1e-4 * 1.25f32.powi(i);
        for d in [[k; 3], [k, k * 1.003, k], [k * 1.002, k, k * 0.999]] {
            let c = lch(d).1;
            if c < FADE_FROM {
                assert_eq!(m.apply(d).map(f32::to_bits), d.map(f32::to_bits), "{d:?} (chroma {c})");
                tested += 1;
            }
        }
    }
    assert!(tested > 150, "{tested}");
    // a file's identity map is dropped when the file is read, so the curve has none
    let doc = serde_json::json!({
        "format": "base-curve", "version": 2,
        "curves": { "a": { "interpolation": "linear", "points": [[0, 0], [1, 1]] } },
        "apply": [ { "op": "curve", "curve": "a", "on": "luminance" } ],
        "colour_map": { "hues": 4, "lightness": [0.0, 1.0], "chroma": [0.1], "gain": vec![1.0; 8], "hue_shift": vec![0.0; 8] }
    });
    assert!(base_curve::parse(doc.to_string().as_bytes()).unwrap().colour_map.is_none());
}

#[test]
fn a_constant_map_scales_the_chroma_and_turns_the_hue_and_keeps_the_lightness() {
    let up = ColourMapK::new(&constant(1.2, 0.0)).unwrap();
    let turn = ColourMapK::new(&constant(1.0, 10.0)).unwrap();
    for (l, c, h) in [(0.4f32, 0.12f32, 40.0f32), (0.7, 0.2, 140.0), (0.55, 0.08, 250.0), (0.9, 0.1, 100.0)] {
        let d = colour(l, c, h);
        let (l1, c1, h1) = lch(up.apply(d));
        assert!((l1 - l).abs() < 2e-4 && (c1 / c - 1.2).abs() < 2e-3 && (h1 - h).abs() < 0.05, "{l} {c} {h}: {l1} {c1} {h1}");
        let (l2, c2, h2) = lch(turn.apply(d));
        assert!(
            (l2 - l).abs() < 2e-4 && (c2 / c - 1.0).abs() < 2e-3 && ((h2 - h - 10.0 + 540.0).rem_euclid(360.0) - 180.0).abs() < 0.05,
            "{l} {c} {h}: {l2} {c2} {h2}"
        );
    }
    // a faint colour is moved only by the fade: part of the way up at chroma 0.0145
    let faint = colour(0.5, 0.0145, 60.0);
    let (_, c, _) = lch(up.apply(faint));
    assert!(c > 0.0145 && c < 0.0145 * 1.2, "{c}");
}

#[test]
fn the_map_is_continuous_in_hue_lightness_chroma_and_across_the_fade() {
    let m = ColourMapK::new(&wavy()).unwrap();
    let dist = |a: [f32; 3], b: [f32; 3]| (0..3).map(|i| (a[i] - b[i]).abs()).fold(0.0, f32::max);
    // a small step of hue, lightness or chroma moves the output by a bounded amount (the map never magnifies by more than a few times)
    for (l, c) in [(0.3f32, 0.04f32), (0.5, 0.1), (0.65, 0.15), (0.8, 0.2), (0.45, 0.012), (0.6, 0.03)] {
        let mut prev = m.apply(colour(l, c, 0.0));
        let mut prev_in = colour(l, c, 0.0);
        for i in 1..=1440 {
            let d = colour(l, c, i as f32 * 0.25);
            let o = m.apply(d);
            assert!(dist(o, prev) <= 4.0 * dist(d, prev_in) + 5e-4, "hue {} at {l} {c}: {}", i as f32 * 0.25, dist(o, prev));
            prev = o;
            prev_in = d;
        }
    }
    for h in [10.0f32, 95.0, 140.0, 200.0, 310.0] {
        let (mut prev, mut prev_in) = (m.apply(colour(0.02, 0.1, h)), colour(0.02, 0.1, h));
        for i in 1..=700 {
            let d = colour(0.02 + i as f32 * 0.0015, 0.1, h);
            let o = m.apply(d);
            assert!(dist(o, prev) <= 4.0 * dist(d, prev_in) + 5e-4, "lightness at hue {h}");
            prev = o;
            prev_in = d;
        }
        let (mut prev, mut prev_in) = (m.apply(colour(0.55, 0.002, h)), colour(0.55, 0.002, h));
        for i in 1..=500 {
            let d = colour(0.55, 0.002 + i as f32 * 0.0006, h);
            let o = m.apply(d);
            assert!(dist(o, prev) <= 4.0 * dist(d, prev_in) + 5e-4, "chroma {} at hue {h}", 0.002 + i as f32 * 0.0006);
            prev = o;
            prev_in = d;
        }
    }
}

#[test]
fn saturated_out_of_gamut_and_extreme_colours_stay_finite() {
    let m = ColourMapK::new(&wavy()).unwrap();
    let mut cases: Vec<[f32; 3]> = vec![];
    for base in [
        [0.0f32, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 1.0, 1.0],
        [1.0, 1.0, 0.0],
        [0.9, 0.02, 0.0],
        [-0.1, 0.8, 0.1],
        [0.5, -0.2, 0.3],
        [-0.3, -0.2, 0.9],
    ] {
        for k in [0.0f32, 1e-6, 1e-3, 0.05, 0.5, 1.0, 4.0, 100.0, 5000.0] {
            cases.push(base.map(|b| b * k));
        }
    }
    cases.push([0.0; 3]);
    for d in cases {
        let o = m.apply(d);
        assert!(o.iter().all(|v| v.is_finite()), "{d:?} -> {o:?}");
        // the colour stays in the same neighbourhood: a gain of at most 1.25 and a shift of at most 8 degrees cannot move it far
        let scale = d.iter().map(|v| v.abs()).fold(1e-3f32, f32::max);
        assert!(o.iter().zip(&d).all(|(a, b)| (a - b).abs() <= 1.5 * scale), "{d:?} -> {o:?}");
    }
}

#[test]
fn a_curve_file_carries_its_map_into_the_tone_map() {
    use crate::SourceInfo;
    use crate::finish::base_tone;
    use lcc_develop::DevelopSettings;
    let doc = |map: Option<serde_json::Value>| {
        let mut d = serde_json::json!({
            "format": "base-curve", "version": 2,
            "curves": { "a": { "interpolation": "linear", "points": [[0, 0], [0.5, 0.8], [1, 1]] } },
            "apply": [ { "op": "curve", "curve": "a", "on": "luminance" } ]
        });
        if let Some(m) = map {
            d["colour_map"] = m;
        }
        d
    };
    let map = serde_json::json!({ "hues": 4, "lightness": [0.0, 1.0], "chroma": [0.1], "gain": vec![1.1; 8], "hue_shift": vec![2.0; 8] });
    let (a, b) = (doc(None).to_string().into_bytes(), doc(Some(map)).to_string().into_bytes());
    let lib = vec![
        ("base:Plain".to_string(), base_curve::parse(&a).unwrap(), base_curve::hash_bytes(&a)),
        ("base:Mapped".to_string(), base_curve::parse(&b).unwrap(), base_curve::hash_bytes(&b)),
    ];
    let _g = crate::tests_base_curve::lock();
    base_curve::install_library(lib);
    let tone = |id: &str| {
        let mut s = DevelopSettings::default();
        s.profile.base_curve = id.into();
        base_tone(&s, &SourceInfo { raw: true, ..Default::default() }, 1.0)
    };
    let (plain, mapped) = (tone("base:Plain"), tone("base:Mapped"));
    assert!(plain.colour_map().is_none() && mapped.colour_map().is_some());
    assert_eq!(plain.lut(), mapped.lut(), "the map does not touch the tone table");
    base_curve::install_library(vec![]);
}
