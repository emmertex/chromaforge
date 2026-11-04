//! Auto Mask range (TEST-NOTES 2026-10-31 note 1): `mask.brushStroke`'s `autoRange` is kept with the stroke it
//! paints, so a later change paints only the strokes after it; the sidecar and an effect preset's brush carry it;
//! a stroke without it (saved before it, or from Lightroom) paints with the default 0.

use lcc_develop::{BrushStroke, MaskShape};
use serde_json::json;

use crate::Session;

fn strokes(s: &Session) -> Vec<BrushStroke> {
    let d = s.develop_of(s.active().expect("a photo")).expect("settings");
    d.masks
        .iter()
        .flat_map(|m| m.components.iter())
        .filter_map(|c| match &c.shape {
            MaskShape::Brush { strokes } => Some(strokes.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

#[test]
fn each_stroke_keeps_the_range_it_was_painted_with() {
    let mut s = Session::with_demo();
    let id = s.visible_cloned()[0];
    s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
    s.execute("mask.brushStroke", &json!({"points": [[0.2, 0.2], [0.3, 0.2]], "autoMask": true})).unwrap();
    s.execute("mask.brushStroke", &json!({"points": [[0.2, 0.4], [0.3, 0.4]], "autoMask": true, "autoRange": -60})).unwrap();
    s.execute("mask.brushStroke", &json!({"points": [[0.2, 0.6], [0.3, 0.6]], "autoMask": true, "autoRange": 45.5})).unwrap();
    // a hostile value is clamped to the slider's ends
    s.execute("mask.brushStroke", &json!({"points": [[0.2, 0.8], [0.3, 0.8]], "autoMask": true, "autoRange": 1e9})).unwrap();
    let got: Vec<f64> = strokes(&s).iter().map(|st| st.auto_range).collect();
    assert_eq!(got, [0.0, -60.0, 45.5, 100.0], "the default, then each as painted: later strokes don't change earlier ones");

    // the sidecar keeps them
    use crate::sidecar::{DevelopPatch, parse_sidecar, sidecar_packet};
    let p = s.catalog.photo(id).unwrap().clone();
    let back = parse_sidecar(&sidecar_packet(&p, &s.catalog), crate::crs::Target::Rendered).unwrap();
    let Some(DevelopPatch::Full(d)) = &back.develop else { panic!("no lc:settings") };
    let read: Vec<f64> = d
        .masks
        .iter()
        .flat_map(|m| m.components.iter())
        .filter_map(
            |c| if let MaskShape::Brush { strokes } = &c.shape { Some(strokes.iter().map(|st| st.auto_range).collect::<Vec<_>>()) } else { None },
        )
        .flatten()
        .collect();
    assert_eq!(read, got);
}

#[test]
fn an_effects_brush_carries_the_range() {
    let mut s = Session::with_demo();
    let id = s.visible_cloned()[0];
    s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
    s.execute("mask.add", &json!({"kind": "brush"})).unwrap();
    s.execute("mask.saveEffect", &json!({"name": "Wide", "brush": {"size": 0.1, "auto_mask": true, "auto_range": 70}})).unwrap();
    s.execute("mask.saveEffect", &json!({"name": "Plain", "brush": {"size": 0.1, "auto_mask": true}})).unwrap();
    s.execute("mask.saveEffect", &json!({"name": "Bad", "brush": {"size": 0.1, "auto_range": -500}})).unwrap();
    let r = s.execute("mask.applyEffect", &json!({"name": "Wide"})).unwrap();
    assert_eq!(r["brush"]["auto_range"], 70.0);
    let r = s.execute("mask.applyEffect", &json!({"name": "Plain"})).unwrap();
    assert!(r["brush"].get("auto_range").is_none(), "the default isn't written: {r}");
    let r = s.execute("mask.applyEffect", &json!({"name": "Bad"})).unwrap();
    assert_eq!(r["brush"]["auto_range"], -100.0, "clamped");
}
