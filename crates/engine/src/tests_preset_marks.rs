//! LR-PRE-ONIMPORT / LR-PRE-MANAGE: presets marked At Import or hidden, built-in ones too, are
//! listed as such and kept with the library.

use serde_json::json;

use crate::Session;

#[test]
fn at_import_and_hidden_marks_toggle_and_are_listed() {
    let mut s = Session::with_demo();
    let id = s.presets.first().unwrap().id.clone();
    let g = s.presets.generation();
    assert_eq!(s.execute("preset.forImport", &json!({"id": id})).unwrap(), json!({"on": true}));
    assert!(s.presets.generation() != g, "a mark is a change of the preset list");
    assert_eq!(s.execute("preset.hide", &json!({"id": id, "hidden": true})).unwrap(), json!({"hidden": true}));
    let list = s.execute("presets.list", &json!({})).unwrap();
    let row = list.as_array().unwrap().iter().find(|p| p["id"] == json!(id)).unwrap().clone();
    assert_eq!((row["atImport"].clone(), row["hidden"].clone()), (json!(true), json!(true)));
    assert_eq!(s.execute("preset.forImport", &json!({"id": id})).unwrap(), json!({"on": false}));
    assert!(s.preset_at_import.is_empty() && s.preset_hidden == vec![id]);
    assert!(s.execute("preset.hide", &json!({"id": "no.such"})).is_err());
    assert!(s.execute("preset.forImport", &json!({})).is_err());
}
