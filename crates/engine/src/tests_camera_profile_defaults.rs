//! The default camera profile per camera model (issue #723): set and cleared with
//! `library.preferences` (`cameraDefault`), saved with the library preferences, and handed to the
//! decoder (`lcc_raw::dcp`), whose `resolve` decides through it when a photo chooses no camera
//! profile of its own.

use std::path::PathBuf;

use serde_json::json;

use crate::Session;

/// The decoder's camera defaults are one per process: the tests that set them take turns.
static DECODER_DEFAULTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-camera-profile-defaults-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn stored(s: &Session) -> Vec<(String, String)> {
    s.camera_profile_defaults.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

#[test]
fn the_default_is_set_removed_and_handed_to_the_decoder() {
    let _turn = DECODER_DEFAULTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = Session::with_demo();
    // unknown ids are refused, so a typo can't silently drop photos to a plain colour
    assert!(s.execute("library.preferences", &json!({"cameraDefault": {"camera": "NIKON Z 8", "profile": "dcp:gone"}})).is_err());
    assert!(s.execute("library.preferences", &json!({"cameraDefault": {"camera": "NIKON Z 8", "profile": "cal:gone"}})).is_err());
    assert!(s.execute("library.preferences", &json!({"cameraDefault": {"camera": "NIKON Z 8", "profile": "sort of"}})).is_err());
    assert!(s.execute("library.preferences", &json!({"cameraDefault": {"camera": "", "profile": "lc"}})).is_err());
    // a global and a per-camera choice
    s.execute("library.preferences", &json!({"cameraDefault": {"camera": "*", "profile": "lc"}})).unwrap();
    s.execute("library.preferences", &json!({"cameraDefault": {"camera": "NIKON Z 8", "profile": "lc"}})).unwrap();
    assert_eq!(stored(&s), vec![("".to_string(), "lc".to_string()), ("NIKON Z 8".to_string(), "lc".to_string())]);
    assert_eq!(lcc_raw::dcp::user_defaults(), stored(&s), "the decoder decides through the same list");
    // the preference reads back
    let prefs = s.execute("library.preferences", &json!({})).unwrap();
    assert_eq!(prefs["cameraDefaults"][""], "lc");
    assert_eq!(prefs["cameraDefaults"]["NIKON Z 8"], "lc");
    // null removes an entry, the per-camera one only
    s.execute("library.preferences", &json!({"cameraDefault": {"camera": "NIKON Z 8", "profile": null}})).unwrap();
    assert_eq!(stored(&s), vec![("".to_string(), "lc".to_string())]);
    s.execute("library.preferences", &json!({"cameraDefault": {"camera": "*", "profile": null}})).unwrap();
    assert!(stored(&s).is_empty());
    assert!(lcc_raw::dcp::user_defaults().is_empty());
}

#[test]
fn the_default_persists_with_the_library_and_reinstalls_on_reopen() {
    let _turn = DECODER_DEFAULTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let lib = temp_dir("persist");
    {
        let mut s = Session::new().with_fs();
        s.open_library(&lib, true).unwrap();
        s.execute("library.preferences", &json!({"cameraDefault": {"camera": "NIKON Z 8", "profile": "lc"}})).unwrap();
        s.execute("library.preferences", &json!({"cameraDefault": {"camera": "*", "profile": "lc"}})).unwrap();
        s.close_library().unwrap();
        // the decoder's registry is per process: pretend a fresh start emptied it
        lcc_raw::dcp::install_user_defaults(Vec::new());
    }
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    assert_eq!(stored(&s), vec![("".to_string(), "lc".to_string()), ("NIKON Z 8".to_string(), "lc".to_string())]);
    assert_eq!(lcc_raw::dcp::user_defaults(), stored(&s), "a reopened library hands its defaults to the decoder again");
    s.close_library().unwrap();
    let _ = std::fs::remove_dir_all(&lib);
}
