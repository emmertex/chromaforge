//! Glow (beyond Lightroom, after DaVinci Resolve's Glow): the bright parts above a threshold,
//! spread as a halo over the picture. Behaviour and invariants, not pixel goldens.

use lcc_develop::{DevelopSettings, Glow, GlowComposite};
use lcc_raster::Rgb32f;

use crate::{RenderRequest, SourceInfo, render};

fn glow(mut f: impl FnMut(&mut Glow)) -> DevelopSettings {
    let mut s = DevelopSettings::default();
    f(&mut s.glow);
    s
}

fn out(src: &Rgb32f, s: &DevelopSettings) -> lcc_raster::Rgba8 {
    render(src, &SourceInfo::default(), s, &RenderRequest::fit(src.width, src.height)).image
}

fn mean(img: &lcc_raster::Rgba8) -> f32 {
    img.data.iter().map(|p| 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32).sum::<f32>() / img.len() as f32
}

/// A bright patch on a dark field: glow sources worth spreading, and darkness to receive it.
fn bright_patch() -> Rgb32f {
    Rgb32f::from_fn(240, 160, |x, y| {
        let inside = (90..150).contains(&x) && (60..100).contains(&y);
        if inside { [0.9, 0.85, 0.7] } else { [0.02, 0.025, 0.03] }
    })
}

#[test]
fn glow_off_changes_nothing() {
    let src = bright_patch();
    let a = out(&src, &DevelopSettings::default());
    // every slider at its neutral amount (0), however the others are set
    let mut other = glow(|g| {
        g.threshold = 0.0;
        g.spread = 100.0;
        g.gain = 100.0;
        g.gamma = 100.0;
        g.ratio = 100.0;
        g.recovery = 100.0;
    });
    other.glow.amount = 0.0;
    let b = out(&src, &other);
    assert_eq!(a.data, b.data, "Glow at Amount 0 must leave the render alone");
}

#[test]
fn glow_brightens_and_stays_bounded() {
    let src = bright_patch();
    let base = out(&src, &DevelopSettings::default());
    let lit = out(&src, &glow(|g| {
        g.amount = 100.0;
        g.spread = 100.0;
    }));
    assert!(mean(&lit) > mean(&base), "the halo adds light: {} vs {}", mean(&lit), mean(&base));
    assert_eq!(lit.len(), src.len(), "the render completes whole");
    // the dark field next to the patch takes some halo, but never more light than the patch holds
    let (x, y) = (70, 80);
    let dark = lit.get(x, y)[1] as i32;
    assert!(dark > base.get(x, y)[1] as i32, "halo reaches into the dark field");
    assert!(dark < 200, "the halo is a veil, not a flood: {dark}");
}

#[test]
fn threshold_gates_the_glow() {
    let src = bright_patch();
    let base = out(&src, &DevelopSettings::default());
    // the dark field alone never reaches a high threshold: nothing glows, the render is unchanged
    let shut = out(&src, &glow(|g| {
        g.amount = 100.0;
        g.threshold = 90.0;
    }));
    assert_eq!(base.get(10, 10), shut.get(10, 10), "a dark pixel above no threshold stays as it was");
    // and a low threshold makes more of the picture glow than a high one
    let low = out(&src, &glow(|g| {
        g.amount = 100.0;
        g.threshold = 0.0;
    }));
    let high = out(&src, &glow(|g| {
        g.amount = 100.0;
        g.threshold = 60.0;
    }));
    assert!(mean(&low) > mean(&high), "lower thresholds take in more sources");
}

#[test]
fn screen_is_softer_than_add() {
    let src = bright_patch();
    let s = |mode| glow(|g| {
        g.amount = 100.0;
        g.gain = 100.0;
        g.mode = mode;
    });
    let (screen, add) = (out(&src, &s(GlowComposite::Screen)), out(&src, &s(GlowComposite::Add)));
    assert!(mean(&add) > mean(&screen), "Add is the hotter composite: {} vs {}", mean(&add), mean(&screen));
}

#[test]
fn recovery_brings_detail_back() {
    // bright stripes: detail the halo smears, that Recovery is meant to give back
    let src = Rgb32f::from_fn(240, 160, |x, _| {
        let stripe = (x / 4) % 2 == 0;
        if stripe { [0.85, 0.85, 0.85] } else { [0.55, 0.55, 0.55] }
    });
    let variance = |r: &lcc_raster::Rgba8| {
        let v: Vec<f32> = r.data.iter().map(|p| p[1] as f32).collect();
        let m = v.iter().sum::<f32>() / v.len() as f32;
        v.iter().map(|x| (x - m).powi(2)).sum::<f32>() / v.len() as f32
    };
    let off = glow(|g| g.amount = 100.0);
    let on = glow(|g| {
        g.amount = 100.0;
        g.recovery = 100.0;
    });
    let (a, b) = (variance(&out(&src, &off)), variance(&out(&src, &on)));
    assert!(b > a, "Recovery keeps the bright regions' detail: {b} vs {a}");
}

#[test]
fn glow_scales_with_the_frame() {
    // the halo is measured against the long edge: a preview and an export lift the dark field
    // by about the same amount (not by their pixel counts)
    let lift = |w: usize| {
        let h = w * 2 / 3;
        let src = Rgb32f::from_fn(w, h, |x, y| {
            let (sx, sy) = (x as f32 * 240.0 / w as f32, y as f32 * 160.0 / h as f32);
            let inside = (90..150).contains(&(sx as usize)) && (60..100).contains(&(sy as usize));
            if inside { [0.9, 0.85, 0.7] } else { [0.02, 0.025, 0.03] }
        });
        let (base, lit) = (out(&src, &DevelopSettings::default()), out(&src, &glow(|g| g.amount = 100.0)));
        (base.get((w * 5 / 100).min(w - 1), h / 2)[1] as f32, lit.get((w * 5 / 100).min(w - 1), h / 2)[1] as f32)
    };
    let lift_at = |w: usize| {
        let (base, lit) = lift(w);
        lit - base
    };
    let (a, b) = (lift_at(480), lift_at(1440));
    assert!((a - b).abs() <= (a.max(b) * 0.35 + 3.0), "the halo's lift is relative: {a} vs {b}");
}

#[test]
fn hostile_values_never_panic() {
    let src = bright_patch();
    for (amount, threshold, spread, gain, gamma, ratio, recovery) in [
        (f64::NAN, 50.0, 25.0, 50.0, 50.0, 50.0, 0.0),
        (100.0, f64::NAN, 25.0, 50.0, 50.0, 50.0, 0.0),
        (100.0, 50.0, f64::INFINITY, 50.0, 50.0, 50.0, 0.0),
        (1e308, -1e308, 1e308, -1e308, 1e308, -1e308, 1e308),
        (-100.0, 200.0, -50.0, 300.0, -20.0, 400.0, -10.0),
    ] {
        let s = glow(|g| {
            g.amount = amount;
            g.threshold = threshold;
            g.spread = spread;
            g.gain = gain;
            g.gamma = gamma;
            g.ratio = ratio;
            g.recovery = recovery;
        });
        let r = out(&src, &s);
        assert_eq!((r.width, r.height), (src.width, src.height), "renders whole for {:?}", s.glow);
    }
}

#[test]
fn the_section_eye_switches_glow_off() {
    let src = bright_patch();
    let mut s = glow(|g| g.amount = 100.0);
    let lit = out(&src, &s);
    s.disabled_sections.push("effects".into());
    let off = out(&src, &s);
    assert!(mean(&off) < mean(&lit), "the Effects eye turns Glow off with the panel");
}
