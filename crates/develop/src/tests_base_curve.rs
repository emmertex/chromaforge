//! `profile.base_curve`: the setting, its JSON, hash, copy/paste and the library hash. Synthetic curves only.

use crate::DevelopSettings;
use crate::base_curve;
use crate::presets::{SettingsGroup, apply_partial, extract_groups};
use serde_json::json;

fn with(id: &str) -> DevelopSettings {
    let mut s = DevelopSettings::default();
    s.profile.base_curve = id.into();
    s
}

fn curve_file(mid: f64) -> Vec<u8> {
    json!({
        "format": "base-curve", "version": 1,
        "curves": { "a": { "interpolation": "linear", "points": [[0, 0], [0.5, mid], [1, 1]] } },
        "apply": [ { "op": "curve", "curve": "a", "on": "luminance" } ]
    })
    .to_string()
    .into_bytes()
}

#[test]
fn off_by_default_and_left_out_of_the_json_and_the_hash_while_empty() {
    let d = DevelopSettings::default();
    assert!(d.profile.base_curve.is_empty());
    assert!(d.to_json()["profile"].get("base_curve").is_none(), "{}", d.to_json()["profile"]);
    // settings that never had it keep their JSON and hash
    assert_eq!(d.hash64(), with("").hash64());
    assert_eq!(d.to_json(), with("").to_json());
    // the JSON of the default settings, as it was before the field existed: the profile object has exactly these keys
    let keys: Vec<String> = d.to_json()["profile"].as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys, ["amount", "id"], "{keys:?}");
}

#[test]
fn a_selected_curve_is_stored_round_trips_and_changes_the_hash() {
    let s = with("base:Example");
    assert_eq!(s.to_json()["profile"]["base_curve"], json!("base:Example"));
    assert_eq!(DevelopSettings::from_json(&s.to_json()).unwrap(), s);
    assert_ne!(s.hash64(), DevelopSettings::default().hash64());
    // settings written before the field existed load with it empty
    let old = json!({"profile": {"id": "lc.color", "amount": 100.0}});
    assert!(DevelopSettings::from_json(&old).unwrap().profile.base_curve.is_empty());
}

#[test]
fn a_value_that_is_not_an_id_is_dropped_when_settings_come_from_outside() {
    for bad in ["x", "base:", "base:a/b", "curve:Example", "base:..\\x"] {
        let s = DevelopSettings::from_untrusted_json(&with(bad).to_json()).unwrap();
        assert!(s.profile.base_curve.is_empty(), "{bad}");
        let m = DevelopSettings::default().merged(&json!({"profile": {"base_curve": bad}})).unwrap();
        assert!(m.profile.base_curve.is_empty(), "{bad}");
    }
    let m = DevelopSettings::default().merged(&json!({"profile": {"base_curve": "base:Example"}})).unwrap();
    assert_eq!(m.profile.base_curve, "base:Example");
}

#[test]
fn copying_the_profile_writes_the_curve_even_when_empty_so_pasting_resets_it() {
    let unedited = DevelopSettings::default();
    let copied = extract_groups(&unedited, &[SettingsGroup::Profile]);
    assert_eq!(copied["profile"]["base_curve"], json!(""), "{copied}");
    let onto = with("base:Example");
    assert_eq!(apply_partial(&onto, &copied, 1.0).profile.base_curve, "");
    // and a curve is carried
    let copied = extract_groups(&with("base:Example"), &[SettingsGroup::Profile]);
    assert_eq!(apply_partial(&unedited, &copied, 1.0).profile.base_curve, "base:Example");
    // other groups leave it alone
    let light = extract_groups(&unedited, &[SettingsGroup::Light]);
    assert_eq!(apply_partial(&onto, &light, 1.0).profile.base_curve, "base:Example");
}

#[test]
fn the_curve_files_content_is_part_of_the_hash_and_a_missing_file_counts_too() {
    let s = with("base:HashTest");
    base_curve::install_library(vec![]);
    let missing = s.hash64();
    assert_ne!(missing, DevelopSettings::default().hash64());
    let (a, b) = (curve_file(0.7), curve_file(0.8));
    let parsed = base_curve::parse(&a).unwrap();
    base_curve::install_library(vec![("base:HashTest".into(), parsed, base_curve::hash_bytes(&a))]);
    let with_a = s.hash64();
    assert_ne!(with_a, missing, "an installed curve differs from a missing one");
    let parsed = base_curve::parse(&b).unwrap();
    base_curve::install_library(vec![("base:HashTest".into(), parsed, base_curve::hash_bytes(&b))]);
    assert_ne!(s.hash64(), with_a, "a replaced file changes the hash");
    // settings without a curve do not depend on the library at all
    assert_eq!(DevelopSettings::default().hash64(), with("").hash64());
    base_curve::install_library(vec![]);
}
