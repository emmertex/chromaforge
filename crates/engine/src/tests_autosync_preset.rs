//! Develop's Presets panel (`preset.apply {followAutoSync}`) follows the slider rule: Auto Sync
//! off, the active photo only; on, every selected photo in one undo step. Without the flag
//! (Quick Develop, "Apply to N Selected Photos", agents) the preset goes on the selection.

use serde_json::json;

use crate::Session;

#[test]
fn a_develop_preset_follows_auto_sync_like_a_slider() {
    let mut s = Session::with_demo();
    let ids: Vec<u64> = s.visible_cloned().iter().take(3).map(|p| p.0).collect();
    let all: Vec<lcc_catalog::PhotoId> = ids.iter().copied().map(lcc_catalog::PhotoId).collect();
    let before: Vec<_> = all.iter().map(|id| s.develop_of(*id).unwrap()).collect();
    let changed = |s: &Session| -> Vec<bool> { all.iter().zip(&before).map(|(id, b)| s.develop_of(*id).unwrap() != *b).collect() };
    s.execute("library.select", &json!({"ids": ids, "active": ids[1]})).unwrap();
    s.execute("develop.autoSync", &json!({"on": false})).unwrap();
    let pid = s.presets.iter().find(|p| p.builtin && !p.settings.is_null()).map(|p| p.id.clone()).expect("a built-in preset");
    s.execute("preset.apply", &json!({"id": pid, "followAutoSync": true})).unwrap();
    assert_eq!(changed(&s), [false, true, false], "Auto Sync off: the active photo only");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(changed(&s), [false, false, false]);
    s.execute("develop.autoSync", &json!({"on": true})).unwrap();
    s.execute("preset.apply", &json!({"id": pid, "followAutoSync": true})).unwrap();
    assert_eq!(changed(&s), [true, true, true], "Auto Sync on: every selected photo");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(changed(&s), [false, false, false], "one undo restores every photo it touched");
    // on purpose, on the whole selection, Auto Sync or not
    s.execute("develop.autoSync", &json!({"on": false})).unwrap();
    s.execute("preset.apply", &json!({"id": pid})).unwrap();
    assert_eq!(changed(&s), [true, true, true]);
    s.execute("edit.undo", &json!({})).unwrap();
    // explicit ids still win
    s.execute("preset.apply", &json!({"id": pid, "ids": [ids[2]], "followAutoSync": true})).unwrap();
    assert_eq!(changed(&s), [false, false, true]);
}

/// One edit on several photos (Auto Sync, Sync Settings, a preset on a selection) gives each
/// photo's History step the same batch id, a new one per edit; one photo's own edit has none; a
/// step without an id is saved as before, and reads back so.
#[test]
fn an_edit_on_several_photos_shares_one_history_batch_id() {
    let mut s = Session::with_demo();
    let ids: Vec<u64> = s.visible_cloned().iter().take(3).map(|p| p.0).collect();
    let [a, b, c] = [ids[0], ids[1], ids[2]].map(lcc_catalog::PhotoId);
    s.execute("library.select", &json!({"ids": ids, "active": a.0})).unwrap();
    s.execute("develop.autoSync", &json!({"on": true})).unwrap();
    let last = |s: &Session, id| s.catalog.photo(id).unwrap().history.last().cloned().unwrap();
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.3})).unwrap();
    let first = last(&s, a).batch;
    assert!(first.is_some());
    assert!([b, c].iter().all(|id| last(&s, *id).batch == first), "the same id on every photo's step");
    s.execute("develop.set", &json!({"control": "light.contrast", "value": 10.0})).unwrap();
    let second = last(&s, a).batch;
    assert!(second.is_some() && second != first, "a new id per edit");
    let listed = s.execute("history.list", &json!({"id": b.0})).unwrap();
    assert_eq!(listed["batches"].as_array().and_then(|v| v.last()).and_then(|v| v.as_u64()), second);
    // one photo's own edit
    s.execute("develop.autoSync", &json!({"on": false})).unwrap();
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.1})).unwrap();
    let own = last(&s, a);
    assert_eq!(own.batch, None);
    // saved without the field, read back without an id; a hostile id is refused, not a panic
    let v = serde_json::to_value(&own).unwrap();
    assert!(v.get("batch").is_none());
    let back: lcc_catalog::HistoryStep = serde_json::from_value(v.clone()).unwrap();
    assert_eq!(back.batch, None);
    let mut bad = v;
    bad["batch"] = json!("not a number");
    assert!(serde_json::from_value::<lcc_catalog::HistoryStep>(bad).is_err());
}
