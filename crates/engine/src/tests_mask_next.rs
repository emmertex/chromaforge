//! The next mask's settings, Lightroom Classic's way (test notes 2026-10-28 #21): a new mask gets
//! exactly what was dialed in while no mask existed yet (none selected, or a tool armed before its
//! first stroke), whatever its tool; editing an existing mask never changes them.

use lcc_catalog::PhotoId;
use serde_json::json;

use crate::Session;

fn exposure_of_last(s: &Session, id: PhotoId) -> f64 {
    s.develop_of(id).unwrap().masks.last().map(|m| m.adjust.exposure).expect("a mask")
}

#[test]
fn new_masks_get_what_was_dialed_with_no_mask_and_edits_never_travel() {
    let mut s = Session::with_demo();
    let ids: Vec<PhotoId> = s.visible_cloned().into_iter().take(2).collect();
    let (a, b) = (ids[0], ids[1]);
    s.execute("library.select", &json!({"ids": [a.0]})).unwrap();
    // dialed in with no mask: the next mask's settings
    s.execute("mask.adjust", &json!({"values": {"exposure": 0.4}})).unwrap();
    assert_eq!(s.mask_next.exposure, 0.4);
    s.execute("mask.add", &json!({"kind": "radial"})).unwrap();
    assert_eq!(exposure_of_last(&s, a), 0.4);
    // editing that mask changes only it
    s.execute("mask.adjust", &json!({"values": {"exposure": -1.2, "contrast": 20}})).unwrap();
    assert_eq!(s.mask_next.exposure, 0.4, "an existing mask's edit never becomes the next mask's settings");
    assert_eq!(s.mask_next.contrast, 0.0);
    // an Effect on an existing Radial: neither the next settings nor the next Radial take it
    s.mask_effects.push(serde_json::from_value(json!({"name": "Dodge", "adjust": {"exposure": 0.5}})).unwrap());
    s.execute("mask.applyEffect", &json!({"name": "Dodge"})).unwrap();
    assert_eq!(exposure_of_last(&s, a), 0.5, "the effect is on the existing Radial");
    assert_eq!(s.mask_next.exposure, 0.4);
    s.execute("mask.select", &json!({"id": null})).unwrap();
    s.execute("mask.add", &json!({"kind": "radial"})).unwrap();
    assert_eq!(s.develop_of(a).unwrap().masks.last().unwrap().adjust, s.mask_next, "the next Radial: exactly the shown settings");
    // a Brush gets the same (whatever the tool), and the settings stay for the mask after it
    s.execute("mask.select", &json!({"id": null})).unwrap();
    s.execute("mask.brushStroke", &json!({"points": [[0.2, 0.2], [0.3, 0.2]]})).unwrap();
    assert_eq!(exposure_of_last(&s, a), 0.4);
    // on another photo too (Lightroom keeps them across photos)
    s.execute("library.select", &json!({"ids": [b.0]})).unwrap();
    s.execute("mask.add", &json!({"kind": "linear"})).unwrap();
    assert_eq!(exposure_of_last(&s, b), 0.4);
    // Reset Settings on New Mask: a mask takes what was dialed in, then they're back at 0
    s.execute("mask.select", &json!({"id": null})).unwrap();
    s.execute("mask.next", &json!({"resetNew": true})).unwrap();
    s.execute("mask.adjust", &json!({"values": {"exposure": 0.9}})).unwrap();
    s.execute("mask.add", &json!({"kind": "radial"})).unwrap();
    assert_eq!(exposure_of_last(&s, b), 0.9);
    assert_eq!(s.mask_next.exposure, 0.0, "the sliders now show what the next mask gets: 0");
    s.execute("mask.select", &json!({"id": null})).unwrap();
    s.execute("mask.add", &json!({"kind": "radial"})).unwrap();
    assert_eq!(exposure_of_last(&s, b), 0.0);
}
