//! Portrait retouching: Find Blemishes (`spot.findBlemishes`) and Smooth Skin (`skin.*`). Ported from OneSlips.
//!
//! Both work from the skin and faces the open MediaPipe models find ([`crate::segment::SkinFound`]):
//! Find Blemishes adds a heal spot on each small dark or red spot of the faces' skin (and of the
//! body's, when asked), Smooth Skin smooths that skin in the develop pipeline (`lcc_pipeline::skin`).
//! Heal spots are applied before Smooth Skin, so it smooths the healed skin. In the desktop app the
//! models run in the background; elsewhere the commands wait.

use lcc_develop::{DevelopSettings, SkinArea, SkinSmoothing, Spot, SpotMode};
use lcc_geom::Point;
use lcc_pipeline::blemish::{self, FaceBox, Options};
use serde_json::{Value, json};

use super::masks::{SPOT_SIZE, pick_source};
use super::{CommandSpec, bad, bool_or, cmd, f64_or, has_active, str_param};
use crate::segment::{SkinFound, SkinKey, SkinPurpose};
use crate::{EngineError, PhotoId, Result, Session};

/// Long edge of the picture blemishes are looked for in.
const LOOK_EDGE: usize = 1600;

/// The face boxes of `found` for the detector.
fn boxes(found: &SkinFound) -> Vec<FaceBox> {
    let p = |q: [f32; 2]| (f64::from(q[0]), f64::from(q[1]));
    found
        .faces
        .iter()
        .map(|f| FaceBox {
            x0: f64::from(f.rect[0]),
            y0: f64::from(f.rect[1]),
            x1: f64::from(f.rect[2]),
            y1: f64::from(f.rect[3]),
            keypoints: [p(f.keypoints[0]), p(f.keypoints[1]), p(f.keypoints[2]), p(f.keypoints[3])],
            mesh: f.mesh.as_ref().map(|m| m.iter().map(|q| p(*q)).collect()),
        })
        .collect()
}

/// Settings `d` without Smooth Skin (what blemishes are looked for on: the skin as it is).
fn unsmoothed(d: &DevelopSettings) -> DevelopSettings {
    DevelopSettings { skin: SkinSmoothing::default(), skin_analysis: None, ..d.clone() }
}

/// A model error keeps its kind (a frontend offers the download); others name the command.
fn wrap(c: &str, e: EngineError) -> EngineError {
    match e {
        EngineError::ModelMissing { model, msg } => EngineError::ModelMissing { model, msg: bad(c, msg).to_string() },
        e => bad(c, e.to_string()),
    }
}

/// Find blemishes on photo `id` with the skin and faces `found` and, unless `add` is false, add a
/// heal spot on each (one undo step).
pub(crate) fn add_blemishes_with(s: &mut Session, id: PhotoId, found: &SkinFound, opts: &Options, add: bool) -> Result<Value> {
    let c = "spot.findBlemishes";
    let d = s.develop_of(id).ok_or_else(|| bad(c, "no such photo"))?;
    // the uncropped photo, so positions are the spots' own coordinates
    let job = s.preview_job(id, LOOK_EDGE, LOOK_EDGE, false, &unsmoothed(&d)).ok_or_else(|| bad(c, "no photo"))?;
    let img = job.run().rendered.map_err(|e| bad(c, e))?.image;
    let skin = found.seg.as_ref().and_then(|seg| blemish::skin_plane(seg, img.width, img.height));
    let faces = boxes(found);
    let found_spots = blemish::detect(&img, &faces, skin.as_ref(), opts);
    let out: Vec<Value> = found_spots.iter().map(|b| json!({"x": b.x, "y": b.y, "size": b.radius, "face": b.face, "body": b.body})).collect();
    let with_landmarks = faces.iter().filter(|f| f.mesh.is_some()).count();
    let summary = |added: usize| json!({"faces": faces.len(), "facesWithLandmarks": with_landmarks, "spots": out, "added": added, "detector": "MediaPipe Face Landmarker"});
    if !add || found_spots.is_empty() {
        return Ok(summary(0));
    }
    let mut dd = (*d).clone();
    let base = Spot::default();
    for b in &found_spots {
        let mut spot = Spot {
            mode: SpotMode::Heal,
            points: vec![Point::new(b.x, b.y)],
            size: b.radius.clamp(SPOT_SIZE.0, SPOT_SIZE.1),
            feather: base.feather,
            opacity: 100.0,
            source_offset: None,
            generated: None,
            auto: None,
        };
        spot.source_offset = pick_source(s, id, &dd, &spot, None);
        dd.spots.push(spot);
    }
    s.set_develop(id, dd, "Find Blemishes")?;
    Ok(summary(found_spots.len()))
}

/// [`add_blemishes_with`], adding them (a background request's result).
#[cfg_attr(not(feature = "sam"), allow(dead_code))]
pub(crate) fn add_blemishes(s: &mut Session, id: PhotoId, found: &SkinFound, opts: &Options) -> Result<Value> {
    add_blemishes_with(s, id, found, opts, true)
}

/// What to tell the user about a Find Blemishes result.
#[cfg_attr(not(feature = "sam"), allow(dead_code))]
pub(crate) fn blemishes_message(v: &Value) -> String {
    let added = v["added"].as_u64().unwrap_or(0);
    let faces = v["faces"].as_u64().unwrap_or(0);
    let marked = v["facesWithLandmarks"].as_u64().unwrap_or(0);
    match (faces, marked, added) {
        (0, _, _) => "Find Blemishes: no faces were found in this photo.".into(),
        (_, 0, _) => "Find Blemishes: the faces are in profile or too small to place their features, so they were left alone.".into(),
        (_, _, 0) => "Find Blemishes: no blemishes were found.".into(),
        (_, _, 1) => "Find Blemishes: found 1 blemish and added a heal spot on it.".into(),
        (_, _, n) => format!("Find Blemishes: found {n} blemishes and added a heal spot on each."),
    }
}

fn active(s: &Session, c: &str) -> Result<PhotoId> {
    s.active().ok_or_else(|| bad(c, "no active photo"))
}

/// Where Smooth Skin stands on photo `id`, for `skin.status` and the commands' replies.
fn status(s: &Session, id: PhotoId) -> Value {
    let Some(d) = s.develop_of(id) else { return Value::Null };
    let a = d.skin_analysis.as_ref().filter(|a| a.is_for(&d.skin, d.orientation));
    let skin = |a: &lcc_develop::SkinAnalysis| a.seg.logits().is_some_and(|l| l.iter().any(|v| *v > 0.0));
    let analysis = match (d.skin.active(), a) {
        (false, _) => "off",
        (true, Some(a)) if skin(a) => "found",
        (true, Some(_)) => "noSkin",
        (true, None) if s.skin_pending(id) => "looking",
        (true, None) => "missing",
    };
    json!({
        "settings": serde_json::to_value(d.skin).unwrap_or(Value::Null),
        "active": d.skin.active(),
        "analysis": analysis,
        "faces": a.map_or(0, |a| a.faces.len()),
        "sectionOn": d.retouch_section_on("skin"),
    })
}

/// Apply `f` to the active photo's Smooth Skin (one undo step; the commit finds the skin it wants).
/// What kept the skin from being found (a model to download) is the reply's `error`.
fn edit(s: &mut Session, c: &str, label: &str, f: impl FnOnce(&mut SkinSmoothing) -> Result<()>) -> Result<Value> {
    let id = active(s, c)?;
    let mut d = (*s.develop_of(id).ok_or_else(|| bad(c, "no such photo"))?).clone();
    f(&mut d.skin)?;
    s.set_develop(id, d, label)?;
    let mut v = status(s, id);
    if let Err(e) = s.skin_ensure(id, false) {
        v["error"] = json!(e.to_string());
    }
    Ok(v)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "spot.findBlemishes",
            "Find Blemishes",
            [],
            None,
            "{sensitivity?: 0..100 (50), add?: bool (true), maxPerFace?: 1..20 (6), body?: bool (false: faces only; true: also shoulders, arms, back)} — find small blemishes (darker or redder round spots) on the skin of the active photo's faces and add a heal spot on each (one undo step). Needs the open MediaPipe people and face models (`ai.models.download {id: \"people\" | \"face\"}`). Eyes, brows, nostrils and lips are cut out by the face's landmarks; faces in profile (no landmarks), faces narrower than 160 px at 1600 px, and faces with more than 14 candidates (stubble, freckles, pores) get none. In the desktop app it runs in the background and returns {queued: true}; otherwise → {faces, facesWithLandmarks, spots: [{x, y, size, face, body}], added}",
            has_active,
            |s, p| {
                let c = "spot.findBlemishes";
                let id = active(s, c)?;
                let opts = Options {
                    sensitivity: f64_or(p, "sensitivity", 50.0).clamp(0.0, 100.0) as f32,
                    max_per_face: (f64_or(p, "maxPerFace", blemish::MAX_PER_FACE as f64).clamp(1.0, 20.0)) as usize,
                    body: bool_or(p, "body", false),
                };
                let add = bool_or(p, "add", true);
                let area = if opts.body { SkinArea::Both } else { SkinArea::Face };
                if s.segmenter.background && add {
                    let orientation = s.develop_of(id).map(|d| d.orientation).unwrap_or_default();
                    let key = SkinKey { photo: id, area, person: 0, orientation };
                    s.segment_skin_later(key, SkinPurpose::Blemishes(opts)).map_err(|e| wrap(c, e))?;
                    return Ok(json!({"queued": true}));
                }
                let found = s.segment_skin(id, area, 0).map_err(|e| wrap(c, e))?;
                add_blemishes_with(s, id, &found, &opts, add)
            }
        ),
        cmd!(
            "skin.smooth",
            "Smooth Skin",
            [],
            None,
            "{off?: bool} — switch Smooth Skin on for the active photo with subtle settings (Amount 35, Texture 0, Even Tone 25, Redness 20; the area and person it had are kept), or off with `off: true` (one undo step), and find the skin with the open MediaPipe models (in the background in the desktop app) → skin.status",
            has_active,
            |s, p| {
                let off = bool_or(p, "off", false);
                edit(s, "skin.smooth", "Smooth Skin", |k| {
                    *k = if off {
                        SkinSmoothing { area: k.area, person: k.person, ..SkinSmoothing::default() }
                    } else {
                        SkinSmoothing { area: k.area, person: k.person, ..SkinSmoothing::SUBTLE }
                    };
                    Ok(())
                })
            }
        ),
        cmd!(
            "skin.set",
            "Set Smooth Skin",
            [],
            None,
            "{amount?: 0..100, texture?: -100..100, evenTone?: 0..100, redness?: 0..100, shine?: 0..100 (Reduce Shine), area?: face|body|both, person?: 0 (everyone) | 1..4 (counted from the left by their face)} — change the active photo's Smooth Skin (one undo step); the sliders are also develop controls (`skin.amount`, `skin.texture`, `skin.evenTone`, `skin.redness`, `skin.shine`). A new area or person finds the skin again → skin.status",
            has_active,
            |s, p| {
                let c = "skin.set";
                let area = match str_param(p, "area") {
                    Some(a) => Some(SkinArea::parse(a).ok_or_else(|| bad(c, format!("unknown area `{a}` (face, body or both)")))?),
                    None => None,
                };
                let person = match p.get("person") {
                    Some(v) => {
                        let n = v
                            .as_u64()
                            .filter(|n| *n <= u64::from(SkinSmoothing::MAX_PERSON))
                            .ok_or_else(|| bad(c, "`person` is 0 (everyone) or 1 to 4"))?;
                        Some(n as u32)
                    }
                    None => None,
                };
                edit(s, c, "Smooth Skin", |k| {
                    let set = |v: &mut f64, key: &str, lo: f64| {
                        if let Some(x) = p.get(key).and_then(Value::as_f64).filter(|x| x.is_finite()) {
                            *v = x.clamp(lo, 100.0);
                        }
                    };
                    set(&mut k.amount, "amount", 0.0);
                    set(&mut k.texture, "texture", -100.0);
                    set(&mut k.even_tone, "evenTone", 0.0);
                    set(&mut k.redness, "redness", 0.0);
                    set(&mut k.shine, "shine", 0.0);
                    if let Some(a) = area {
                        k.area = a;
                    }
                    if let Some(n) = person {
                        k.person = n;
                    }
                    Ok(())
                })
            }
        ),
        cmd!(
            "skin.find",
            "Find Skin",
            [],
            None,
            "{} — look for the skin Smooth Skin works on again, on the active photo (after installing the models, or when the first try failed); needs Smooth Skin on. In the desktop app it runs in the background → skin.status",
            has_active,
            |s, _| {
                let c = "skin.find";
                let id = active(s, c)?;
                let d = s.develop_of(id).ok_or_else(|| bad(c, "no such photo"))?;
                if !d.skin.active() {
                    return Err(bad(c, "Smooth Skin is off on this photo (switch it on with skin.smooth)"));
                }
                s.skin_models().map_err(|e| wrap(c, e))?;
                if d.skin_analysis.is_some() {
                    // forget what was found, without an undo step: the next analysis replaces it
                    let mut dd = (*d).clone();
                    dd.skin_analysis = None;
                    let edited = s.catalog.photo(id).and_then(|ph| ph.edited.clone());
                    let op = lcc_catalog::Op::SetDevelop { id, settings: std::sync::Arc::new(dd), label: "Find Skin".into(), edited };
                    s.apply_unrecorded(op)?;
                }
                let error = s.skin_ensure(id, true).err();
                let mut v = status(s, id);
                if let Some(e) = error {
                    v["error"] = json!(e.to_string());
                }
                Ok(v)
            }
        ),
        cmd!(
            query "skin.status",
            "Smooth Skin Status",
            [],
            None,
            "{} → {settings: {amount, texture, even_tone, redness, shine, area, person}, active, analysis: off|looking|found|noSkin|missing, faces, sectionOn} for the active photo",
            has_active,
            |s, _| {
                let id = active(s, "skin.status")?;
                Ok(status(s, id))
            }
        ),
    ]
}
