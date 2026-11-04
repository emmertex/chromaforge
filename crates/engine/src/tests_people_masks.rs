//! People masks (`mask.add {kind: "people"}`): on MediaPipe's people and faces models when they
//! are installed (`<PREFIX>_MEDIAPIPE_ROOT`: the folder holding both), on the heuristic otherwise.

use lcc_develop::MaskShape;
use serde_json::json;

use crate::Session;

fn people_shape(s: &Session) -> MaskShape {
    let id = s.active().expect("demo photo");
    let d = s.develop_of(id).expect("develop");
    d.masks.last().expect("a mask").components[0].shape.clone()
}

#[test]
fn people_masks_without_the_models_use_the_heuristic() {
    let mut s = Session::with_demo();
    s.segmenter.models_root = Some(std::env::temp_dir().join("no-such-models-folder"));
    s.execute("mask.add", &json!({"kind": "people", "parts": ["Clothes", "face skin"], "person": 2})).unwrap();
    let MaskShape::People { person, parts, seg, .. } = people_shape(&s) else { panic!("not a People mask") };
    assert_eq!((person, parts.len(), seg.is_none()), (2, 2, true));
    // Recompute AI Masks has nothing to run and says nothing
    let id = s.active().unwrap();
    let done = s.compute_masks(id, true).unwrap();
    assert!(done.notes.is_empty(), "{:?}", done.notes);
}

#[test]
fn hostile_people_arguments_are_errors_not_panics() {
    let mut s = Session::with_demo();
    assert!(s.execute("mask.add", &json!({"kind": "people", "parts": ["Elbows"]})).is_err());
    assert!(s.execute("mask.add", &json!({"kind": "people", "who": 7})).is_err());
    assert!(s.execute("mask.add", &json!({"kind": "people", "parts": "Hair"})).is_ok(), "not a list: the entire person");
    s.execute("mask.add", &json!({"kind": "people", "person": u64::MAX, "parts": vec!["Hair"; 500]})).unwrap();
    let MaskShape::People { parts, .. } = people_shape(&s) else { panic!("not a People mask") };
    assert!(parts.len() <= lcc_develop::MAX_PEOPLE_PARTS);
}

/// With the real models, a People mask on a demo photo (landscapes: nobody there) runs without
/// an error and leaves the photo's look to the models' answer.
#[cfg(feature = "sam")]
#[test]
fn people_masks_run_on_the_real_models() {
    let Some(root) = lcc_brand::env_os("MEDIAPIPE_ROOT").map(std::path::PathBuf::from) else {
        lcc_testkit::skip!(Model, "set {} to the open models' folder", lcc_brand::env_var("MEDIAPIPE_ROOT"));
        return;
    };
    let mut s = Session::with_demo();
    s.segmenter.models_root = Some(root);
    assert!(s.segmenter.people_models().is_some(), "the people and faces models are in the folder");
    s.execute("mask.add", &json!({"kind": "people"})).unwrap();
    s.execute("mask.add", &json!({"kind": "people", "parts": ["Lips", "Teeth"]})).unwrap();
    let id = s.active().unwrap();
    s.compute_masks(id, true).unwrap();
}
