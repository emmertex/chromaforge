//! A preset changes only the settings it names, and its amount is measured from the photo's import
//! look when a default preset recorded one, otherwise from the photo's current values (adapted from
//! upstream #2207).

use serde_json::json;

use crate::Session;

/// A photo of `s`'s demo library given an import look (`look` edited by `edit`), as a default
/// preset at import records it; its settings start as that look.
fn with_import_look(s: &mut Session, edit: impl FnOnce(&mut lcc_develop::DevelopSettings)) -> lcc_catalog::PhotoId {
    let id = s.active().unwrap();
    let mut p = (**s.catalog.photo(id).unwrap()).clone();
    let mut look = p.camera_defaults();
    edit(&mut look);
    p.import_look = Some(std::sync::Arc::new(look.clone()));
    p.develop = std::sync::Arc::new(look);
    s.catalog.apply(lcc_catalog::Op::RemovePhoto { id: p.id }).unwrap();
    s.catalog.apply(lcc_catalog::Op::AddPhoto { photo: Box::new(p) }).unwrap();
    id
}

/// A colour preset after a B&W one brings the colours back. The treatment and the settings the
/// colour preset names take its values; a setting it does not name keeps the value the B&W preset
/// gave it.
#[test]
fn a_color_preset_after_a_bw_one_returns_the_colors() {
    let mut s = Session::with_demo();
    s.execute("preset.apply", &json!({"id": "lc.bw-high-contrast"})).unwrap();
    let dv = s.develop_of(s.active().unwrap()).unwrap();
    assert_eq!(dv.treatment, lcc_develop::Treatment::Bw);
    s.execute("preset.apply", &json!({"id": "lc.warm-glow"})).unwrap();
    let dv = s.develop_of(s.active().unwrap()).unwrap();
    assert_eq!(dv.treatment, lcc_develop::Treatment::Color, "the colour preset restores the treatment");
    assert_eq!(dv.light.contrast, 8.0, "the colour preset's values, not the B&W preset's");
    assert_eq!(dv.light.whites, 20.0, "a setting the colour preset doesn't name keeps the B&W preset's value");
}

/// Every built-in preset that styles a photo (not a mask-only one) names its treatment, so none
/// leaves a photo black and white or in colour because an earlier preset made it so.
#[test]
fn every_builtin_look_names_its_treatment() {
    for p in crate::presets::builtin().into_iter().filter(|p| !p.mask_only()) {
        assert!(p.settings.get("treatment").is_some_and(|t| t.is_string()), "{} names no treatment", p.id);
    }
}

/// A manual edit to a setting a preset does not name survives it, though the preset styles the
/// same group (Bright & Airy names exposure, contrast, highlights, shadows and whites).
#[test]
fn a_preset_keeps_a_setting_it_does_not_name_in_the_group_it_styles() {
    let mut s = Session::with_demo();
    s.execute("develop.set", &json!({"control": "light.blacks", "value": -30.0})).unwrap();
    s.execute("preset.apply", &json!({"id": "lc.bright-airy"})).unwrap();
    let dv = s.develop_of(s.active().unwrap()).unwrap();
    assert_eq!(dv.light.blacks, -30.0, "blacks is not named by the preset");
    assert_eq!(dv.light.exposure, 0.45, "exposure is named by the preset and takes its value");
}

/// Colours a partial preset does not name keep the photo's values. Deep Sky names only the blue
/// and aqua mixer colours, and only their luminance and saturation.
#[test]
fn a_preset_keeps_the_mixer_colours_it_does_not_name() {
    let mut s = Session::with_demo();
    s.execute("develop.set", &json!({"control": "mixer.red.sat", "value": 20.0})).unwrap();
    s.execute("develop.set", &json!({"control": "mixer.blue.hue", "value": 15.0})).unwrap();
    s.execute("preset.apply", &json!({"id": "lc.deep-sky"})).unwrap();
    let dv = s.develop_of(s.active().unwrap()).unwrap();
    assert_eq!(dv.mixer.red.sat, 20.0, "a colour the preset doesn't name");
    assert_eq!(dv.mixer.blue.hue, 15.0, "a setting of a named colour that the preset doesn't name");
    assert_eq!(dv.mixer.blue.lum, -25.0, "a setting the preset names takes its value");
}

/// Settings that came from an XMP sidecar or a catalog import record no import look, so the camera
/// defaults are not their original: a preset that names only some of their group keeps the rest,
/// at full amount and below it.
#[test]
fn xmp_imported_exposure_whites_and_blacks_survive_crisp_landscape() {
    for amount in [100, 50] {
        let mut s = Session::with_demo();
        s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.7})).unwrap();
        s.execute("develop.set", &json!({"control": "light.whites", "value": 10.0})).unwrap();
        s.execute("develop.set", &json!({"control": "light.blacks", "value": -20.0})).unwrap();
        let id = s.active().unwrap();
        assert!(s.catalog.photo(id).unwrap().import_look.is_none());
        s.execute("preset.apply", &json!({"id": "lc.crisp-landscape", "amount": amount})).unwrap();
        let dv = s.develop_of(id).unwrap();
        assert_eq!((dv.light.exposure, dv.light.whites, dv.light.blacks), (0.7, 10.0, -20.0), "amount {amount}");
        assert_eq!(dv.light.contrast, 12.0 * f64::from(amount) / 100.0, "amount {amount}: a setting the preset names takes its value");
    }
}

/// Amount 0 applies nothing to a photo with no import look: the photo keeps every value, also the
/// exposure that Bright & Airy names (a camera-default original would reset it to 0).
#[test]
fn amount_zero_applies_nothing_to_a_photo_without_an_import_look() {
    let mut s = Session::with_demo();
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 2.0})).unwrap();
    let id = s.active().unwrap();
    let before = s.develop_of(id).unwrap();
    s.execute("preset.apply", &json!({"id": "lc.bright-airy", "amount": 0})).unwrap();
    assert_eq!(s.develop_of(id).unwrap(), before);
}

/// With no import look, a preset's amount moves the named settings from the photo's current
/// values: 50 % of Bright & Airy's exposure (0.45) from 2.0 is 1.225.
#[test]
fn a_preset_amount_without_an_import_look_is_measured_from_the_current_settings() {
    let mut s = Session::with_demo();
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 2.0})).unwrap();
    s.execute("preset.apply", &json!({"id": "lc.bright-airy", "amount": 50})).unwrap();
    let dv = s.develop_of(s.active().unwrap()).unwrap();
    assert!((dv.light.exposure - 1.225).abs() < 1e-9, "{}", dv.light.exposure);
}

/// A photo whose default preset recorded an import look measures a preset's amount from that look:
/// 50 % is halfway from the look to the preset whatever the photo was edited to, and 0 % gives the
/// look's value for the settings the preset names.
#[test]
fn a_preset_amount_is_measured_from_the_import_look_when_there_is_one() {
    let mut s = Session::with_demo();
    let id = with_import_look(&mut s, |look| {
        look.light.exposure = 0.5;
        look.light.blacks = -10.0;
    });
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 2.0})).unwrap();
    s.execute("develop.set", &json!({"control": "light.blacks", "value": -30.0})).unwrap();
    s.execute("preset.apply", &json!({"id": "lc.bright-airy", "amount": 50})).unwrap();
    let dv = s.develop_of(id).unwrap();
    assert!((dv.light.exposure - 0.475).abs() < 1e-9, "{}", dv.light.exposure);
    assert_eq!(dv.light.blacks, -30.0, "a setting the preset doesn't name keeps the photo's edit");
    s.execute("preset.apply", &json!({"id": "lc.bright-airy", "amount": 0})).unwrap();
    let dv = s.develop_of(id).unwrap();
    assert_eq!(dv.light.exposure, 0.5, "amount 0 gives the import look's value for a setting the preset names");
    assert_eq!(dv.light.blacks, -30.0, "and keeps the photo's edit where the preset names nothing");
}

/// A photo whose import look records a free crop aspect (null) but whose edits locked one gets the
/// import value back at 0 % from a preset that names the aspect.
#[test]
fn amount_zero_restores_a_null_import_aspect() {
    let mut s = Session::with_demo();
    let id = with_import_look(&mut s, |_| {});
    s.execute("crop.aspect", &json!({"aspect": [3, 2]})).unwrap();
    assert!(s.develop_of(id).unwrap().crop.aspect.is_some(), "the aspect is locked for this test");
    let added = s.add_presets(vec![lcc_develop::Preset {
        id: "user.free-aspect".into(),
        name: "Free aspect".into(),
        group: "Test".into(),
        settings: json!({"crop": {"aspect": null}}),
        favorite: false,
        builtin: false,
    }]);
    s.execute("preset.apply", &json!({"id": added[0], "amount": 0})).unwrap();
    assert_eq!(s.develop_of(id).unwrap().crop.aspect, None, "amount 0 gives the import look's free aspect back");
}

/// The Presets panel's previews and the `preset.apply` command share one composition, so hovering
/// shows exactly what a click commits, below full amount on a photo with an import look too.
#[test]
fn the_preview_is_what_preset_apply_commits() {
    let mut s = Session::with_demo();
    let id = with_import_look(&mut s, |look| look.light.exposure = 0.5);
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 2.0})).unwrap();
    let preset = s.presets.iter().find(|p| p.id == "lc.bright-airy").cloned().unwrap();
    for amount in [100, 50] {
        let preview = s.preset_applied(id, &preset, f64::from(amount) / 100.0).unwrap();
        s.execute("preset.apply", &json!({"id": "lc.bright-airy", "amount": amount})).unwrap();
        assert_eq!(*s.develop_of(id).unwrap(), preview, "amount {amount}");
        s.execute("edit.undo", &json!({})).unwrap();
    }
}
