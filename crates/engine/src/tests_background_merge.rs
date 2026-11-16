//! Ported from OneSlips 1c785da00 (test notes 2026-10-24 note 18): a background result (an AI mask's selection, a
//! Retouch or Smooth Skin analysis) is merged into the photo's settings as they are when it arrives, and into its
//! undo / redo steps, a running drag's starting points and its History, so no edit made meanwhile is lost and none
//! of those drops the result.

use lcc_develop::{DevelopSettings, controls};
use serde_json::json;

use crate::Session;

/// The stand-in result: Highlights set to -33 (only where nothing set it).
fn result(d: &mut DevelopSettings) -> bool {
    if controls::get(d, "light.highlights").is_some_and(|v| v != 0.0) {
        return false;
    }
    controls::set(d, "light.highlights", -33.0)
}

fn get(s: &Session, id: lcc_catalog::PhotoId, c: &str) -> f64 {
    controls::get(&s.develop_of(id).unwrap(), c).unwrap()
}

#[test]
fn a_background_result_merges_into_the_edits_made_while_it_ran() {
    let mut s = Session::with_demo();
    let id = s.active().unwrap();
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.5})).unwrap();
    // a drag running when the result arrives
    s.execute("develop.beginInteraction", &json!({"label": "Contrast"})).unwrap();
    s.execute("develop.set", &json!({"control": "light.contrast", "value": 20.0})).unwrap();
    assert!(s.merge_background(id, "Test", false, &result).unwrap(), "the current settings take it");
    assert_eq!((get(&s, id, "light.exposure"), get(&s, id, "light.contrast"), get(&s, id, "light.highlights")), (0.5, 20.0, -33.0));
    // the drag cancelled: back to its start, which has the result
    s.execute("develop.cancelInteraction", &json!({})).unwrap();
    assert_eq!((get(&s, id, "light.exposure"), get(&s, id, "light.contrast"), get(&s, id, "light.highlights")), (0.5, 0.0, -33.0));
    // History has it on every step
    let p = s.catalog.photo(id).unwrap();
    assert!(!p.history.is_empty());
    for st in &p.history {
        assert_eq!(controls::get(&st.settings, "light.highlights"), Some(-33.0), "History step {}", st.label);
    }
    // undo the Exposure edit: the result stays; redo brings the edit back with it
    s.undo_step().unwrap();
    assert_eq!((get(&s, id, "light.exposure"), get(&s, id, "light.highlights")), (0.0, -33.0), "undo keeps the result");
    s.redo_step().unwrap();
    assert_eq!((get(&s, id, "light.exposure"), get(&s, id, "light.highlights")), (0.5, -33.0), "redo too");
    // a result whose place is taken meanwhile takes nothing
    s.execute("develop.set", &json!({"control": "light.highlights", "value": 10.0})).unwrap();
    assert!(!s.merge_background(id, "Test", false, &result).unwrap());
    assert_eq!(get(&s, id, "light.highlights"), 10.0);
    // an unknown photo is no error
    assert!(!s.merge_background(lcc_catalog::PhotoId(u64::MAX), "Test", false, &result).unwrap());
}
