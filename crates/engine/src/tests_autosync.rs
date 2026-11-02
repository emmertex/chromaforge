//! Auto Sync merges masks into each photo's own and carries AI masks as recipes.

use serde_json::json;

use crate::Session;

fn demo() -> Session {
    Session::with_demo()
}

/// A stored segmentation for `mask.add {seg}` (no model needed), all `v`.
fn seg(v: f32) -> serde_json::Value {
    serde_json::to_value(lcc_develop::SegMask::from_logits(4, &[v; 16])).unwrap()
}

fn masks(s: &Session, id: lcc_catalog::PhotoId) -> Vec<lcc_develop::Mask> {
    s.develop_of(id).unwrap().masks.clone()
}

/// Note 1: with Auto Sync on, an AI mask made on the active photo goes to the other selected photos
/// as a recipe, to be computed from each photo's own pixels: never the active photo's bitmap, for
/// every AI kind (an Object's clicks stay on the photo they were made on). The other photos keep
/// their own masks (merged, not replaced); a later edit of the mask carries only what changed and
/// leaves each photo's own computed selection; deleting it deletes each copy; one undo step each.
#[test]
fn auto_sync_carries_ai_masks_as_recipes_and_keeps_each_photos_own_masks() {
    use lcc_develop::MaskShape;
    let mut s = demo();
    let ids: Vec<u64> = s.visible_cloned().iter().take(3).map(|p| p.0).collect();
    let [a, b, c] = [0, 1, 2].map(|i| lcc_catalog::PhotoId(ids[i]));
    // b has a hand-drawn mask of its own
    s.execute("library.select", &json!({"ids": [b.0]})).unwrap();
    s.execute("mask.add", &json!({"kind": "linear", "start": [0.5, 0.0], "end": [0.5, 0.4], "name": "B's own"})).unwrap();
    s.execute("mask.adjust", &json!({"values": {"exposure": -1.0}})).unwrap();
    let b_own = masks(&s, b);
    assert_eq!(b_own.len(), 1);
    s.execute("library.select", &json!({"ids": [a.0, b.0, c.0], "active": a.0})).unwrap();
    s.execute("develop.autoSync", &json!({"on": true})).unwrap();

    // every AI kind, made on a with its segmentation
    // (Subject, Sky and Background are evaluated in the pipeline here: no stored selection)
    let kinds = [json!({"kind": "prompt", "text": "the red car"}), json!({"kind": "object", "points": [[0.4, 0.5]]})];
    for k in &kinds {
        let mut p = k.clone();
        p["seg"] = seg(2.0);
        s.execute("mask.add", &p).unwrap();
    }
    let on_a = masks(&s, a);
    assert_eq!(on_a.len(), kinds.len());
    assert!(on_a.iter().all(|m| m.components[0].shape.has_seg()), "a keeps the selections made on it");
    for (photo, before) in [(b, b_own.len()), (c, 0)] {
        let got = masks(&s, photo);
        assert_eq!(got.len(), before + kinds.len(), "merged into the photo's own masks: {got:#?}");
        if photo == b {
            assert_eq!(got[0], b_own[0], "b's own mask stays as it was");
        }
        for m in &got[before..] {
            let shape = &m.components[0].shape;
            assert!(shape.is_ai() && !shape.has_seg(), "a recipe, never a's bitmap: {shape:?}");
            assert!(m.needs_compute(), "computed on this photo");
            if let MaskShape::Object { hint, exclude, .. } = shape {
                assert!(hint.is_empty() && exclude.is_empty(), "an Object's clicks stay on a: picked again here");
            }
        }
        let ids: std::collections::HashSet<u32> = got.iter().map(|m| m.id).collect();
        assert_eq!(ids.len(), got.len(), "no two masks share an id");
    }

    // b's Describe mask gets its own selection (as computing it on b would)
    let mut d = (*s.develop_of(b).unwrap()).clone();
    let own = lcc_develop::SegMask::from_logits(4, &[-3.0; 16]);
    if let MaskShape::Prompt { seg, .. } = &mut d.masks[1].components[0].shape {
        *seg = Some(own.clone());
    }
    s.set_develop(b, d, "Update AI Mask").unwrap();

    // an edit of the Describe mask on a: only the changed setting travels; b keeps its own pixels
    let subject = on_a[0].id;
    s.execute("mask.select", &json!({"id": subject})).unwrap();
    s.execute("mask.adjust", &json!({"values": {"exposure": 0.7}})).unwrap();
    let got = masks(&s, b);
    assert_eq!(got[0], b_own[0], "b's own mask is untouched by an edit of another");
    assert!((got[1].adjust.exposure - 0.7).abs() < 1e-9, "the change travels: {:?}", got[1].adjust);
    assert!(matches!(&got[1].components[0].shape, MaskShape::Prompt { seg: Some(x), .. } if *x == own), "b keeps the selection computed on it");
    assert!((masks(&s, c)[0].adjust.exposure - 0.7).abs() < 1e-9);
    // recomputing a's selection carries nothing
    let before_b = masks(&s, b);
    let mut d = (*s.develop_of(a).unwrap()).clone();
    if let MaskShape::Prompt { seg, .. } = &mut d.masks[0].components[0].shape {
        *seg = Some(lcc_develop::SegMask::from_logits(4, &[5.0; 16]));
    }
    s.set_develop(a, d, "Update AI Mask").unwrap();
    assert_eq!(masks(&s, b), before_b, "a's own recompute stays on a");

    // a hand-drawn mask syncs as it is
    s.execute("mask.add", &json!({"kind": "radial", "center": [0.3, 0.6], "rx": 0.1, "ry": 0.1})).unwrap();
    let radial_a = masks(&s, a).last().unwrap().components.clone();
    assert_eq!(masks(&s, b).last().unwrap().components, radial_a);

    // deleting the Describe mask on a deletes each photo's copy, nothing else; one undo restores all
    let (nb, nc) = (masks(&s, b).len(), masks(&s, c).len());
    s.execute("mask.delete", &json!({"id": subject})).unwrap();
    let got = masks(&s, b);
    assert_eq!(got.len(), nb - 1);
    assert_eq!(got[0], b_own[0]);
    assert!(!got.iter().any(|m| matches!(m.components[0].shape, MaskShape::Prompt { .. })));
    assert_eq!(masks(&s, c).len(), nc - 1);
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(masks(&s, b).len(), nb);
    assert_eq!(masks(&s, c).len(), nc);
}

/// Note 1 for Copy / Paste and Sync Settings: an Object mask arrives without the clicks made on
/// the other photo (the object is picked again on each photo), and no AI bitmap travels.
#[test]
fn pasted_object_masks_leave_their_clicks_behind() {
    use lcc_develop::MaskShape;
    let mut s = demo();
    let ids: Vec<u64> = s.visible_cloned().iter().take(2).map(|p| p.0).collect();
    let (a, b) = (lcc_catalog::PhotoId(ids[0]), lcc_catalog::PhotoId(ids[1]));
    s.execute("library.select", &json!({"ids": [a.0]})).unwrap();
    s.execute("mask.add", &json!({"kind": "object", "points": [[0.4, 0.5]], "exclude": [[0.1, 0.1]], "seg": seg(2.0)})).unwrap();
    s.execute("develop.copy", &json!({"groups": ["masks"]})).unwrap();
    s.execute("library.select", &json!({"ids": [b.0]})).unwrap();
    s.execute("develop.paste", &json!({})).unwrap();
    let got = masks(&s, b);
    assert_eq!(got.len(), 1);
    let MaskShape::Object { hint, exclude, seg, .. } = &got[0].components[0].shape else { panic!("an Object: {got:?}") };
    assert!(hint.is_empty() && exclude.is_empty() && seg.is_none(), "no clicks or pixels from a");
}
