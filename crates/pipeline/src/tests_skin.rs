//! Smooth Skin: its bands, what it may touch, and that a preview matches an export.

use lcc_develop::{DevelopSettings, SegMask, SkinAnalysis, SkinFace, SkinSmoothing};
use lcc_geom::Orientation;
use lcc_raster::Rgb32f;

use crate::geometry::Frame;
use crate::skin;

/// Linear skin colour.
const SKIN: [f32; 3] = [0.42, 0.27, 0.20];

/// Settings with Smooth Skin at `k`, its skin `inside(x, y)` (normalized) on a 256 grid, one face
/// at the centre with eyes `eyes` of the long edge apart.
fn settings(k: SkinSmoothing, eyes: f64, inside: impl Fn(f64, f64) -> bool) -> DevelopSettings {
    let side = 256;
    let logits: Vec<f32> = (0..side * side)
        .map(|i| {
            let (x, y) = (((i % side) as f64 + 0.5) / side as f64, ((i / side) as f64 + 0.5) / side as f64);
            if inside(x, y) { 12.0 } else { -12.0 }
        })
        .collect();
    let seg = SegMask::from_logits(side, &logits);
    DevelopSettings {
        skin: k,
        skin_analysis: Some(SkinAnalysis {
            area: k.area,
            person: k.person,
            orientation: Orientation::Normal,
            seg,
            faces: vec![SkinFace { x: 0.5, y: 0.5, eyes }],
        }),
        ..Default::default()
    }
}

fn smooth(img: &mut Rgb32f, s: &DevelopSettings) {
    let frame = Frame::new(img.width, img.height, s, true);
    let ppl = frame.px_per_long(img.width);
    skin::apply(img, s, &frame, ppl);
}

/// A skin-coloured image whose luminance carries `bands` (wavelength in eye distances, relative
/// amplitude) as sinusoids along x, eyes `eyes_px` apart.
fn banded(w: usize, h: usize, eyes_px: f64, bands: &[(f64, f32)]) -> Rgb32f {
    Rgb32f::from_fn(w, h, |x, y| {
        let mut g = 1.0f32;
        for (k, (wl, amp)) in bands.iter().enumerate() {
            let phase = k as f64 * 1.3 + (y as f64 * 0.0);
            g *= 1.0 + amp * ((x as f64 / (wl * eyes_px) * std::f64::consts::TAU + phase).sin() as f32);
        }
        SKIN.map(|c| c * g)
    })
}

/// Amplitude (relative) of the sinusoid of wavelength `wl_px` in row `y` of the luminance,
/// between columns `x0..x1` (a single-frequency projection).
fn amplitude(img: &Rgb32f, y: usize, x0: usize, x1: usize, wl_px: f64) -> f64 {
    let lum: Vec<f64> = (x0..x1).map(|x| img.data[y * img.width + x]).map(|p| (0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2]) as f64).collect();
    let mean = lum.iter().sum::<f64>() / lum.len() as f64;
    let (mut s, mut c) = (0.0, 0.0);
    for (i, v) in lum.iter().enumerate() {
        let a = (x0 + i) as f64 / wl_px * std::f64::consts::TAU;
        s += (v - mean) * a.sin();
        c += (v - mean) * a.cos();
    }
    2.0 * (s * s + c * c).sqrt() / lum.len() as f64 / mean
}

#[test]
fn the_middle_band_softens_while_pores_and_the_faces_shape_stay() {
    // eyes 300 px apart: pores of 0.006 d, uneven texture of 0.06 d, the face's shape at 1 d
    let (w, h, d) = (1200, 300, 300.0);
    let bands = [(0.006, 0.04f32), (0.06, 0.04), (1.0, 0.15)];
    let before = banded(w, h, d, &bands);
    let s = settings(SkinSmoothing { amount: 100.0, ..Default::default() }, d / w as f64, |_, _| true);
    let mut after = before.clone();
    smooth(&mut after, &s);
    let (y, x0, x1) = (h / 2, 300, 900);
    let ratio = |wl: f64| amplitude(&after, y, x0, x1, wl * d) / amplitude(&before, y, x0, x1, wl * d);
    let (pores, middle, shape) = (ratio(0.006), ratio(0.06), ratio(1.0));
    assert!(middle < 0.5, "the middle band is softened: {middle:.3}");
    assert!((0.85..1.15).contains(&pores), "pores stay: {pores:.3}");
    assert!((0.93..1.07).contains(&shape), "the face's shape stays: {shape:.3}");
}

#[test]
fn strong_edges_inside_the_skin_stay() {
    // a step of a stop (a shadow edge, the side of the nose) in the middle of the skin
    let (w, h, d) = (800, 200, 250.0);
    let before = Rgb32f::from_fn(w, h, |x, _| SKIN.map(|c| if x < w / 2 { c } else { c * 0.5 }));
    let s = settings(SkinSmoothing { amount: 100.0, even_tone: 100.0, ..Default::default() }, d / w as f64, |_, _| true);
    let mut after = before.clone();
    smooth(&mut after, &s);
    let y = h / 2;
    let edge = |img: &Rgb32f| (img.data[y * w + w / 2 - 20][1] / img.data[y * w + w / 2 + 20][1]).log2();
    assert!(edge(&after) > 0.85 * edge(&before), "{} vs {}", edge(&after), edge(&before));
}

#[test]
fn even_tone_and_redness_reduce_blotches() {
    // a skin with a red blotch of 0.1 d (a pimple's halo, a patch of redness), strong or mild
    let (w, h, d) = (800, 800, 300.0);
    let (cx, cy, r) = (400.0, 400.0, 30.0);
    let blotch = |red: f32| {
        Rgb32f::from_fn(w, h, |x, y| {
            let q = (((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2)).sqrt() / r).min(3.0);
            let t = (-q * q / 2.0).exp() as f32;
            [SKIN[0] * (1.0 + red * t), SKIN[1] * (1.0 - red / 5.0 * t), SKIN[2]]
        })
    };
    let redness = |img: &Rgb32f| {
        let c = img.data[400 * w + 400];
        let o = img.data[400 * w + 700];
        (c[0] / c[1]).log2() - (o[0] / o[1]).log2()
    };
    // Redness takes any patch redder than the skin around it; Even Tone the mild ones (a strong
    // colour difference is more likely light, or a feature, than a blotch)
    for (k, red, most) in [
        (SkinSmoothing { redness: 100.0, ..Default::default() }, 0.25, 0.5),
        (SkinSmoothing { redness: 100.0, ..Default::default() }, 0.06, 0.5),
        (SkinSmoothing { even_tone: 100.0, ..Default::default() }, 0.06, 0.7),
    ] {
        let before = blotch(red);
        let mut after = before.clone();
        smooth(&mut after, &settings(k, d / w as f64, |_, _| true));
        assert!(redness(&after) < most * redness(&before), "{k:?} {red}: {} vs {}", redness(&after), redness(&before));
        assert!(redness(&after) > 0.0, "{k:?}: towards the skin around it, not past it");
    }
}

#[test]
fn nothing_outside_the_skin_changes() {
    // skin: a disc with the eyes and the mouth cut out
    let (w, h) = (600, 600);
    let hole = |x: f64, y: f64| ((x - 0.4).hypot(y - 0.42) < 0.05) || ((x - 0.6).hypot(y - 0.42) < 0.05) || ((x - 0.5).hypot(y - 0.62) < 0.06);
    let inside = move |x: f64, y: f64| (x - 0.5).hypot(y - 0.5) < 0.3 && !hole(x, y);
    let before = banded(w, h, 120.0, &[(0.006, 0.05), (0.05, 0.08), (0.2, 0.1)]);
    let s = settings(SkinSmoothing { amount: 100.0, texture: -50.0, even_tone: 100.0, redness: 100.0, ..Default::default() }, 0.2, inside);
    let mut after = before.clone();
    smooth(&mut after, &s);
    let mut changed_inside = 0;
    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
            let (a, b) = (after.data[y * w + x], before.data[y * w + x]);
            // well outside the skin (or inside a hole), beyond the stored grid's soft edge
            let far = (nx - 0.5).hypot(ny - 0.5) > 0.31
                || ((nx - 0.4).hypot(ny - 0.42) < 0.04)
                || ((nx - 0.6).hypot(ny - 0.42) < 0.04)
                || ((nx - 0.5).hypot(ny - 0.62) < 0.05);
            if far {
                assert!(a.iter().zip(&b).all(|(p, q)| (p - q).abs() <= 1e-4 * q.abs().max(1e-3)), "{x},{y} changed: {a:?} vs {b:?}");
            } else if inside(nx, ny) && a != b {
                changed_inside += 1;
            }
        }
    }
    assert!(changed_inside > 50_000, "the skin itself is smoothed ({changed_inside} pixels)");
}

#[test]
fn off_another_orientation_or_damaged_data_change_nothing() {
    let before = banded(300, 200, 80.0, &[(0.05, 0.1)]);
    let check = |s: &DevelopSettings| {
        let mut after = before.clone();
        smooth(&mut after, s);
        assert_eq!(after.data, before.data, "{:?}", s.skin);
    };
    check(&settings(SkinSmoothing::default(), 0.2, |_, _| true));
    let mut rotated = settings(SkinSmoothing::SUBTLE, 0.2, |_, _| true);
    rotated.orientation = Orientation::Rotate90;
    check(&rotated);
    let mut other = settings(SkinSmoothing::SUBTLE, 0.2, |_, _| true);
    other.skin.person = 2;
    check(&other);
    let mut off = settings(SkinSmoothing::SUBTLE, 0.2, |_, _| true);
    off.set_section_enabled("skin", false);
    check(&off);
    let mut damaged = settings(SkinSmoothing::SUBTLE, 0.2, |_, _| true);
    if let Some(a) = damaged.skin_analysis.as_mut() {
        a.seg.data = "not base64!".into();
    }
    check(&damaged);
    // hostile faces and sizes don't panic
    let mut weird = settings(SkinSmoothing { amount: 100.0, ..Default::default() }, 0.2, |_, _| true);
    if let Some(a) = weird.skin_analysis.as_mut() {
        a.faces = vec![
            SkinFace { x: f64::NAN, y: 1e300, eyes: f64::INFINITY },
            SkinFace { x: -5.0, y: 0.5, eyes: -1.0 },
            SkinFace { x: 0.5, y: 0.5, eyes: 1e-12 },
        ];
    }
    let mut img = before.clone();
    smooth(&mut img, &weird);
    assert!(img.data.iter().flatten().all(|v| v.is_finite()));
    for (w, h) in [(0, 0), (1, 1), (1, 500), (3, 2)] {
        let mut tiny = Rgb32f::new(w, h);
        smooth(&mut tiny, &settings(SkinSmoothing::SUBTLE, 0.2, |_, _| true));
    }
    // out-of-gamut and non-finite pixels are left as they are
    let mut odd = Rgb32f::from_fn(64, 64, |x, _| if x % 2 == 0 { [-0.1, 0.2, 0.3] } else { [f32::NAN, 0.2, 0.1] });
    let copy = odd.clone();
    smooth(&mut odd, &settings(SkinSmoothing { amount: 100.0, ..Default::default() }, 0.2, |_, _| true));
    assert!(odd.data.iter().zip(&copy.data).all(|(a, b)| a.iter().zip(b).all(|(p, q)| p == q || (p.is_nan() && q.is_nan()))));
}

#[test]
fn a_preview_matches_an_export() {
    // the same photo at a third of the size (a 2.5 MP loupe of a 24 MP export), smoothed: compare
    // after shrinking the big one; eyes a quarter of the width apart
    let s = settings(SkinSmoothing::SUBTLE, 0.25, |x, y| (x - 0.5).hypot(y - 0.5) < 0.35);
    let make = |w: usize| {
        let f = w as f64 / 1800.0;
        banded(w, w * 3 / 4, 450.0 * f, &[(0.03, 0.04), (0.06, 0.06), (0.15, 0.08)])
    };
    let mut big = make(1800);
    let mut small = make(600);
    let (big0, small0) = (big.clone(), small.clone());
    smooth(&mut big, &s);
    smooth(&mut small, &s);
    // the change each made, the big one's averaged down 3 × 3
    let mut err = 0.0f64;
    let mut sum = 0.0f64;
    for y in 0..small.height {
        for x in 0..small.width {
            let mut d_big = 0.0f64;
            for yy in 0..3 {
                for xx in 0..3 {
                    let i = (y * 3 + yy) * big.width + x * 3 + xx;
                    d_big += f64::from(big.data[i][1] - big0.data[i][1]) / 9.0;
                }
            }
            let i = y * small.width + x;
            let d_small = f64::from(small.data[i][1] - small0.data[i][1]);
            err += (d_big - d_small).abs();
            sum += d_small.abs();
        }
    }
    assert!(sum > 1.0, "something was smoothed");
    assert!(err / sum < 0.15, "the preview's change matches the export's (relative error {:.3})", err / sum);
}

#[test]
fn faces_of_different_sizes_get_their_own_bands() {
    // a big face at the left (eyes 200 px apart), a small one at the right (30 px)
    let (w, h) = (1000, 500);
    let mut s = settings(SkinSmoothing { amount: 100.0, ..Default::default() }, 0.2, |_, _| true);
    if let Some(a) = s.skin_analysis.as_mut() {
        a.faces = vec![SkinFace { x: 0.25, y: 0.5, eyes: 0.2 }, SkinFace { x: 0.85, y: 0.5, eyes: 0.03 }];
    }
    // texture of 30 px everywhere: the big face's middle band, but the small face's shape
    let before = banded(w, h, 200.0, &[(0.15, 0.05)]);
    let mut after = before.clone();
    smooth(&mut after, &s);
    let big = amplitude(&after, 250, 100, 400, 30.0) / amplitude(&before, 250, 100, 400, 30.0);
    let small = amplitude(&after, 250, 820, 880, 30.0) / amplitude(&before, 250, 820, 880, 30.0);
    assert!(big < 0.6, "softened on the big face: {big:.3}");
    assert!(small > 0.9, "on the small face it is its shape, kept: {small:.3}");
}

#[test]
fn a_windowed_render_reads_the_room_it_needs() {
    let s = settings(SkinSmoothing::SUBTLE, 0.1, |_, _| true);
    assert!((skin::reach(&s, 1000.0) - (3.0 * skin::LOW * 100.0).ceil()).abs() < 1e-9);
    assert_eq!(skin::reach(&DevelopSettings::default(), 1000.0), 0.0);
    assert!(skin::key(&s).is_some() && skin::key(&DevelopSettings::default()).is_none());
    let mut t = s.clone();
    t.skin.amount += 1.0;
    assert_ne!(skin::key(&s), skin::key(&t), "a slider changes the key");
}

#[test]
fn a_whole_render_with_smooth_skin_matches_a_windowed_one() {
    use crate::{PixelWindow, RenderRequest, SourceInfo, render};
    let src = banded(800, 600, 100.0, &[(0.05, 0.08), (0.2, 0.05)]);
    let s = settings(SkinSmoothing { amount: 80.0, even_tone: 60.0, ..Default::default() }, 0.125, |x, y| (x - 0.5).hypot(y - 0.5) < 0.3);
    let info = SourceInfo::default();
    let full = render(&src, &info, &s, &RenderRequest::fit(800, 600)).image;
    let win = PixelWindow { x: 380, y: 280, w: 64, h: 48 };
    let part = render(&src, &info, &s, &RenderRequest { window: Some(win), ..RenderRequest::fit(800, 600) }).image;
    let mut worst = 0u8;
    for y in 0..win.h {
        for x in 0..win.w {
            let a = full.data[(win.y + y) * 800 + win.x + x];
            let b = part.data[y * win.w + x];
            for k in 0..3 {
                worst = worst.max(a[k].abs_diff(b[k]));
            }
        }
    }
    assert!(worst <= 2, "window differs by {worst}");
    // and the smoothing shows in the render at all
    let plain = render(&src, &info, &DevelopSettings::default(), &RenderRequest::fit(800, 600)).image;
    assert_ne!(plain.data, full.data);
}

#[test]
#[ignore = "timing: cargo test -p lcc-pipeline --release smooth_skin_speed -- --ignored --nocapture"]
fn smooth_skin_speed() {
    // a 2.5 MP portrait whose face and neck are a third of the frame
    let (w, h) = (1936, 1290);
    let s = settings(SkinSmoothing::SUBTLE, 0.09, |x, y| ((x - 0.5) / 0.6).hypot((y - 0.5) / 0.9) < 0.5);
    let img = banded(w, h, 0.09 * w as f64, &[(0.006, 0.03), (0.05, 0.05), (0.2, 0.05)]);
    for _ in 0..3 {
        let mut a = img.clone();
        let t = std::time::Instant::now();
        smooth(&mut a, &s);
        eprintln!("smooth skin {w}×{h}: {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
    }
}

#[test]
fn the_fast_logarithm_and_power_are_accurate() {
    use crate::skin::{fast_exp2, fast_log2};
    for i in 0..10_000 {
        let x = 1e-6f32 * 1.0017f32.powi(i * 2);
        assert!((fast_log2(x) - x.log2()).abs() < 2e-6, "{x}: {} vs {}", fast_log2(x), x.log2());
    }
    for i in -2000..2000 {
        let x = i as f32 / 100.0;
        let (a, b) = (fast_exp2(x), x.exp2());
        assert!((a - b).abs() <= 4e-6 * b, "{x}: {a} vs {b}");
    }
    assert_eq!(fast_exp2(0.0), 1.0);
    assert_eq!(fast_exp2(-200.0), 0.0);
    assert!(fast_exp2(500.0).is_finite() && fast_exp2(f32::NAN) == 1.0);
}

#[test]
fn reduce_shine_tones_down_hot_spots_and_leaves_the_rest() {
    // a shiny spot of 0.1 d, a stop brighter than the skin and paler, in a skin that ends at x = 0.75
    let (w, h, d) = (800, 800, 300.0);
    let (cx, cy, r) = (300.0, 400.0, 30.0);
    let before = Rgb32f::from_fn(w, h, |x, y| {
        let q = (((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2)).sqrt() / r).min(3.0);
        let t = (-q * q / 2.0).exp() as f32;
        // (brighter, and towards white)
        SKIN.map(|c| c * (1.0 + t) + 0.25 * t)
    });
    let s = settings(SkinSmoothing { shine: 100.0, ..Default::default() }, d / w as f64, |x, _| x < 0.75);
    let mut after = before.clone();
    smooth(&mut after, &s);
    let lum = |p: [f32; 3]| 0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2];
    let at = |img: &Rgb32f, x: usize, y: usize| img.data[y * w + x];
    let (hot0, hot1) = (lum(at(&before, 300, 400)), lum(at(&after, 300, 400)));
    let skin0 = lum(at(&before, 500, 400));
    assert!((hot1 / skin0).log2() < 0.6 * (hot0 / skin0).log2(), "the hot spot is toned down: {hot0} → {hot1} (skin {skin0})");
    // its colour comes back towards the skin's (redder than the washed-out spot)
    let rg = |p: [f32; 3]| (p[0] / p[1]).log2();
    assert!(rg(at(&after, 300, 400)) > rg(at(&before, 300, 400)) + 0.02);
    // the plain skin around it and everything outside the skin stay
    for (x, y) in [(550, 400), (500, 150)] {
        let (a, b) = (at(&before, x, y), at(&after, x, y));
        assert!((lum(a) / lum(b) - 1.0).abs() < 0.02, "skin at {x},{y}: {a:?} → {b:?}");
    }
    assert_eq!(at(&before, 700, 400), at(&after, 700, 400), "outside the skin");
    // off: nothing changes
    let mut same = before.clone();
    smooth(&mut same, &settings(SkinSmoothing::default(), d / w as f64, |x, _| x < 0.75));
    assert_eq!(same.data, before.data);
}
