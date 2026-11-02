//! Smooth Skin settings: their JSON, the copy group and the stored analysis.

use serde_json::json;

use crate::{DevelopSettings, SegMask, SettingsGroup, SkinAnalysis, SkinArea, SkinFace, SkinSmoothing, apply_partial, controls, extract_groups};
use lcc_geom::Orientation;

fn analysis() -> SkinAnalysis {
    SkinAnalysis {
        area: SkinArea::Face,
        person: 0,
        orientation: Orientation::Normal,
        seg: SegMask::from_logits(2, &[4.0, -4.0, 4.0, -4.0]),
        faces: vec![SkinFace { x: 0.5, y: 0.4, eyes: 0.08 }],
    }
}

#[test]
fn off_is_not_written_and_keeps_the_hash() {
    let s = DevelopSettings::default();
    let v = s.to_json();
    assert!(v.get("skin").is_none() && v.get("skin_analysis").is_none());
    assert!(!s.skin.active());
    let mut on = s.clone();
    assert!(controls::set(&mut on, "skin.amount", 40.0));
    assert!(on.skin.active());
    assert_ne!(on.hash64(), s.hash64());
    assert_eq!(on.to_json()["skin"]["amount"], json!(40.0));
}

#[test]
fn settings_and_analysis_round_trip() {
    let mut s = DevelopSettings { skin: SkinSmoothing { area: SkinArea::Both, person: 2, ..SkinSmoothing::SUBTLE }, ..Default::default() };
    s.skin_analysis = Some(analysis());
    let back: DevelopSettings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    assert_eq!(back, s);
    assert_eq!(back.to_json()["skin"]["area"], json!("both"));
}

#[test]
fn the_controls_clamp_their_ranges() {
    let mut s = DevelopSettings::default();
    for (id, v, want) in
        [("skin.amount", 150.0, 100.0), ("skin.texture", -300.0, -100.0), ("skin.evenTone", -5.0, 0.0), ("skin.redness", f64::NAN, 0.0)]
    {
        assert!(controls::set(&mut s, id, v), "{id}");
        assert_eq!(controls::get(&s, id), Some(want), "{id}");
    }
    assert!(controls::in_section(crate::Section::Skin).count() == 4);
}

#[test]
fn copying_smooth_skin_never_carries_the_photos_own_analysis() {
    let mut a = DevelopSettings { skin: SkinSmoothing::SUBTLE, ..Default::default() };
    a.skin_analysis = Some(analysis());
    let copied = extract_groups(&a, &SettingsGroup::ALL);
    assert!(copied.get("skin_analysis").is_none(), "no group holds the analysis");
    assert_eq!(copied["skin"]["amount"], json!(35.0));
    // the target keeps its own analysis (or none) and gets the sliders
    let target = DevelopSettings::default();
    let pasted = apply_partial(&target, &copied, 1.0);
    assert_eq!(pasted.skin, SkinSmoothing::SUBTLE);
    assert!(pasted.skin_analysis.is_none());
    // copying it off switches it off where it lands
    let off = extract_groups(&DevelopSettings::default(), &[SettingsGroup::SmoothSkin]);
    let cleared = apply_partial(&pasted, &off, 1.0);
    assert!(!cleared.skin.active());
    // not copied by default (a local retouch, like spots)
    assert!(!SettingsGroup::default_copy().contains(&SettingsGroup::SmoothSkin));
}

#[test]
fn an_analysis_belongs_to_its_area_person_and_orientation() {
    let a = analysis();
    let s = SkinSmoothing::SUBTLE;
    assert!(a.is_for(&s, Orientation::Normal));
    assert!(!a.is_for(&s, Orientation::Rotate90));
    assert!(!a.is_for(&SkinSmoothing { person: 1, ..s }, Orientation::Normal));
    assert!(!a.is_for(&SkinSmoothing { area: SkinArea::Body, ..s }, Orientation::Normal));
}

#[test]
fn hostile_analyses_load_capped_or_not_at_all() {
    let face = json!({"x": 0.5, "y": 0.5, "eyes": 0.1});
    let doc = json!({"skin_analysis": {"area": "face", "person": 0, "orientation": "Normal",
        "seg": {"side": 2, "data": "AA=="}, "faces": vec![face; 10_000]}});
    let s: DevelopSettings = serde_json::from_value(doc).unwrap();
    assert_eq!(s.skin_analysis.unwrap().faces.len(), crate::MAX_SKIN_FACES);
    // a damaged analysis costs the analysis, not the photo's settings
    let bad = json!({"light": {"exposure": 1.0}, "skin": {"area": "elbows", "amount": 50.0}, "skin_analysis": {"seg": 3}});
    let s: DevelopSettings = serde_json::from_value(bad).unwrap();
    assert_eq!((s.light.exposure, s.skin.active(), s.skin_analysis.is_none()), (1.0, false, true));
    assert_eq!(SkinArea::parse("Body Skin"), Some(SkinArea::Body));
    assert_eq!(SkinArea::parse("knees"), None);
}
