//! Lens Blur through the commands: switching it on, the focus picker, the bokeh, undo, the develop controls, and
//! hostile parameters (refused, never a panic).

use lcc_develop::BokehShape;
use serde_json::json;

use crate::Session;

fn lens(s: &Session) -> lcc_develop::LensBlur {
    let id = s.active().unwrap();
    s.develop_of(id).unwrap().lens_blur.clone()
}

#[test]
fn lens_blur_is_set_picked_undone_and_refuses_bad_values() {
    let mut s = Session::with_demo();
    assert!(!lens(&s).enabled);
    let undo = s.undo.len();
    let r = s.execute("lensBlur.set", &json!({"amount": 70, "focus": {"x": 0.3, "y": 0.6}, "focusSize": 0.05, "bokeh": "hexagon"})).unwrap();
    assert_eq!(s.undo.len(), undo + 1, "one step");
    let lb = lens(&s);
    assert!(lb.active(), "a slider switches it on");
    assert_eq!((lb.amount, lb.bokeh, lb.focus_size), (70.0, BokehShape::Hexagon, 0.05));
    assert!(lb.focus.is_some_and(|p| (p.x - 0.3).abs() < 1e-9 && (p.y - 0.6).abs() < 1e-9));
    assert_eq!(r["bokeh"], "hexagon");
    assert!(r["depth"] == "model" || r["depth"] == "subject", "{r}");
    assert_eq!(s.execute("lensBlur.status", &json!({})).unwrap()["amount"], 70.0);
    // a typed focal distance replaces the picked point; out-of-range numbers are clamped
    s.execute("lensBlur.set", &json!({"focalDistance": 250, "focalRange": -4})).unwrap();
    let lb = lens(&s);
    assert_eq!((lb.focal_distance, lb.focal_range, lb.focus), (100.0, 0.0, None));
    // the sliders are develop controls too
    s.execute("develop.set", &json!({"control": "lensBlur.amount", "value": 30})).unwrap();
    assert_eq!(lens(&s).amount, 30.0);
    // Boost: by the command (clamped) and as a develop control
    assert_eq!(s.execute("lensBlur.set", &json!({"boost": 400})).unwrap()["boost"], 100.0);
    s.execute("develop.set", &json!({"control": "lensBlur.boost", "value": 25})).unwrap();
    assert_eq!(lens(&s).boost, 25.0);
    // hostile parameters are refused with a message
    for p in [
        json!({"bokeh": "triangle"}),
        json!({"focus": {"x": "left"}}),
        json!({"focus": [0.5, 0.5]}),
        json!({"amount": "lots"}),
        json!({"boost": "bright"}),
        json!({"enabled": 1}),
    ] {
        assert!(s.execute("lensBlur.set", &p).is_err(), "{p}");
    }
    // switched off, undo brings it back
    s.execute("lensBlur.set", &json!({"enabled": false})).unwrap();
    assert!(!lens(&s).enabled);
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(lens(&s).enabled);
}

/// Brush Refinement and a mask's Lens Blur through the commands: strokes are added, capped by their checks, reset
/// and undone; the mask block is set by `mask.adjust` and clamped.
#[test]
fn brush_refinement_and_mask_lens_blur_through_the_commands() {
    let mut s = Session::with_demo();
    let undo = s.undo.len();
    let r = s
        .execute(
            "lensBlur.brushStroke",
            &json!({"mode": "focus", "points": [[0.4, 0.5], [0.5, 0.5]], "size": 9, "amount": 400, "pressure": [1.0, 0.5]}),
        )
        .unwrap();
    assert_eq!((r["focusStrokes"].as_u64(), r["blurStrokes"].as_u64()), (Some(1), Some(0)));
    assert_eq!(s.undo.len(), undo + 1, "one step");
    s.execute("lensBlur.brushStroke", &json!({"mode": "blur", "points": [[0.1, 0.1]], "erase": true})).unwrap();
    let lb = lens(&s);
    assert_eq!((lb.focus_strokes.len(), lb.blur_strokes.len()), (1, 1));
    let f = &lb.focus_strokes[0];
    assert_eq!((f.size, f.density, f.points.len(), f.pressure.len()), (0.5, 100.0, 2, 2), "clamped");
    assert!(lb.blur_strokes[0].erase);
    // hostile strokes are refused with a message
    for p in [
        json!({"points": [[0.5, 0.5]]}),
        json!({"mode": "sharpen", "points": [[0.5, 0.5]]}),
        json!({"mode": "focus"}),
        json!({"mode": "focus", "points": []}),
        json!({"mode": "focus", "points": [["a", "b"], [null, 1]]}),
        json!({"mode": "focus", "points": [[0.5, 0.5]], "pressure": [1, 1, 1]}),
        json!({"mode": "blur", "points": "everywhere"}),
    ] {
        assert!(s.execute("lensBlur.brushStroke", &p).is_err(), "{p}");
    }
    assert!(s.execute("lensBlur.brushReset", &json!({"mode": "both"})).is_err());
    assert_eq!(lens(&s).focus_strokes.len(), 1, "a refused command changes nothing");
    // reset one brush, then both; an empty reset is no undo step
    assert_eq!(s.execute("lensBlur.brushReset", &json!({"mode": "blur"})).unwrap()["blurStrokes"], 0);
    assert_eq!(lens(&s).focus_strokes.len(), 1);
    s.execute("lensBlur.brushReset", &json!({})).unwrap();
    let steps = s.undo.len();
    s.execute("lensBlur.brushReset", &json!({})).unwrap();
    assert_eq!(s.undo.len(), steps);
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(lens(&s).focus_strokes.len(), 1, "undo brings the strokes back");

    // a mask's Lens Blur block
    s.execute("mask.add", &json!({"kind": "radial"})).unwrap();
    s.execute("mask.adjust", &json!({"values": {"lens_blur": 250, "lens_blur_boost": -3, "lens_blur_outside": true}})).unwrap();
    let id = s.active().unwrap();
    let a = s.develop_of(id).unwrap().masks.last().unwrap().adjust;
    assert_eq!((a.lens_blur, a.lens_blur_boost, a.lens_blur_outside, a.lens_blur_off), (100.0, 0.0, true, false));
    s.execute("mask.adjust", &json!({"values": {"lens_blur_off": true}})).unwrap();
    let a = s.develop_of(id).unwrap().masks.last().unwrap().adjust;
    assert!(a.lens_blur_off && a.lens_blur == 100.0, "switched off, the value kept");
}
