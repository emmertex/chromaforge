//! A camera profile and a look are two layers (ported from OneSlips, LR-PROF-LAYERS): a camera profile with a look of
//! its own (tone curve, look table) develops with that look under the default look ("As profile"); any other look
//! sits on its calibration only, so two looks are never stacked. Profiles are built in code (none is ever a fixture).

use lcc_catalog::{MediaKind, Op, Photo, PhotoId, Source};
use serde_json::json;

use crate::Session;
use crate::media::LoadOptions;

const MAKE: &str = "TESTMAKE";
const MODEL: &str = "LAYERS1";
const WITH_LOOK: &str = "dcp:layers-test-with-look";
const PLAIN: &str = "dcp:layers-test-plain";

fn profile(name: &str, look: bool) -> lcc_raw::dcp::Dcp {
    let m = lcc_raw::Mat3([[0.7, -0.1, -0.05], [-0.5, 1.3, 0.2], [-0.1, 0.25, 0.65]]);
    let mut color = lcc_raw::ColorData { illuminant: [21, 0], color_matrix: [Some(m), None], ..Default::default() };
    if look {
        color.profile.tone_curve = lcc_raw::profile::ToneCurve::from_tag(&[0.0, 0.0, 0.25, 0.42, 1.0, 1.0]);
    }
    lcc_raw::dcp::Dcp { model: format!("{MAKE} {MODEL}"), name: name.into(), copyright: String::new(), color }
}

/// The two test profiles in the decoder's list of imported ones (beside whatever other tests put there).
fn install() -> std::sync::MutexGuard<'static, ()> {
    let lock = crate::camera_profiles::test_library_lock();
    let mut list: Vec<(String, lcc_raw::dcp::Dcp)> =
        lcc_raw::dcp::library().into_iter().filter(|(k, _)| k != WITH_LOOK && k != PLAIN).map(|(k, d)| (k, (*d).clone())).collect();
    list.push((WITH_LOOK.into(), profile("With Look", true)));
    list.push((PLAIN.into(), profile("Plain", false)));
    lcc_raw::dcp::install_library(list);
    lock
}

/// A tiny RGGB DNG of the test camera.
fn dng() -> Vec<u8> {
    let (w, h) = (64usize, 48usize);
    let mut raw = lcc_raw::RawImage {
        format: lcc_raw::RawFormat::Dng,
        width: w,
        height: h,
        cpp: 1,
        data: lcc_raw::RawData::U16((0..w * h).map(|i| 1000 + (i % 97) as u16 * 40).collect()),
        cfa: lcc_raw::Cfa::bayer("RGGB"),
        bits: 14,
        black: lcc_raw::BlackLevel::uniform(512.0),
        white: vec![16000.0],
        active_area: lcc_raw::Rect::new(0, 0, w, h),
        crop: lcc_raw::Rect::new(0, 0, w, h),
        orientation: lcc_raw::Orientation::Normal,
        color: lcc_raw::ColorData {
            illuminant: [21, 0],
            color_matrix: [Some(lcc_raw::Mat3([[0.8, -0.2, -0.05], [-0.45, 1.3, 0.15], [-0.08, 0.22, 0.62]])), None],
            as_shot_neutral: Some([0.5, 1.0, 0.7]),
            ..Default::default()
        },
        wb_multipliers: None,
        linearized: false,
        pixel_aspect: 1.0,
        opcodes: Default::default(),
        metadata: Default::default(),
        lens_metadata: Default::default(),
    };
    raw.metadata.make = Some(MAKE.into());
    raw.metadata.model = Some(MODEL.into());
    lcc_raw::write_dng(&raw, &Default::default()).unwrap()
}

fn raw_photo(s: &mut Session) -> PhotoId {
    let id = s.catalog.alloc_photo_id();
    let mut p = Photo::new(id, Source::File { path: "/nonexistent/LAYERS.DNG".into() }, "LAYERS.DNG", "DNG", 64, 48, "2026-01-01");
    p.kind = MediaKind::Raw;
    p.meta.camera = format!("{MAKE} {MODEL}");
    s.commit("Add", Op::AddPhoto { photo: Box::new(p) }).unwrap();
    s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
    id
}

#[test]
fn a_look_over_a_camera_profile_with_its_own_look_decodes_with_its_calibration_only() {
    let _lock = install();
    let bytes = dng();
    let decode = |look: bool| {
        let options = LoadOptions { camera: WITH_LOOK.into(), camera_look: look, ..Default::default() };
        crate::files::load_bytes_with(&bytes, 64, options).unwrap().1
    };
    assert!(decode(true).camera_tone.is_some(), "As profile: the profile's own tone curve");
    assert!(decode(false).camera_tone.is_none(), "under another look: the calibration only, no tone curve stacked under it");
    assert!(profile("x", true).has_look() && !profile("x", false).has_look());
    let mut identity = profile("x", false);
    identity.color.profile.tone_curve = lcc_raw::profile::ToneCurve::from_tag(&[0.0, 0.0, 1.0, 1.0]);
    assert!(!identity.has_look(), "an identity tone curve is no look");
}

#[test]
fn choosing_a_camera_profile_with_a_look_is_as_profile_and_a_look_keeps_its_calibration() {
    let _lock = install();
    let mut s = Session::new();
    let id = raw_photo(&mut s);
    s.execute("develop.profile", &json!({"id": "lc.vivid"})).unwrap();
    // a camera profile with a look of its own comes with it: the look goes back to the default
    s.execute("develop.profile", &json!({"id": WITH_LOOK})).unwrap();
    let p = s.catalog.photo(id).unwrap().clone();
    assert_eq!((p.develop.profile.camera.as_str(), p.develop.profile.id.as_str()), (WITH_LOOK, lcc_develop::DEFAULT_LOOK));
    assert!(crate::camera_profiles::as_profile(&p));
    assert!(LoadOptions::for_photo(&p).camera_look, "decoded with the profile's look");
    // a look of the app's keeps the camera profile as the calibration, without its look
    s.execute("develop.profile", &json!({"id": "lc.vivid"})).unwrap();
    let p = s.catalog.photo(id).unwrap().clone();
    assert_eq!((p.develop.profile.camera.as_str(), p.develop.profile.id.as_str()), (WITH_LOOK, "lc.vivid"));
    assert!(!crate::camera_profiles::as_profile(&p));
    assert!(!LoadOptions::for_photo(&p).camera_look, "the calibration only");
    // a camera profile without a look keeps the look, and a change of look never decodes it again
    s.execute("develop.profile", &json!({"id": PLAIN})).unwrap();
    let p = s.catalog.photo(id).unwrap().clone();
    assert_eq!((p.develop.profile.camera.as_str(), p.develop.profile.id.as_str()), (PLAIN, "lc.vivid"));
    assert!(LoadOptions::for_photo(&p).camera_look);
    assert!(s.execute("develop.profile", &json!({"id": "dcp:no-such-profile"})).is_err());
}

#[test]
fn settings_from_before_the_two_layers_keep_their_look_on_the_default() {
    let _lock = install();
    // an old edit: the camera profile chosen with develop.cameraProfile, the look left at the default
    let mut s = Session::new();
    let id = raw_photo(&mut s);
    s.execute("develop.cameraProfile", &json!({"id": WITH_LOOK})).unwrap();
    let p = s.catalog.photo(id).unwrap().clone();
    assert_eq!(p.develop.profile.id, lcc_develop::DEFAULT_LOOK);
    assert!(crate::camera_profiles::as_profile(&p), "renders as it did: with the profile's own look");
    assert!(LoadOptions::for_photo(&p).camera_look);
}
