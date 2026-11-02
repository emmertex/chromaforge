//! Smooth Skin and Find Blemishes through the commands: the sliders, the skin found for each photo
//! (stored without an undo step, kept through undo and redo, never copied to another photo), the
//! sidecar, and the errors without the models. With the real models (`LC_AI_MODELS_TEST_DIR`)
//! and a portrait (`LC_RETOUCH_TEST_PHOTO`), the whole path runs on it.

use lcc_develop::{SegMask, SettingsGroup, SkinAnalysis, SkinArea, SkinFace, SkinSmoothing};
use lcc_geom::Orientation;
use serde_json::json;

use crate::{PhotoId, Session};

/// A skin over the middle of the photo, one face at its centre.
fn analysis() -> SkinAnalysis {
    let side = 64;
    let logits: Vec<f32> = (0..side * side)
        .map(|i| {
            let (x, y) = ((i % side) as f64 / side as f64, (i / side) as f64 / side as f64);
            if (x - 0.5).hypot(y - 0.5) < 0.3 { 12.0 } else { -12.0 }
        })
        .collect();
    SkinAnalysis {
        area: SkinArea::Face,
        person: 0,
        orientation: Orientation::Normal,
        seg: SegMask::from_logits(side, &logits),
        faces: vec![SkinFace { x: 0.5, y: 0.5, eyes: 0.1 }],
    }
}

/// A demo session without any AI models (an empty models folder).
fn demo() -> (Session, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("lc-retouch-{}-{}", std::process::id(), rand_suffix()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Session::with_demo();
    s.segmenter.models_root = Some(dir.clone());
    (s, dir)
}

fn rand_suffix() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::SeqCst)
}

fn skin_of(s: &Session, id: PhotoId) -> (SkinSmoothing, Option<SkinAnalysis>) {
    let d = s.develop_of(id).unwrap();
    (d.skin, d.skin_analysis.clone())
}

#[test]
fn smooth_skin_switches_on_subtly_and_says_which_model_it_needs() {
    let (mut s, dir) = demo();
    let id = s.active().unwrap();
    let undo = s.undo.len();
    let r = s.execute("skin.smooth", &json!({})).unwrap();
    assert_eq!(skin_of(&s, id).0, SkinSmoothing::SUBTLE);
    assert_eq!(r["active"], true);
    assert_eq!(r["analysis"], "missing", "{r}");
    if crate::segment::Segmenter::AVAILABLE {
        let e = r["error"].as_str().unwrap();
        assert!(e.contains("ai.models.download"), "names the model to download: {e}");
    }
    assert_eq!(s.undo.len(), undo + 1, "one step");
    // a failure is not asked again on every command (no repeated messages), until Find Skin
    s.segment_poll();
    s.execute("skin.set", &json!({"amount": 50})).unwrap();
    assert!(s.segment_poll().messages.is_empty());
    // Find Skin without the models: the error a frontend offers the download on
    match s.execute("skin.find", &json!({})) {
        Err(e) if crate::segment::Segmenter::AVAILABLE => assert!(matches!(e.kind(), crate::ErrorKind::ModelMissing(_)), "{e}"),
        r => assert!(r.is_err() || r.as_ref().is_ok_and(|v| v["error"].is_string()), "{r:?}"),
    }
    // area and person, and bad values
    s.execute("skin.set", &json!({"area": "both", "person": 2, "texture": -500})).unwrap();
    let k = skin_of(&s, id).0;
    assert_eq!((k.area, k.person, k.texture, k.amount), (SkinArea::Both, 2, -100.0, 50.0));
    for p in [json!({"area": "knees"}), json!({"person": 9}), json!({"person": -1}), json!({"person": "two"})] {
        assert!(s.execute("skin.set", &p).is_err(), "{p}");
    }
    // the sliders are develop controls too
    s.execute("develop.set", &json!({"control": "skin.redness", "value": 70})).unwrap();
    assert_eq!(skin_of(&s, id).0.redness, 70.0);
    // off, and undo brings it back
    s.execute("skin.smooth", &json!({"off": true})).unwrap();
    assert!(!skin_of(&s, id).0.active());
    assert_eq!(s.execute("skin.status", &json!({})).unwrap()["analysis"], "off");
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(skin_of(&s, id).0.active());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_skin_found_is_no_undo_step_and_stays_through_undo_and_redo() {
    let (mut s, dir) = demo();
    let id = s.active().unwrap();
    s.execute("skin.smooth", &json!({})).unwrap();
    let a = analysis();
    let undo = s.undo.len();
    s.store_skin_analysis(id, &a).unwrap();
    assert_eq!(s.undo.len(), undo, "finding the skin is not an edit");
    assert_eq!(skin_of(&s, id).1.as_ref(), Some(&a));
    assert_eq!(s.execute("skin.status", &json!({})).unwrap()["analysis"], "found");
    // later edits, undone and redone, keep it
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.5})).unwrap();
    s.execute("skin.set", &json!({"amount": 60})).unwrap();
    for _ in 0..2 {
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(skin_of(&s, id).1.as_ref(), Some(&a));
    }
    // undoing Smooth Skin itself switches it off; redoing it brings it back with its skin
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(!skin_of(&s, id).0.active());
    s.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(skin_of(&s, id), (SkinSmoothing::SUBTLE, Some(a.clone())));
    // another person needs another analysis: the old one is kept but not used
    s.execute("skin.set", &json!({"person": 1})).unwrap();
    assert_eq!(s.execute("skin.status", &json!({})).unwrap()["analysis"], "missing");
    // and a stored analysis that doesn't fit is never put into settings
    let mut other = a.clone();
    other.area = SkinArea::Body;
    s.store_skin_analysis(id, &other).unwrap();
    assert_ne!(skin_of(&s, id).1.as_ref(), Some(&other));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn copy_paste_carries_the_sliders_never_another_photos_skin() {
    let (mut s, dir) = demo();
    let ids: Vec<PhotoId> = s.visible().to_vec();
    let (a, b) = (ids[0], ids[1]);
    s.execute("library.select", &json!({"ids": [a.0], "active": a.0})).unwrap();
    s.execute("skin.set", &json!({"amount": 40, "evenTone": 10, "area": "both"})).unwrap();
    s.store_skin_analysis(a, &SkinAnalysis { area: SkinArea::Both, ..analysis() }).unwrap();
    assert!(skin_of(&s, a).1.is_some());
    // not copied by default
    s.execute("develop.copy", &json!({})).unwrap();
    s.execute("library.select", &json!({"ids": [b.0], "active": b.0})).unwrap();
    s.execute("develop.paste", &json!({})).unwrap();
    assert!(!skin_of(&s, b).0.active());
    // copied when chosen: the sliders, not the skin
    s.execute("library.select", &json!({"ids": [a.0], "active": a.0})).unwrap();
    s.execute("develop.copy", &json!({"groups": [SettingsGroup::SmoothSkin]})).unwrap();
    s.execute("library.select", &json!({"ids": [b.0], "active": b.0})).unwrap();
    s.execute("develop.paste", &json!({})).unwrap();
    let (k, found) = skin_of(&s, b);
    assert_eq!((k.amount, k.even_tone, k.area), (40.0, 10.0, SkinArea::Both));
    assert!(found.is_none(), "photo b gets its own skin, never photo a's");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_skin_found_renders_and_rides_in_the_sidecar() {
    let (mut s, dir) = demo();
    let id = s.active().unwrap();
    let render = |s: &mut Session| s.render_job(id, 320, 320, false, true).unwrap().run().rendered.unwrap().image;
    let plain = render(&mut s);
    s.execute("skin.set", &json!({"amount": 100, "evenTone": 100, "redness": 100})).unwrap();
    assert_eq!(render(&mut s).data, plain.data, "no skin found yet: nothing changes");
    s.store_skin_analysis(id, &analysis()).unwrap();
    assert_ne!(render(&mut s).data, plain.data, "smoothed where the skin is");
    // the photo's own data travels with it in the sidecar
    let xmp = crate::sidecar::sidecar_packet(s.catalog.photo(id).unwrap(), &s.catalog);
    let back = crate::sidecar::parse_sidecar(&xmp, crate::crs::Target::Rendered).unwrap();
    let Some(crate::sidecar::DevelopPatch::Full(d)) = back.develop else { panic!("lc:settings") };
    assert_eq!(d.skin, skin_of(&s, id).0);
    assert_eq!(d.skin_analysis, Some(analysis()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn find_blemishes_without_the_models_says_which_to_download() {
    let (mut s, dir) = demo();
    if !crate::segment::Segmenter::AVAILABLE {
        assert!(s.execute("spot.findBlemishes", &json!({})).is_err());
        return;
    }
    for p in [json!({}), json!({"body": true, "sensitivity": 1e9, "maxPerFace": -4})] {
        let e = s.execute("spot.findBlemishes", &p).unwrap_err();
        // the app offers the download on the kind (#1046)
        assert!(matches!(e.kind(), crate::ErrorKind::ModelMissing(crate::AiModel::FaceDetector | crate::AiModel::People)), "{e}");
        assert!(e.to_string().contains("ai.models.download"), "{e}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The whole path on a real portrait with the real models: `LC_AI_MODELS_TEST_DIR` (a models folder holding
/// MediaPipe's people and face models) and `LC_RETOUCH_TEST_PHOTO` (a photo with a face). Skipped without them.
#[cfg(feature = "sam")]
#[test]
fn on_a_real_portrait_with_the_real_models() {
    let (Some(root), Some(photo)) = (std::env::var_os("LC_AI_MODELS_TEST_DIR"), std::env::var_os("LC_RETOUCH_TEST_PHOTO")) else {
        lcc_testkit::skip!(Model, "set LC_AI_MODELS_TEST_DIR and LC_RETOUCH_TEST_PHOTO");
        return;
    };
    let mut s = Session::new().with_fs();
    s.segmenter.models_root = Some(root.into());
    let r = s.execute("library.import", &json!({"paths": [photo.to_string_lossy()]})).unwrap();
    let id = PhotoId(r["imported"][0].as_u64().unwrap());
    s.execute("library.select", &json!({"ids": [id.0], "active": id.0})).unwrap();
    let r = s.execute("skin.smooth", &json!({})).unwrap();
    assert_eq!(r["analysis"], "found", "{r}");
    assert!(r["faces"].as_u64().unwrap() >= 1, "{r}");
    let found = s.develop_of(id).unwrap().skin_analysis.clone().unwrap();
    assert!(found.faces.iter().all(|f| f.eyes > 0.005 && f.eyes < 0.5), "{:?}", found.faces);
    let r = s.execute("spot.findBlemishes", &json!({"add": false})).unwrap();
    assert!(r["facesWithLandmarks"].as_u64().unwrap() >= 1, "{r}");
    eprintln!("blemishes: {r}");
    let r = s.execute("faces.detect", &json!({"apply": false})).unwrap();
    assert!(!r["photos"][0]["faces"].as_array().unwrap().is_empty(), "{r}");
}
