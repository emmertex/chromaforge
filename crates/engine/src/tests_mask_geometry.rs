//! Masks stay on the photo under Rotate and Transform (`Mask::on_photo`).

use lcc_develop::{Mask, MaskComponent, MaskOp, MaskShape};
use lcc_geom::Point;
use serde_json::json;

use crate::Session;

fn radial(p: Point) -> MaskComponent {
    MaskComponent {
        anchor: None,
        name: None,
        op: MaskOp::Add,
        invert: false,
        shape: MaskShape::Radial { center: p, rx: 0.1, ry: 0.1, angle: 0.0, feather: 50.0, invert: false },
    }
}

#[test]
fn rotate_turns_the_masks_with_the_photo_and_back() {
    let mut s = Session::with_demo();
    let id = s.active().unwrap();
    s.execute("mask.add", &json!({"kind": "radial", "center": [0.2, 0.3], "rx": 0.1, "ry": 0.1})).unwrap();
    s.execute("photo.rotateRight", &json!({})).unwrap();
    let d = s.develop_of(id).unwrap();
    let MaskShape::Radial { center, .. } = d.masks[0].components[0].shape else { panic!("radial") };
    assert!((center.x - 0.7).abs() < 1e-9 && (center.y - 0.2).abs() < 1e-9, "turned with the photo: {center:?}");
    s.execute("photo.rotateLeft", &json!({})).unwrap();
    let MaskShape::Radial { center, .. } = s.develop_of(id).unwrap().masks[0].components[0].shape else { panic!("radial") };
    assert!((center.x - 0.2).abs() < 1e-9 && (center.y - 0.3).abs() < 1e-9, "{center:?}");
}

#[test]
fn an_old_mask_moves_onto_the_photo_at_its_next_edit_without_moving_on_screen() {
    let mut s = Session::with_demo();
    let id = s.active().unwrap();
    let mut d = (*s.develop_of(id).unwrap()).clone();
    d.geometry.vertical = 40.0;
    d.masks.push(Mask { id: 1, components: vec![radial(Point::new(0.3, 0.4))], on_photo: false, ..Default::default() });
    s.catalog.apply(lcc_catalog::Op::SetDevelop { id, settings: std::sync::Arc::new(d), label: "old".into(), edited: None }).unwrap();
    // any edit: the old mask goes onto the photo, rendering where it did
    s.execute("mask.adjust", &json!({"id": 1, "values": {"exposure": 1.0}})).unwrap();
    let d = s.develop_of(id).unwrap();
    assert!(d.masks[0].on_photo);
    let MaskShape::Radial { center, .. } = d.masks[0].components[0].shape else { panic!("radial") };
    assert!((center.x - 0.3).hypot(center.y - 0.4) > 0.005, "its numbers are on the photo now: {center:?}");
    // a Transform change no longer moves its numbers (it stays on the content)
    s.execute("develop.set", &json!({"control": "geometry.vertical", "value": 0.0})).unwrap();
    let MaskShape::Radial { center: c2, .. } = s.develop_of(id).unwrap().masks[0].components[0].shape else { panic!("radial") };
    assert_eq!(center, c2);
    // a new mask is on the photo from the start
    s.execute("mask.add", &json!({"kind": "radial"})).unwrap();
    assert!(s.develop_of(id).unwrap().masks.iter().all(|m| m.on_photo));
}
