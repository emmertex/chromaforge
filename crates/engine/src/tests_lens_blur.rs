//! Lens Blur through the commands: switching it on, the focus picker, the bokeh, undo, the develop controls, and
//! hostile parameters (refused, never a panic).

use lcc_develop::BokehShape;
use serde_json::json;

use crate::Session;

fn lens(s: &Session) -> lcc_develop::LensBlur {
    let id = s.active().unwrap();
    s.develop_of(id).unwrap().lens_blur
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
    // hostile parameters are refused with a message
    for p in [
        json!({"bokeh": "triangle"}),
        json!({"focus": {"x": "left"}}),
        json!({"focus": [0.5, 0.5]}),
        json!({"amount": "lots"}),
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
