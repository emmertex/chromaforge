//! The recolor maths (ported from upstream's `recolor/tests.rs`, adapted).

use super::measure::main_colour;
use super::*;
use lcc_color::perceptual::{lab_to_lch, lch_to_lab, oklab_from_2020};

fn hue_dist(a: f32, b: f32) -> f32 {
    wrap(a - b).abs()
}

fn reference_of(pixels: &[[f32; 3]]) -> Reference {
    let samples: Vec<([f32; 3], f32)> = pixels.iter().map(|p| (*p, 1.0)).collect();
    main_colour(&samples, None).unwrap_or(Reference { l: 0.5, c: 0.0, h: [1.0, 0.0], spread: 0.0 })
}

fn chroma(p: [f32; 3]) -> f32 {
    p[1].hypot(p[2])
}

/// OkLab ΔE between two colours.
fn delta_e(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn red_hue() -> f32 {
    lab_to_lch(oklab_from_2020([0.6, 0.05, 0.04]))[2]
}

fn red_gradient() -> Vec<[f32; 3]> {
    (0..21).map(|i| lch_to_lab([0.3 + 0.02 * i as f32, 0.15 * (0.6 + 0.02 * i as f32), red_hue()])).collect()
}

/// A red gradient (lightness 0.3 → 0.7, as shading on a red boot) recolored to a purple as light as
/// its main colour keeps every pixel's lightness and takes the purple's hue.
#[test]
fn a_red_gradient_recolored_to_purple_keeps_its_lightness_profile() {
    let grad = red_gradient();
    let r = reference_of(&grad);
    let w = Window::new(&r, 0.0);
    let purple = lab_to_lch(srgb_to_oklab([0.48, 0.25, 0.69]));
    let target = lch_to_lab([r.l, purple[1], purple[2]]);
    for p in &grad {
        let o = apply(*p, target, &r, &w, 1.0);
        let (lo, li) = (lab_to_lch(o), lab_to_lch(*p));
        assert!((lo[0] - li[0]).abs() < 1e-4, "lightness {} → {}", li[0], lo[0]);
        assert!(hue_dist(lo[2], purple[2]) < 0.05, "hue {} vs purple {}", lo[2], purple[2]);
        assert!(lo[1] > 0.05, "chroma {}", lo[1]);
    }
    let lighter = lch_to_lab([r.l + 0.1, purple[1], purple[2]]);
    let ls: Vec<f32> = grad.iter().map(|p| apply(*p, lighter, &r, &w, 1.0)[0]).collect();
    assert!(ls.windows(2).all(|w| w[1] > w[0]));
}

/// A black garment with folds recolored to white keeps the folds' contrast in proportion.
#[test]
fn a_black_garment_recolored_to_white_keeps_its_fold_contrast_in_proportion() {
    let folds = [0.08f32, 0.12, 0.16, 0.2, 0.24, 0.2, 0.16];
    let px: Vec<[f32; 3]> = folds.iter().map(|l| [*l, 0.002, -0.003]).collect();
    let r = reference_of(&px);
    let w = Window::new(&r, 0.0);
    let white = srgb_to_oklab([0.95, 0.95, 0.95]);
    let out: Vec<[f32; 3]> = px.iter().map(|p| apply(*p, white, &r, &w, 1.0)).collect();
    for (p, o) in px.iter().zip(&out) {
        if p[0] <= r.l {
            assert!((o[0] / white[0] - p[0] / r.l).abs() < 1e-4, "{} → {}", p[0], o[0]);
        }
        assert!(o[0] <= 1.0 + 1e-6 && o[0] > p[0]);
        assert!(chroma(*o) < 0.01, "chroma {o:?}");
    }
    assert!(out[4][0] / out[0][0] > 1.5);
}

/// Under one mask holding a red gradient (the most of it), a blue star, grey dust and an off-white
/// lining, only the red changes; the others stay within a small ΔE.
#[test]
fn only_the_main_colour_changes_inside_a_mixed_mask() {
    let mut px = red_gradient();
    px.extend(red_gradient());
    let star = [0.45, -0.03, -0.14];
    let dust = [0.35, 0.008, 0.012];
    let lining = [0.9, 0.006, 0.014];
    let rubber = [0.12, 0.01, 0.004];
    px.extend([star, star, dust, dust, dust, lining, lining, rubber]);
    let r = reference_of(&px);
    assert!(hue_dist(hue(r.h[0], r.h[1]), red_hue()) < 0.1, "the main colour is the red: {r:?}");
    let w = Window::new(&r, 0.0);
    let purple = srgb_to_oklab([0.48, 0.25, 0.69]);
    for other in [star, dust, lining, rubber] {
        let o = apply(other, purple, &r, &w, 1.0);
        assert!(delta_e(o, other) < 0.01, "{other:?} → {o:?}");
    }
    let red = apply(red_gradient()[10], purple, &r, &w, 1.0);
    assert!(hue_dist(lab_to_lch(red)[2], lab_to_lch(purple)[2]) < 0.1, "the red turns purple: {red:?}");
    // Range: an orange next to the red stays at 0 and changes when widened; narrowed, it stays too
    let orange = lch_to_lab([0.6, 0.14, red_hue() + 0.7]);
    assert!(delta_e(apply(orange, purple, &r, &w, 1.0), orange) < 0.01);
    assert!(delta_e(apply(orange, purple, &r, &Window::new(&r, 100.0), 1.0), orange) > 0.05);
    assert!(delta_e(apply(orange, purple, &r, &Window::new(&r, -100.0), 1.0), orange) < 0.01);
}

/// A dark red edge (the shadow at a sole) does not gain chroma above its own relative level; it
/// fades towards grey.
#[test]
fn a_dark_edge_does_not_take_saturated_colour() {
    let grad = red_gradient();
    let r = reference_of(&grad);
    let w = Window::new(&r, 0.0);
    let magenta = srgb_to_oklab([0.85, 0.1, 0.6]);
    let edge = lch_to_lab([0.09, 0.05, red_hue()]);
    let o = apply(edge, magenta, &r, &w, 1.0);
    let own = chroma(edge) / r.c;
    assert!(chroma(o) / chroma(magenta) < 0.6 * own, "edge chroma {} vs its relative level {}", chroma(o), own * chroma(magenta));
}

/// A grey garment recolored to a strong colour: bright folds don't get more chroma than the target,
/// and chroma fades out above the garment's own lightness.
#[test]
fn a_grey_area_never_takes_more_chroma_than_the_target() {
    let px: Vec<[f32; 3]> = [0.15f32, 0.2, 0.25, 0.3, 0.35].iter().map(|l| [*l, 0.0, 0.0]).collect();
    let r = reference_of(&px);
    let w = Window::new(&r, 0.0);
    let red = srgb_to_oklab([0.8, 0.1, 0.1]);
    let ct = chroma(red);
    for l in [0.3f32, 0.4, 0.5, 0.6] {
        let o = apply([l, 0.0, 0.0], red, &r, &w, 1.0);
        assert!(chroma(o) <= ct + 1e-5, "fold at {l}: {} over {ct}", chroma(o));
    }
    let lit = apply([0.6, 0.0, 0.0], red, &r, &w, 1.0);
    let mid = apply([r.l, 0.0, 0.0], red, &r, &w, 1.0);
    assert!(chroma(lit) < chroma(mid), "above the area's lightness chroma fades");
    assert!(chroma(apply([1.0, 0.0, 0.0], red, &r, &w, 1.0)) < 0.01, "a specular stays neutral");
}

/// Colour to Replace: with two strong colours under the mask, it picks which one changes.
#[test]
fn colour_to_replace_picks_the_colour_that_changes() {
    let red = lch_to_lab([0.55, 0.15, red_hue()]);
    let blue = [0.5, -0.03, -0.15];
    let mut px = vec![red; 30];
    px.extend(vec![blue; 20]);
    let samples: Vec<([f32; 3], f32)> = px.iter().map(|p| (*p, 1.0)).collect();
    let green = srgb_to_oklab([0.2, 0.6, 0.2]);
    let auto = main_colour(&samples, None).expect("a main colour");
    let w = Window::new(&auto, 0.0);
    assert!(delta_e(apply(blue, green, &auto, &w, 1.0), blue) < 0.01, "automatic: the larger red");
    let from = main_colour(&samples, Some(blue)).expect("a main colour");
    let w = Window::new(&from, 0.0);
    assert!(delta_e(apply(red, green, &from, &w, 1.0), red) < 0.01, "the red stays");
    assert!(delta_e(apply(blue, green, &from, &w, 1.0), blue) > 0.05, "the blue changes");
}

/// Amount 0 (or no alpha) returns the pixel bit for bit; the terms of an Amount-0 recolor are all
/// zeros.
#[test]
fn amount_zero_and_no_alpha_change_nothing() {
    let r = Reference { l: 0.5, c: 0.1, h: [1.0, 0.0], spread: 0.1 };
    let w = Window::new(&r, 0.0);
    let p = [0.43, 0.12, -0.07];
    let t = srgb_to_oklab([0.2, 0.7, 0.3]);
    assert_eq!(apply(p, t, &r, &w, 0.0), p);
    assert_eq!(apply(p, t, &r, &w, f32::NAN), p);
    let j = LocalAdjustments { recolor: 0.0, recolor_r: 0.5, ..Default::default() };
    assert_eq!(terms(&j, 1.0, Some(&r)), [0.0; TERMS]);
    let j = LocalAdjustments { recolor: 100.0, recolor_r: 0.5, ..Default::default() };
    let tm = terms(&j, 1.0, Some(&r));
    assert_eq!(apply_terms(p, &tm, 0.0), p);
    assert_ne!(apply_terms(p, &tm, 1.0), p);
    assert_eq!(terms(&j, 1.0, None), [0.0; TERMS]);
}

/// Hostile values (NaN, infinite, negative) never panic and never give NaN.
#[test]
fn hostile_values_stay_finite() {
    let bad = [f32::NAN, f32::INFINITY, -1.0, 0.0, 2.0];
    for l in bad {
        for c in bad {
            let r = Reference { l, c, h: [c, l], spread: l };
            let w = Window { hue: c, sat: l };
            let o = apply([0.5, 0.1, 0.1], [l, c, 0.1], &r, &w, 1.0);
            assert!(o.iter().all(|v| v.is_finite()), "{o:?}");
        }
    }
    assert!(main_colour(&[], None).is_none());
    assert!(main_colour(&[([f32::NAN, 0.0, 0.0], 1.0), ([0.5, 0.0, 0.0], f32::INFINITY)], None).is_none());
    let w = Window::new(&Reference { l: 0.5, c: 0.1, h: [1.0, 0.0], spread: f32::NAN }, f64::NAN);
    assert!(w.hue.is_finite() || w.hue.is_nan());
    let mut img = vec![[0.5f32, 0.2, 0.2]; 16];
    recolor_srgb(&mut img, &[1.0; 16], [f64::NAN, 2.0, -1.0], 1.0, f64::INFINITY, Some([f64::NAN; 3]));
    assert!(img.iter().flatten().all(|v| v.is_finite()));
}

#[test]
fn hex_colours_round_trip_and_bad_ones_are_refused() {
    assert_eq!(parse_hex("#7a3fb0").map(to_hex).as_deref(), Some("#7a3fb0"));
    assert_eq!(parse_hex(" 73b ").map(to_hex).as_deref(), Some("#7733bb"));
    for bad in ["", "#12", "#12345g", "#1234567", "#éé1", "zzz"] {
        assert_eq!(parse_hex(bad), None, "{bad}");
    }
    assert_eq!(to_hex([f64::NAN, 2.0, -1.0]), "#00ff00");
}

/// The per-mask term block: with a reference the terms carry the strength, the target and the
/// window; [`with_recolor`] matches by mask id and leaves others at zeros.
#[test]
fn terms_land_on_the_right_mask_by_id() {
    use crate::finish::{MASK_RECOLOR, MASK_TERMS, mask_terms};
    let mut a = LocalAdjustments::default();
    (a.recolor, a.recolor_r, a.recolor_range, a.amount) = (100.0, 0.48, 0.0, 100.0);
    let r = Reference { l: 0.5, c: 0.12, h: [1.0, 0.0], spread: 0.2 };
    let mut terms = vec![mask_terms(&a), mask_terms(&a)];
    let ids = [7u32, 9];
    let adjusts = [a, a];
    with_recolor(&mut terms, &ids, &adjusts, &[(9, r)]);
    assert_eq!(terms[0][MASK_RECOLOR], 0.0, "mask 7 has no reference");
    assert!(terms[1][MASK_RECOLOR] > 0.0, "mask 9 recolors");
    assert_eq!(terms[1][MASK_RECOLOR + 4], r.l);
    assert_eq!(MASK_TERMS, MASK_RECOLOR + TERMS);
}
