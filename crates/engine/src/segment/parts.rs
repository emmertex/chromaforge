//! The parts of people the Retouch sections work on (Teeth so far), from the open MediaPipe models
//! (Selfie Multiclass and Face Landmarker, as for Smooth Skin). Ported from OneSlips, fitted to
//! the Smooth Skin design ([`super::skin`]) rather than the AI mask worker.
//!
//! What was found is kept in the photo's settings ([`RetouchAnalysis`]), so rendering and exporting
//! never need a model. Like Smooth Skin's skin it is the photo's own data: in no settings group,
//! stored without an undo step. After every committed edit, undo and redo, the photos whose retouch
//! sections want a part they don't hold (a section switched on, pasted, another person, the photo
//! turned) get it ([`Session::retouch_after`]): on a background thread with a row in the activity
//! stack in the desktop app, at once in the CLI, MCP and tests. Results are kept for the session
//! per photo, parts, orientation and look, so undoing and redoing never runs the models again. A
//! missing model is a [`crate::EngineError::ModelMissing`] the frontends answer with the AI Models
//! dialog; nothing is downloaded on its own.

use std::sync::Arc;

use lcc_catalog::{Op, PhotoId};
use lcc_develop::{DevelopSettings, PartRequest, RetouchAnalysis, RetouchMask};
use lcc_geom::Orientation;

use crate::Session;

/// Requests kept for the session.
#[cfg_attr(not(feature = "sam"), allow(dead_code))]
const CACHE: usize = 16;

/// One request: which photo, which parts, the photo turned how.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RetouchKey {
    pub photo: PhotoId,
    pub parts: Vec<PartRequest>,
    pub orientation: Orientation,
}

/// A finished background request.
#[cfg_attr(not(feature = "sam"), allow(dead_code))]
pub(crate) struct RetouchDone {
    key: RetouchKey,
    look: u64,
    result: Result<Vec<RetouchMask>, String>,
}

/// The session's retouch requests: queued, failed (asked again only on request), kept results.
#[derive(Default)]
pub(crate) struct RetouchJobs {
    pub pending: Vec<RetouchKey>,
    pub failed: Vec<RetouchKey>,
    cache: Vec<(RetouchKey, u64, Vec<RetouchMask>)>,
    #[cfg_attr(not(feature = "sam"), allow(dead_code))]
    results: Option<(std::sync::mpsc::Sender<RetouchDone>, std::sync::mpsc::Receiver<RetouchDone>)>,
}

/// The parts of `d`'s retouch sections that are on (their eye and Retouch's too).
fn wanted(d: &DevelopSettings) -> Vec<PartRequest> {
    if !d.section_enabled("retouch") {
        return Vec::new();
    }
    d.retouch.wanted(|k| d.retouch_section_on(k))
}

/// Each part of `parts` on an 8-bit RGB picture `w × h` (`seg: None` where it isn't there: no teeth
/// showing, no such person).
#[cfg(feature = "sam")]
fn analyse(
    rgb: &[u8],
    w: usize,
    h: usize,
    parts: &[PartRequest],
    people: &std::path::Path,
    face: &std::path::Path,
    scene: Option<&std::path::Path>,
) -> Result<Vec<RetouchMask>, String> {
    use lcc_segment::people::{PeopleRequest, people_mask_with};
    let pm = super::skin::loaded::people(people)?;
    let fm = super::skin::loaded::face(face)?;
    let map = pm.segment(rgb, w, h).map_err(|e| e.to_string())?;
    let faces = fm.faces(rgb, w, h).map_err(|e| e.to_string())?;
    let names: Vec<&str> = parts.iter().map(|r| r.part.people_name()).collect();
    let surfaces = scene.and_then(|dir| surfaces(&names, rgb, w, h, dir));
    let mut out = Vec::with_capacity(parts.len());
    for r in parts {
        let req = PeopleRequest::parse(r.person, &[r.part.people_name().to_string()]).ok_or("unknown People part")?;
        let seg = people_mask_with(&req, rgb, w, h, Some(&map), &faces, None, surfaces.as_ref()).map(|g| match g.rect {
            Some(q) => lcc_develop::SegMask::from_logits_in(g.side, &g.logits, q),
            None => lcc_develop::SegMask::from_logits(g.side, &g.logits),
        });
        out.push(RetouchMask { part: r.part, person: r.person, seg });
    }
    Ok(out)
}

/// The floors, walls and chairs around people, from the scene model in `dir`, when one of `names`
/// is Clothes (the only part they change: `people_mask_with`); `None` otherwise or when the model
/// can't run (Clothes then goes without the cut, as before).
#[cfg(feature = "sam")]
fn surfaces(names: &[&str], rgb: &[u8], w: usize, h: usize, dir: &std::path::Path) -> Option<lcc_segment::people::Surfaces> {
    if !names.iter().any(|n| n.eq_ignore_ascii_case("clothes")) {
        return None;
    }
    let model = lcc_segment::scene::SceneModel::load_cpu(dir).map_err(|e| log::warn!("scene model: {e}")).ok()?;
    let scene = model.segment(rgb, w, h).map_err(|e| log::warn!("scene model: {e}")).ok()?;
    Some(lcc_segment::people::Surfaces::from_scene(&scene))
}

/// Settings `d` with analysis `a` merged into theirs, when it is for their orientation and adds
/// something (`None`: nothing to change).
fn with(d: &DevelopSettings, a: &RetouchAnalysis) -> Option<DevelopSettings> {
    if a.orientation != d.orientation {
        return None;
    }
    let merged = match &d.retouch_analysis {
        Some(old) => old.merged(a),
        None => a.clone(),
    };
    if d.retouch_analysis.as_ref() == Some(&merged) {
        return None;
    }
    Some(DevelopSettings { retouch_analysis: Some(merged), ..d.clone() })
}

impl Session {
    /// The model input's render and its cache key for photo `id` (the look the models see).
    fn parts_render(&mut self, id: PhotoId) -> Result<(crate::media::RenderJob, u64), String> {
        let p = self.catalog.photo(id).ok_or("no such photo")?;
        let d = super::skin::model_look(&p.develop);
        let mut look = d.hash64() ^ id.0.rotate_left(23);
        for b in crate::media::content_key(p).bytes() {
            look = (look ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
        let edge = super::skin::INPUT_EDGE;
        let job = self.preview_job(id, edge, edge, false, &d).ok_or("no such photo")?;
        Ok((job, look))
    }

    fn parts_cached(&self, key: &RetouchKey, look: u64) -> Option<Vec<RetouchMask>> {
        self.segmenter.parts.cache.iter().find(|(k, l, _)| k == key && *l == look).map(|(_, _, m)| m.clone())
    }

    #[cfg_attr(not(feature = "sam"), allow(dead_code))]
    fn parts_keep(&mut self, key: RetouchKey, look: u64, masks: &[RetouchMask]) {
        let c = &mut self.segmenter.parts.cache;
        c.retain(|(k, l, _)| !(k == &key && *l == look));
        c.push((key, look, masks.to_vec()));
        if c.len() > CACHE {
            c.remove(0);
        }
    }

    /// Find `key`'s parts, waiting for the models (CLI, MCP, tests).
    fn segment_parts(&mut self, key: &RetouchKey) -> crate::Result<Vec<RetouchMask>> {
        let (people, face) = self.skin_models()?;
        let (job, look) = self.parts_render(key.photo).map_err(crate::EngineError::Other)?;
        if let Some(m) = self.parts_cached(key, look) {
            return Ok(m);
        }
        #[cfg(feature = "sam")]
        {
            let img = job.run().rendered.map_err(crate::EngineError::Other)?.image;
            let rgb: Vec<u8> = img.data.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
            let scene = self.segmenter.scene_dir_in_use();
            let masks = analyse(&rgb, img.width, img.height, &key.parts, &people, &face, scene.as_deref()).map_err(crate::EngineError::Other)?;
            self.parts_keep(key.clone(), look, &masks);
            Ok(masks)
        }
        #[cfg(not(feature = "sam"))]
        {
            let _ = (job, people, face);
            Err(crate::EngineError::Other("AI masks are not available in this build".into()))
        }
    }

    /// Queue `key` on a background thread (one activity row); [`Session::retouch_poll`] applies it.
    fn segment_parts_later(&mut self, key: &RetouchKey) -> crate::Result<()> {
        let (people, face) = self.skin_models()?;
        let (job, look) = self.parts_render(key.photo).map_err(crate::EngineError::Other)?;
        #[cfg(feature = "sam")]
        {
            let scene = self.segmenter.scene_dir_in_use();
            let results = self.segmenter.parts.results.get_or_insert_with(std::sync::mpsc::channel);
            let tx = results.0.clone();
            let key = key.clone();
            if let Some(masks) = self.parts_cached(&key, look) {
                let _ = tx.send(RetouchDone { key, look, result: Ok(masks) });
                return Ok(());
            }
            let task = self.activity.start("retouch", "Finding teeth", crate::activity::Cancel::No);
            let spawned = std::thread::Builder::new().name("retouch-parts".into()).spawn(move || {
                let _task = task;
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let img = job.run().rendered?.image;
                    let rgb: Vec<u8> = img.data.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
                    analyse(&rgb, img.width, img.height, &key.parts, &people, &face, scene.as_deref())
                }))
                .unwrap_or_else(|_| Err("the AI model failed unexpectedly".into()));
                let _ = tx.send(RetouchDone { key, look, result });
            });
            spawned.map(|_| ()).map_err(|e| crate::EngineError::Other(format!("could not start finding the teeth: {e}")))
        }
        #[cfg(not(feature = "sam"))]
        {
            let _ = (key, people, face, job, look);
            Err(crate::EngineError::Other("AI masks are not available in this build".into()))
        }
    }

    /// Give photo `id` the parts found (`a`) when its settings are for the same orientation. Saved
    /// like any change, without an undo step: the app found them, not the user.
    pub(crate) fn store_retouch_analysis(&mut self, id: PhotoId, a: &RetouchAnalysis) -> crate::Result<()> {
        let Some(photo) = self.catalog.photo(id) else { return Ok(()) };
        let Some(d) = with(&photo.develop, a) else { return Ok(()) };
        let op = Op::SetDevelop { id, settings: Arc::new(d), label: "Find Teeth".into(), edited: photo.edited.clone() };
        self.apply_unrecorded(op)
    }

    /// The parts photo `id`'s retouch sections want and its settings don't hold (`None`: none).
    pub(crate) fn retouch_wanted(&self, id: PhotoId) -> Option<RetouchKey> {
        let d = self.develop_of(id)?;
        let wanted = wanted(&d);
        let parts = match &d.retouch_analysis {
            Some(a) => a.missing(&wanted, d.orientation),
            None => wanted,
        };
        (!parts.is_empty()).then_some(RetouchKey { photo: id, parts, orientation: d.orientation })
    }

    /// Get photo `id` the parts its retouch sections want, if it lacks some: queued in background
    /// mode, waited for otherwise. `force` asks again after a failure. `Ok(true)`: they are there
    /// (or on their way); `Ok(false)`: nothing was wanted.
    pub(crate) fn retouch_ensure(&mut self, id: PhotoId, force: bool) -> crate::Result<bool> {
        let Some(key) = self.retouch_wanted(id) else { return Ok(false) };
        if force {
            self.segmenter.parts.failed.retain(|k| k.photo != id);
        }
        if self.segmenter.parts.pending.contains(&key) {
            return Ok(true);
        }
        if self.segmenter.parts.failed.contains(&key) {
            return Err(crate::EngineError::Other("Retouch couldn't find the teeth on this photo before; use Find Again to try again".into()));
        }
        let r = if self.segmenter.background {
            self.segment_parts_later(&key).map(|()| self.segmenter.parts.pending.push(key.clone()))
        } else {
            self.segment_parts(&key).and_then(|masks| self.store_retouch_analysis(id, &RetouchAnalysis { orientation: key.orientation, masks }))
        };
        match r {
            Ok(()) => Ok(true),
            Err(e) => {
                if !matches!(e, crate::EngineError::ModelMissing { .. }) {
                    self.segmenter.parts.failed.push(key);
                }
                Err(e)
            }
        }
    }

    /// After `op` was committed (or undone, or redone): every photo whose settings it set, and
    /// whose retouch wants parts it lacks, gets them. Failures are reported once, as messages.
    pub(crate) fn retouch_after(&mut self, op: &Op) {
        let mut ids = Vec::new();
        super::skin::developed(op, &mut ids, 0);
        for id in ids {
            if self.retouch_wanted(id).is_none() {
                continue;
            }
            match self.retouch_ensure(id, false) {
                Ok(_) | Err(crate::EngineError::ModelMissing { .. }) => {}
                Err(e) => {
                    let e = e.to_string();
                    if !e.starts_with("Retouch couldn't") {
                        log::warn!("Retouch: {e}");
                        let m = format!("Retouch: {e}");
                        if !self.segmenter.messages.contains(&m) {
                            self.segmenter.messages.push(m);
                        }
                    }
                }
            }
        }
    }

    /// Whether Retouch is looking for the parts of photo `id` right now.
    pub fn retouch_pending(&self, id: PhotoId) -> bool {
        self.segmenter.parts.pending.iter().any(|k| k.photo == id)
    }

    /// Apply the background retouch requests that finished (called with [`Session::skin_poll`]).
    pub(crate) fn retouch_poll(&mut self, polled: &mut super::Polled) {
        #[cfg(feature = "sam")]
        {
            let done: Vec<RetouchDone> = match &self.segmenter.parts.results {
                Some((_, rx)) => rx.try_iter().collect(),
                None => return,
            };
            for RetouchDone { key, look, result } in done {
                self.segmenter.parts.pending.retain(|k| *k != key);
                let masks = match result {
                    Ok(m) => m,
                    Err(e) => {
                        self.segmenter.parts.failed.push(key);
                        polled.messages.push(format!("Retouch: {e}"));
                        continue;
                    }
                };
                self.parts_keep(key.clone(), look, &masks);
                if masks.iter().all(|m| m.seg.is_none()) {
                    polled.messages.push("Retouch: no teeth were found in this photo.".to_string());
                }
                match self.store_retouch_analysis(key.photo, &RetouchAnalysis { orientation: key.orientation, masks }) {
                    Ok(()) => polled.changed = true,
                    Err(e) => polled.messages.push(e.to_string()),
                }
            }
        }
        #[cfg(not(feature = "sam"))]
        let _ = polled;
    }
}

#[cfg(all(test, feature = "sam"))]
mod tests {
    use super::*;

    #[test]
    fn only_clothes_runs_the_scene_model_and_a_damaged_one_is_no_cut() {
        let dir = std::env::temp_dir().join(format!("lcc-parts-scene-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("config.json"), br#"{"model_type": "eomt"}"#).unwrap();
        std::fs::write(dir.join("model.safetensors"), b"not weights").unwrap();
        let rgb = vec![128u8; 32 * 24 * 3];
        assert!(surfaces(&["Teeth"], &rgb, 32, 24, &dir).is_none());
        assert!(surfaces(&["Clothes"], &rgb, 32, 24, &dir).is_none(), "a damaged model: no cut, no panic");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_real_scene_model_finds_surfaces_for_clothes() {
        let Some(dir) = lcc_brand::env_os("SCENE_DIR").map(std::path::PathBuf::from) else {
            lcc_testkit::skip!(Model, "set {} to the EoMT-S checkpoint", lcc_brand::env_var("SCENE_DIR"));
            return;
        };
        // a grey wall above a brown wooden floor
        let (w, h) = (320usize, 240usize);
        let rgb: Vec<u8> = (0..w * h).flat_map(|i| if i / w < h / 2 { [170u8, 168, 160] } else { [120, 80, 50] }).collect();
        let s = surfaces(&["Clothes"], &rgb, w, h, &dir).expect("the model runs");
        assert!(s.at(0.5, 0.8) > 0.3 || s.at(0.5, 0.2) > 0.3, "a wall or floor is a surface");
    }
}
