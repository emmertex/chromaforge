//! The Recolor commands (`mask.recolor`, `mask.recolorPick`), their clamping, and that an edit
//! without Recolor saves without the Recolor fields.

use serde_json::json;

use crate::Session;

fn demo() -> Session {
    Session::with_demo()
}

fn active_dev(s: &Session) -> lcc_develop::DevelopSettings {
    (*s.develop_of(s.active().unwrap()).unwrap()).clone()
}

fn mask_adjust(s: &mut Session, mask: u32) -> lcc_develop::LocalAdjustments {
    active_dev(s).masks.iter().find(|m| m.id == mask).map(|m| m.adjust).unwrap_or_default()
}

#[test]
fn recolor_sets_the_target_amount_and_range() {
    let mut s = demo();
    s.execute("mask.add", &json!({"kind": "radial", "center": {"x": 0.5, "y": 0.5}})).unwrap();
    let mask = s.active_mask.unwrap();
    let r = s.execute("mask.recolor", &json!({"id": mask, "color": "#7a3fb0", "amount": 80, "range": 25})).unwrap();
    let a = mask_adjust(&mut s, mask);
    assert_eq!(a.recolor, 80.0);
    assert_eq!((a.recolor_r, a.recolor_g, a.recolor_b), (122.0 / 255.0, 63.0 / 255.0, 176.0 / 255.0), "{r}");
    assert_eq!(a.recolor_range, 25.0);
    assert_eq!(r["recolor"], 80.0);
    // an amount alone, and a bad colour is an error that changes nothing
    s.execute("mask.recolor", &json!({"id": mask, "amount": 50})).unwrap();
    assert_eq!(mask_adjust(&mut s, mask).recolor, 50.0);
    let e = s.execute("mask.recolor", &json!({"id": mask, "color": "purple"})).unwrap_err().to_string();
    assert!(e.contains("#rrggbb"), "{e}");
    assert_eq!(mask_adjust(&mut s, mask).recolor, 50.0);
    // hostile numbers clamp instead of passing through
    s.execute("mask.recolor", &json!({"id": mask, "amount": 5000, "range": -1e9})).unwrap();
    let a = mask_adjust(&mut s, mask);
    assert_eq!((a.recolor, a.recolor_range), (100.0, -100.0));
    // Color to Replace: set and back to Automatic
    s.execute("mask.recolor", &json!({"id": mask, "from": "#224466"})).unwrap();
    assert!(mask_adjust(&mut s, mask).recolor_from_r > 0.0);
    s.execute("mask.recolor", &json!({"id": mask, "from": null})).unwrap();
    let a = mask_adjust(&mut s, mask);
    assert_eq!((a.recolor_from_r, a.recolor_from_g, a.recolor_from_b), (0.0, 0.0, 0.0));
}

#[test]
fn recolor_with_no_mask_sets_the_next_mask() {
    let mut s = demo();
    let r = s.execute("mask.recolor", &json!({"color": "#112233", "amount": 60})).unwrap();
    assert_eq!(r["recolor"], 60.0);
    let next = s.execute("mask.next", &json!({})).unwrap();
    assert_eq!(next["next"]["recolor"], json!(60.0));
}

#[test]
fn recolor_pick_samples_a_colour_into_the_mask() {
    let mut s = demo();
    s.execute("mask.add", &json!({"kind": "radial", "center": {"x": 0.5, "y": 0.5}})).unwrap();
    let mask = s.active_mask.unwrap();
    let r = s.execute("mask.recolorPick", &json!({"id": mask, "x": 0.5, "y": 0.1})).unwrap();
    let hex = r["hex"].as_str().unwrap().to_string();
    assert!(hex.len() == 7 && hex.starts_with('#'), "{hex}");
    let a = mask_adjust(&mut s, mask);
    assert!(a.recolor_r > 0.0 || a.recolor_g > 0.0 || a.recolor_b > 0.0, "a colour was stored: {r}");
    // into Color to Replace
    let r = s.execute("mask.recolorPick", &json!({"id": mask, "x": 0.5, "y": 0.5, "target": "from"})).unwrap();
    assert_eq!(r["target"], "from");
    let a = mask_adjust(&mut s, mask);
    assert!(a.recolor_from_r > 0.0 || a.recolor_from_g > 0.0 || a.recolor_from_b > 0.0, "{r}");
    // a point off the photo is an error, and changes nothing
    let e = s.execute("mask.recolorPick", &json!({"id": mask, "x": 5.0, "y": 0.5}));
    assert!(e.is_err(), "off-photo picks fail");
}

#[test]
fn an_edit_without_recolor_serializes_without_the_recolor_fields() {
    let mut s = demo();
    s.execute("mask.add", &json!({"kind": "radial", "center": {"x": 0.3, "y": 0.3}})).unwrap();
    s.execute("mask.recolor", &json!({"color": "#7a3fb0", "amount": 100})).unwrap();
    let with = serde_json::to_string(&active_dev(&s).masks[0].adjust).unwrap();
    assert!(with.contains("recolor"), "{with}");
    // the amount back to 0 leaves the fields out again
    s.execute("mask.recolor", &json!({"amount": 0})).unwrap();
    let without = serde_json::to_string(&active_dev(&s).masks[0].adjust).unwrap();
    assert!(!without.contains("\"recolor\":"), "{without}");
    // and a mask that never saw Recolor round-trips byte for byte as before
    s.execute("mask.add", &json!({"kind": "radial", "center": {"x": 0.7, "y": 0.7}})).unwrap();
    s.execute("mask.adjust", &json!({"values": {"exposure": 0.5}})).unwrap();
    let plain = active_dev(&s).masks[1].adjust;
    let text = serde_json::to_string(&plain).unwrap();
    assert!(!text.contains("recolor"), "{text}");
    let back: lcc_develop::LocalAdjustments = serde_json::from_str(&text).unwrap();
    assert_eq!(plain, back);
}
