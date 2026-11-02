//! Lightroom Classic's brush model (TEST-NOTES 2026-10-28 notes 18, 20 and 22): single clicks and strokes
//! rasterized here against the mask values measured on Lightroom's own exports of a procedural chart.

use super::*;
use lcc_develop::{BRUSH_MODEL_LEGACY, BRUSH_MODEL_LRC, DevelopSettings, brush_model};

const W: usize = 1200;
const H: usize = 1000;
/// The core radius `R0` in output pixels.
const R0: f64 = 80.0;

fn frame() -> Frame {
    Frame::new(W, H, &DevelopSettings::default(), true)
}

/// `points` (output pixels) as a stroke of the Lightroom model at `feather`.
fn stroke(points: &[(f64, f64)], feather: f64) -> BrushStroke {
    BrushStroke {
        points: points.iter().map(|(x, y)| Point::new(x / W as f64, y / H as f64)).collect(),
        size: R0 / W.max(H) as f64,
        feather,
        model: BRUSH_MODEL_LRC,
        ..Default::default()
    }
}

fn render(strokes: Vec<BrushStroke>) -> Plane {
    shape_alpha(&MaskShape::Brush { strokes }, &frame(), W, H, &Rgb32f::new(W, H), &Plane::new(W, H), 0.0, None)
}

/// The measured single click: r / R0 → m per Feather ("Brush model, exact").
const R: [f64; 11] = [0.0, 0.2, 0.4, 0.6, 0.8, 0.9, 1.0, 1.1, 1.2, 1.4, 1.6];
const CLICKS: [(f64, &[f64]); 6] = [
    (0.0, &[1.0, 1.0, 1.0, 1.0, 0.96, 0.73, 0.0]),
    (20.0, &[1.0, 0.99, 0.95, 0.88, 0.65, 0.44, 0.19, 0.0]),
    (40.0, &[1.0, 0.96, 0.84, 0.66, 0.45, 0.36, 0.26, 0.18, 0.11, 0.0]),
    (60.0, &[1.0, 0.95, 0.81, 0.62, 0.42, 0.34, 0.27, 0.20, 0.16, 0.09, 0.04]),
    (80.0, &[1.0, 0.95, 0.81, 0.61, 0.42, 0.33, 0.26, 0.20, 0.15, 0.09, 0.05]),
    (100.0, &[1.0, 0.95, 0.81, 0.61, 0.41, 0.33, 0.25, 0.19, 0.15, 0.08, 0.05]),
];

/// The measured click at `r` (in R0), linearly between the measured distances, 0 past them.
fn measured(table: &[f64], r: f64) -> f64 {
    let Some(k) = R.iter().position(|x| *x >= r) else { return 0.0 };
    if k == 0 {
        return table[0];
    }
    if k >= table.len() {
        return 0.0;
    }
    let t = (r - R[k - 1]) / (R[k] - R[k - 1]);
    table[k - 1] + (table[k] - table[k - 1]) * t
}

fn tolerance(feather: f64) -> f64 {
    // Feather 20 and 40 carry ~0.035 of measurement scatter around the fitted profile
    if feather == 20.0 || feather == 40.0 { 0.045 } else { 0.03 }
}

// Note 18: a single click matches Lightroom's (its core solid, Feather 80 much softer than 20)
#[test]
fn a_single_click_matches_lightrooms() {
    let (cx, cy) = (600.5, 500.5);
    for (f, table) in CLICKS {
        let a = render(vec![stroke(&[(cx, cy)], f)]);
        for (i, r) in R.iter().enumerate().take(table.len()) {
            let x = (cx + r * R0).floor() as usize;
            // a sample exactly on the cut (Radius = R0 at Feather 0, 1.1 R0 at 20) reads the outside
            let x = if (r - brush_model::outer_ratio(f)).abs() < 1e-6 { x + 1 } else { x };
            let got = a.get(x, cy as usize) as f64;
            assert!((got - table[i]).abs() <= tolerance(f), "Feather {f}, r {r}: {got:.3} vs {}", table[i]);
        }
    }
    // the bug of note 18: Feather 16 and 80 painted almost the same
    let soft = render(vec![stroke(&[(cx, cy)], 80.0)]);
    let hard = render(vec![stroke(&[(cx, cy)], 16.0)]);
    let at = |p: &Plane| p.get((cx + 0.6 * R0) as usize, cy as usize);
    assert!(at(&hard) - at(&soft) > 0.2, "{} vs {}", at(&hard), at(&soft));
    // and the halo reaches past Size, out to Radius
    assert!(soft.get((cx + 1.4 * R0) as usize, cy as usize) > 0.05);
}

// Notes 18 and 20: a stroke is its dabs (every 0.3 × Radius, as Lightroom saves them) added up as
// 1 − ∏(1 − m_i): its cross-section matches the one predicted from the measured clicks
#[test]
fn a_stroke_cross_section_matches_lightrooms_dabs_added_up() {
    let x0 = 600.5;
    for (f, table) in CLICKS {
        let step = DAB_SPACING * R0 * brush_model::outer_ratio(f);
        let pts: Vec<(f64, f64)> = (0..).map(|k| (x0, 100.0 + k as f64 * step)).take_while(|p| p.1 <= 900.0).collect();
        let a = render(vec![stroke(&pts, f)]);
        // the row through the dab nearest the middle and the row halfway to the next
        let mid = pts[pts.len() / 2].1;
        for y in [mid, mid + step / 2.0] {
            let y = y.floor() + 0.5;
            for r in [0.0, 0.2, 0.4, 0.6, 0.8, 0.9, 1.0, 1.2, 1.4] {
                let x = x0 + r * R0;
                let want = 1.0 - pts.iter().map(|(px, py)| 1.0 - measured(table, ((x - px).powi(2) + (y - py).powi(2)).sqrt() / R0)).product::<f64>();
                let got = a.get(x.floor() as usize, y as usize) as f64;
                assert!((got - want).abs() <= tolerance(f), "Feather {f}, r {r}, row {y}: {got:.3} vs {want:.3}");
            }
        }
    }
}

// Note 20: two soft strokes that overlap add up into one shape (no crease where they meet): along the line
// joining them the coverage is 1 − (1 − a)(1 − b), above either one alone. A legacy pair still takes the maximum.
#[test]
fn overlapping_soft_strokes_add_up_instead_of_pinching() {
    let (y, ax, bx) = (500.5, 450.5, 450.5 + 2.6 * R0);
    let one = |x: f64, model: u8| render(vec![BrushStroke { model, ..stroke(&[(x, y)], 80.0) }]);
    for model in [BRUSH_MODEL_LRC, BRUSH_MODEL_LEGACY] {
        let (a, b) = (one(ax, model), one(bx, model));
        let both = render(vec![BrushStroke { model, ..stroke(&[(ax, y)], 80.0) }, BrushStroke { model, ..stroke(&[(bx, y)], 80.0) }]);
        let mut fuller = false;
        for x in (ax as usize)..(bx as usize) {
            let (va, vb, v) = (a.get(x, y as usize), b.get(x, y as usize), both.get(x, y as usize));
            let want = if model == BRUSH_MODEL_LRC { 1.0 - (1.0 - va) * (1.0 - vb) } else { va.max(vb) };
            assert!((v - want).abs() < 1e-5, "model {model}, x {x}: {v} vs {want}");
            fuller |= v > va.max(vb) + 0.02;
        }
        assert_eq!(fuller, model == BRUSH_MODEL_LRC, "model {model}");
    }
    // density still caps the sum
    let capped =
        render(vec![BrushStroke { density: 60.0, ..stroke(&[(ax, y)], 80.0) }, BrushStroke { density: 60.0, ..stroke(&[(ax + 10.0, y)], 80.0) }]);
    assert!(capped.data.iter().all(|v| *v <= 0.6 + 1e-6));
}

// The dab walk lays the same dabs however the samples come in: a slow drag (a sample every pixel) gives the
// same stroke as one fast segment, not a denser, harder one
#[test]
fn a_slow_drag_lays_the_same_dabs_as_a_fast_one() {
    let slow: Vec<(f64, f64)> = (0..=400).map(|k| (400.5 + k as f64, 500.5)).collect();
    let fast = [(400.5, 500.5), (800.5, 500.5)];
    let (bs, bf) = (brush_dabs(&stroke(&slow, 60.0), &frame(), W, H), brush_dabs(&stroke(&fast, 60.0), &frame(), W, H));
    assert_eq!(bs.dabs.len(), bf.dabs.len());
    for (p, q) in bs.dabs.iter().zip(&bf.dabs) {
        assert!(p.dist(*q) < 1e-6, "{p:?} vs {q:?}");
    }
    let spacing = bf.dabs[1].dist(bf.dabs[0]);
    assert!((spacing - DAB_SPACING * bf.r).abs() < 1e-6, "{spacing} vs {}", bf.r);
    // a stationary or hostile sample lays nothing
    let odd = brush_dabs(&stroke(&[(500.5, 500.5), (500.5, 500.5), (f64::NAN, 3.0), (f64::INFINITY, 0.0)], 60.0), &frame(), W, H);
    assert_eq!(odd.dabs.len(), 1);
}

// Old strokes keep their look: a stroke saved before the model field reads as the legacy brush and renders
// the old way (size = the whole dab, Feather inward); a new one is saved with its model
#[test]
fn strokes_saved_before_the_lightroom_model_render_unchanged() {
    let old: BrushStroke = serde_json::from_str(r#"{"points":[{"x":0.5,"y":0.5}],"size":0.1,"feather":50.0}"#).unwrap();
    assert_eq!(old.model, BRUSH_MODEL_LEGACY);
    assert!(!serde_json::to_string(&old).unwrap().contains("model"), "an old stroke saves as it was");
    let bd = brush_dabs(&old, &frame(), W, H);
    assert!(bd.lrc.is_none());
    assert_eq!((bd.r, bd.hard), (120.0, 60.0));
    let a = render(vec![old.clone()]);
    let (cx, cy) = (600usize, 500usize);
    for dx in [0usize, 30, 59, 70, 90, 110, 119, 121] {
        let dd = Point::new(cx as f64 + dx as f64 + 0.5, cy as f64 + 0.5).dist(Point::new(600.0, 500.0));
        assert!((a.get(cx + dx, cy) - dab_alpha(dd, 120.0, 60.0)).abs() < 1e-6, "{dx}");
    }
    let new = BrushStroke { model: BRUSH_MODEL_LRC, ..old };
    let json = serde_json::to_string(&new).unwrap();
    assert!(json.contains("\"model\":1"), "{json}");
    assert_eq!(serde_json::from_str::<BrushStroke>(&json).unwrap(), new);
}

// Note 24: Auto Mask tolerances measured on Lightroom: brightness passes fully up to 0.7 EV and is blocked from
// 0.8 EV; at the same brightness, our chroma distance 0.13 passes, 0.195 → 0.93, 0.26 → 0.70, 0.40 → 0.05
#[test]
fn auto_mask_tolerances_match_lightrooms() {
    let grey = [1.0f32, 1.0, 1.0];
    for ev in [0.2f32, 0.3, 0.5, 0.7] {
        for sign in [-1.0f32, 1.0] {
            assert_eq!(auto_similarity_lrc(sign * ev, grey, 0.0, grey), 1.0, "{ev} EV");
        }
    }
    for ev in [0.83f32, 1.32, 1.85, 2.0] {
        assert_eq!(auto_similarity_lrc(-ev, grey, 0.0, grey), 0.0, "{ev} EV");
    }
    for (dc, want) in [(0.13f32, 1.0f32), (0.195, 0.93), (0.26, 0.70), (0.40, 0.05)] {
        let got = auto_similarity_lrc(0.0, [1.0 + dc, 1.0, 1.0], 0.0, grey);
        assert!((got - want).abs() <= 0.05, "chroma {dc}: {got} vs {want}");
    }
    // the legacy brush's tolerances are unchanged (0.5 EV blocks)
    assert_eq!(auto_similarity(-0.5, grey, 0.0, grey), 0.0);
}

// Note 24: an Auto Mask click across a brightness step: Lightroom passes 0.5 EV as if there were no edge and
// stops sharply at 1 EV (full strength two pixels before the edge, nothing one pixel past it)
#[test]
fn an_auto_mask_click_stops_sharply_at_an_edge_lightroom_blocks() {
    let edge = 600usize;
    for (ev, blocked) in [(0.5f32, false), (1.0, true)] {
        let mut img = Rgb32f::new(W, H);
        for (i, p) in img.data.iter_mut().enumerate() {
            let v = if i % W < edge { 0.18 } else { 0.18 * (-ev).exp2() };
            *p = [v, v, v];
        }
        let log_l = Plane { width: W, height: H, data: img.data.iter().map(|c| lcc_color::luminance_2020(*c).max(1e-6).log2()).collect() };
        let render_auto = |auto_mask: bool, model| {
            let s = BrushStroke { auto_mask, model, ..stroke(&[(edge as f64 - 60.5, 500.5)], 100.0) };
            shape_alpha(&MaskShape::Brush { strokes: vec![s] }, &frame(), W, H, &img, &log_l, 0.0, None)
        };
        let (auto, plain) = (render_auto(true, BRUSH_MODEL_LRC), render_auto(false, BRUSH_MODEL_LRC));
        let y = 500;
        if blocked {
            assert!(auto.get(edge - 2, y) >= plain.get(edge - 2, y) - 1e-4, "full strength before the edge");
            for x in edge + 1..edge + 40 {
                assert_eq!(auto.get(x, y), 0.0, "{ev} EV, {} px past the edge", x - edge);
            }
        } else {
            for x in edge - 40..edge + 40 {
                assert!((auto.get(x, y) - plain.get(x, y)).abs() < 1e-4, "{ev} EV passes at {x}");
            }
        }
    }
}

/// A stroke's Auto Mask over a brightness step of `ev`, painted with Auto Mask range `range`, and the same
/// stroke without Auto Mask.
fn auto_over_step(ev: f32, range: f64) -> (Plane, Plane) {
    let edge = 600usize;
    let mut img = Rgb32f::new(W, H);
    for (i, p) in img.data.iter_mut().enumerate() {
        let v = if i % W < edge { 0.18 } else { 0.18 * (-ev).exp2() };
        *p = [v, v, v];
    }
    let log_l = Plane { width: W, height: H, data: img.data.iter().map(|c| lcc_color::luminance_2020(*c).max(1e-6).log2()).collect() };
    let render_auto = |auto_mask: bool| {
        let s = BrushStroke { auto_mask, auto_range: range, ..stroke(&[(edge as f64 - 60.5, 500.5)], 100.0) };
        shape_alpha(&MaskShape::Brush { strokes: vec![s] }, &frame(), W, H, &img, &log_l, 0.0, None)
    };
    (render_auto(true), render_auto(false))
}

// TEST-NOTES 2026-10-31 note 1: the Auto Mask range at its default is exactly the limits measured on Lightroom
#[test]
fn the_default_auto_mask_range_is_todays_limits() {
    assert_eq!(BrushStroke::default().auto_range, 0.0);
    assert_eq!(BrushStroke::default().auto_range_scale(), 1.0);
    let grey = [1.0f32, 1.0, 1.0];
    for i in 0..=400 {
        let d = i as f32 * 0.005;
        for (l, ch) in [(d, grey), (-d, grey), (0.0, [1.0 + d, 1.0, 1.0]), (d * 0.5, [1.0, 1.0 - d * 0.3, 1.0])] {
            assert_eq!(auto_similarity_lrc_scaled(l, ch, 0.0, grey, 1.0).to_bits(), auto_similarity_lrc(l, ch, 0.0, grey).to_bits());
        }
    }
    // and a stroke saved before the slider (no `auto_range`) renders as one at range 0, bit for bit
    let s = BrushStroke { auto_mask: true, ..stroke(&[(540.0, 500.0)], 100.0) };
    let mut json = serde_json::to_value(&s).unwrap();
    assert!(json.get("auto_range").is_none(), "the default isn't written: {json}");
    json.as_object_mut().unwrap().remove("auto_range");
    let old: BrushStroke = serde_json::from_value(json).unwrap();
    assert_eq!(old, s);
    // the dab data carries the scale 1 to the rasterizer and the GPU (whose kernel multiplies by 1 / 1)
    let d = brush_dabs(&old, &frame(), W, H);
    assert_eq!(d.auto_scale, 1.0);
}

// Note 1: a tighter range stops at a smaller step, a wider one paints across a bigger one
#[test]
fn the_auto_mask_range_scales_the_limits() {
    let edge = 600usize;
    let y = 500;
    let past = |a: &Plane| (edge + 1..edge + 40).map(|x| a.get(x, y)).fold(0.0f32, f32::max);
    // 0.5 EV: passes by default, blocked at the tightest (limits 0.35 / 0.4 EV)
    let (auto, plain) = auto_over_step(0.5, 0.0);
    assert!((past(&auto) - past(&plain)).abs() < 1e-4, "0.5 EV passes at the default");
    let (tight, _) = auto_over_step(0.5, -100.0);
    assert_eq!(past(&tight), 0.0, "0.5 EV is blocked at the tightest");
    // 1 EV: blocked by default, passes at the widest (limits 1.4 / 1.6 EV)
    let (auto, plain) = auto_over_step(1.0, 0.0);
    assert_eq!(past(&auto), 0.0, "1 EV is blocked at the default");
    let (wide, _) = auto_over_step(1.0, 100.0);
    assert!((past(&wide) - past(&plain)).abs() < 1e-4, "1 EV passes at the widest");
    // the scale: ½ … 1 … 2, a hostile value clamped
    let at = |v: f64| BrushStroke { auto_range: v, ..Default::default() }.auto_range_scale();
    assert_eq!((at(-100.0), at(0.0), at(100.0)), (0.5, 1.0, 2.0));
    assert_eq!((at(-1e9), at(1e9), at(f64::NAN), at(f64::INFINITY)), (0.5, 2.0, 1.0, 1.0));
    // the chroma limits scale with it: 0.3 fades at the default, passes at the widest, is gone at the tightest
    let grey = [1.0f32, 1.0, 1.0];
    let c = [1.3f32, 1.0, 1.0];
    let mid = auto_similarity_lrc_scaled(0.0, c, 0.0, grey, 1.0);
    assert!(mid > 0.0 && mid < 1.0, "{mid}");
    assert_eq!(auto_similarity_lrc_scaled(0.0, c, 0.0, grey, 2.0), 1.0);
    assert_eq!(auto_similarity_lrc_scaled(0.0, c, 0.0, grey, 0.5), 0.0);
}

// Note 1: the range is saved with the stroke (sidecar / catalog JSON) and read back
#[test]
fn the_auto_mask_range_round_trips() {
    for v in [-100.0, -37.0, 25.0, 100.0] {
        let s = BrushStroke { auto_mask: true, auto_range: v, ..stroke(&[(500.0, 500.0)], 50.0) };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"auto_range\""), "{json}");
        assert_eq!(serde_json::from_str::<BrushStroke>(&json).unwrap(), s);
    }
}
