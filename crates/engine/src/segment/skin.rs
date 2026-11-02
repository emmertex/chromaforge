//! The skin and faces Smooth Skin and Find Blemishes work with, from the open MediaPipe models
//! (Selfie Multiclass for skin, Face Landmarker for faces and their landmarks; `ai.models.download
//! {"id": "people" | "face"}`). Ported from OneSlips.
//!
//! Smooth Skin keeps what was found in the photo's settings ([`SkinAnalysis`]), so rendering and
//! exporting never need a model. It is the photo's own data, not an edit: it is in no settings
//! group (copy, paste, sync and presets don't carry it) and it is stored without an undo step of
//! its own. After every committed edit, the photos whose Smooth Skin wants an analysis they lack
//! (just switched on, pasted, synced, another area or person, turned) get one
//! ([`Session::skin_after`]): on a background thread with a row in the activity stack in the desktop
//! app ([`super::Segmenter::background`]), at once in the CLI, MCP and tests. Results are kept for the
//! session, so undoing and redoing never runs the models again.

use std::sync::Arc;

use lcc_catalog::{Op, PhotoId};
use lcc_develop::{DevelopSettings, SegMask, SkinAnalysis, SkinArea, SkinFace, SkinSmoothing};
use lcc_geom::Orientation;

use crate::Session;

/// FaceMesh landmarks: the iris centres (image left first), the nose tip, the mouth corners.
const IRIS: [usize; 2] = [468, 473];
const NOSE_TIP: usize = 1;
const MOUTH_CORNERS: [usize; 2] = [61, 291];
/// Long edge of the picture the models are given.
#[cfg_attr(not(feature = "sam"), allow(dead_code))]
const INPUT_EDGE: usize = 1024;
/// Analyses kept for the session (undo, redo, switching area back and forth).
#[cfg_attr(not(feature = "sam"), allow(dead_code))]
const CACHE: usize = 16;

/// A face the MediaPipe models found, in normalized coordinates (0..1, y down) of the uncropped,
/// upright photo as developed.
#[derive(Clone, Debug, PartialEq)]
pub struct FoundFace {
    /// `[x0, y0, x1, y1]`.
    pub rect: [f32; 4],
    pub score: f32,
    /// BlazeFace's keypoints: right eye (on the image's left), left eye, nose tip, mouth, right
    /// ear, left ear.
    pub keypoints: [[f32; 2]; 6],
    /// FaceMesh's 478 landmarks, when it placed them (it doesn't on strong profiles).
    pub mesh: Option<Vec<[f32; 2]>>,
    /// Width / height of the photo it was found in.
    pub aspect: f32,
}

impl FoundFace {
    /// From the models' output in pixels of a `w × h` input.
    #[cfg(feature = "sam")]
    pub(crate) fn from_pixels(f: &lcc_segment::people::Face, w: f32, h: f32) -> FoundFace {
        let d = &f.detection;
        let (w, h) = (w.max(1.0), h.max(1.0));
        let n = |p: [f32; 2]| [p[0] / w, p[1] / h];
        FoundFace {
            rect: [d.x0 / w, d.y0 / h, d.x1 / w, d.y1 / h],
            score: d.score,
            keypoints: d.keypoints.map(n),
            mesh: f.landmarks.as_ref().map(|lm| lm.iter().map(|p| n(*p)).collect()),
            aspect: w / h,
        }
    }

    fn mesh_point(&self, i: usize) -> Option<[f32; 2]> {
        self.mesh.as_ref()?.get(i).copied().filter(|p| p[0].is_finite() && p[1].is_finite())
    }

    /// The eye centres (image left first): the irises when FaceMesh placed them, else BlazeFace's.
    pub fn eyes(&self) -> ([f32; 2], [f32; 2]) {
        match (self.mesh_point(IRIS[0]), self.mesh_point(IRIS[1])) {
            (Some(a), Some(b)) => (a, b),
            _ => (self.keypoints[0], self.keypoints[1]),
        }
    }

    /// The distance between the eyes as a fraction of the photo's long edge: the scale Smooth Skin
    /// measures its bands in. A turned face's eyes look closer than they are, so it is at least a
    /// share of the face's width (a fifth with landmarks, about a third without).
    pub fn eye_distance(&self) -> f64 {
        let (a, b) = self.eyes();
        let aspect = if self.aspect.is_finite() && self.aspect > 0.0 { f64::from(self.aspect) } else { 1.0 };
        // (in units of the photo's height, then of its long edge)
        let (dx, dy) = (f64::from(b[0] - a[0]) * aspect, f64::from(b[1] - a[1]));
        let width = f64::from(self.rect[2] - self.rect[0]).abs() * aspect;
        let floor = width * if self.mesh.is_some() { 0.2 } else { 0.33 };
        let d = dx.hypot(dy).max(floor) / aspect.max(1.0);
        if d.is_finite() { d } else { 0.0 }
    }

    /// The box's centre.
    pub fn centre(&self) -> [f32; 2] {
        [(self.rect[0] + self.rect[2]) / 2.0, (self.rect[1] + self.rect[3]) / 2.0]
    }

    /// The five points a face recogniser aligns by, in its order: right eye, left eye, nose tip,
    /// right and left mouth corner (the subject's right eye is on the image's left). Without
    /// landmarks the mouth corners are placed beside BlazeFace's mouth centre.
    pub fn five(&self) -> [(f32, f32); 5] {
        let (re, le) = self.eyes();
        let nose = self.mesh_point(NOSE_TIP).unwrap_or(self.keypoints[2]);
        let (mr, ml) = match (self.mesh_point(MOUTH_CORNERS[0]), self.mesh_point(MOUTH_CORNERS[1])) {
            (Some(a), Some(b)) => (a, b),
            _ => {
                let m = self.keypoints[3];
                let half = [(le[0] - re[0]) * 0.415, (le[1] - re[1]) * 0.415];
                ([m[0] - half[0], m[1] - half[1]], [m[0] + half[0], m[1] + half[1]])
            }
        };
        [re, le, nose, mr, ml].map(|p| (p[0], p[1]))
    }
}

/// The skin and faces found on a photo.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SkinFound {
    /// The skin asked for (`None`: none found, e.g. no such person).
    pub seg: Option<SegMask>,
    /// Every face, left to right.
    pub faces: Vec<FoundFace>,
}

/// One analysis: which photo, which skin, the photo turned how.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SkinKey {
    pub photo: PhotoId,
    pub area: SkinArea,
    pub person: u32,
    pub orientation: Orientation,
}

impl SkinKey {
    fn of(photo: PhotoId, d: &DevelopSettings) -> SkinKey {
        SkinKey { photo, area: d.skin.area, person: d.skin.person, orientation: d.orientation }
    }
}

/// What a skin request is for.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(not(feature = "sam"), allow(dead_code))]
pub(crate) enum SkinPurpose {
    /// Store it for Smooth Skin.
    Smooth,
    /// Find blemishes with it and add a heal spot on each.
    Blemishes(lcc_pipeline::blemish::Options),
}

/// A finished background request.
#[cfg_attr(not(feature = "sam"), allow(dead_code))]
pub(crate) struct SkinDone {
    key: SkinKey,
    look: u64,
    purpose: SkinPurpose,
    result: Result<SkinFound, String>,
}

/// The session's skin requests: queued, failed (asked again only on request), kept results.
#[derive(Default)]
pub(crate) struct SkinJobs {
    pub pending: Vec<SkinKey>,
    pub failed: Vec<SkinKey>,
    cache: Vec<(SkinKey, u64, SkinFound)>,
    #[cfg_attr(not(feature = "sam"), allow(dead_code))]
    results: Option<(std::sync::mpsc::Sender<SkinDone>, std::sync::mpsc::Receiver<SkinDone>)>,
}

/// Smooth Skin's analysis of `found`: its skin (an empty one when none was found, so nothing is
/// smoothed and nothing is asked again) and the faces its bands are measured by (the chosen
/// person's, or everyone's).
pub(crate) fn analysis_of(area: SkinArea, person: u32, orientation: Orientation, found: &SkinFound) -> SkinAnalysis {
    let faces: Vec<&FoundFace> = match person {
        0 => found.faces.iter().collect(),
        n => found.faces.get((n as usize).saturating_sub(1)).into_iter().collect(),
    };
    let faces = faces
        .into_iter()
        .map(|f| {
            let c = f.centre();
            SkinFace { x: f64::from(c[0]), y: f64::from(c[1]), eyes: f.eye_distance() }
        })
        .filter(|f| f.eyes > 0.0 && f.x.is_finite() && f.y.is_finite())
        .take(lcc_develop::MAX_SKIN_FACES)
        .collect();
    SkinAnalysis { area, person, orientation, seg: found.seg.clone().unwrap_or_else(|| SegMask::from_logits(1, &[-16.0])), faces }
}

/// Photo ids whose settings `op` sets (batches nested at most 16 deep).
fn developed(op: &Op, out: &mut Vec<PhotoId>, depth: usize) {
    match op {
        Op::SetDevelop { id, .. } if !out.contains(id) => out.push(*id),
        Op::Batch { ops } if depth < 16 => ops.iter().for_each(|o| developed(o, out, depth + 1)),
        _ => {}
    }
}

/// Settings `d` as the models see the photo: its look without masks or Smooth Skin.
fn model_look(d: &DevelopSettings) -> DevelopSettings {
    let mut d = d.clone();
    d.masks.clear();
    d.skin = SkinSmoothing::default();
    d.skin_analysis = None;
    d
}

/// The models, loaded once and kept (a new one is loaded when its folder changes).
#[cfg(feature = "sam")]
mod loaded {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, PoisonError};

    use lcc_segment::people::{FaceModel, PeopleModel};

    type Slot<T> = Mutex<Option<(PathBuf, Arc<T>)>>;

    fn get<T>(slot: &Slot<T>, dir: &Path, load: impl FnOnce(&Path) -> Result<T, lcc_segment::Error>, what: &str) -> Result<Arc<T>, String> {
        let mut g = slot.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((p, m)) = g.as_ref()
            && p == dir
        {
            return Ok(m.clone());
        }
        let m = Arc::new(load(dir).map_err(|e| format!("the {what} model doesn't load ({e}): remove it and download it again"))?);
        *g = Some((dir.to_path_buf(), m.clone()));
        Ok(m)
    }

    static FACE: Slot<FaceModel> = Mutex::new(None);
    static PEOPLE: Slot<PeopleModel> = Mutex::new(None);

    pub fn face(dir: &Path) -> Result<Arc<FaceModel>, String> {
        get(&FACE, dir, FaceModel::load, "MediaPipe faces")
    }

    pub fn people(dir: &Path) -> Result<Arc<PeopleModel>, String> {
        get(&PEOPLE, dir, PeopleModel::load, "MediaPipe people")
    }
}

/// The face model in `dir`, loaded once (Detect Faces and the face scan share it with Retouch).
#[cfg(feature = "sam")]
pub(crate) fn face_model(dir: &std::path::Path) -> Result<Arc<lcc_segment::people::FaceModel>, String> {
    loaded::face(dir)
}

/// The skin of `area` (everyone's, or the `person`-th person's from the left) and every face on an
/// 8-bit RGB picture `w × h`.
#[cfg(feature = "sam")]
fn analyse(
    rgb: &[u8],
    w: usize,
    h: usize,
    area: SkinArea,
    person: u32,
    people: &std::path::Path,
    face: &std::path::Path,
) -> Result<SkinFound, String> {
    use lcc_segment::people::{PeopleRequest, people_mask};
    let pm = loaded::people(people)?;
    let fm = loaded::face(face)?;
    let map = pm.segment(rgb, w, h).map_err(|e| e.to_string())?;
    let faces = fm.faces(rgb, w, h).map_err(|e| e.to_string())?;
    let parts: Vec<String> = match area {
        SkinArea::Face => vec!["Face Skin".into()],
        SkinArea::Body => vec!["Body Skin".into()],
        SkinArea::Both => vec!["Face Skin".into(), "Body Skin".into()],
    };
    let req = PeopleRequest::parse(person, &parts).ok_or("unknown People part")?;
    let seg = people_mask(&req, rgb, w, h, Some(&map), &faces).map(|g| match g.rect {
        Some(r) => SegMask::from_logits_in(g.side, &g.logits, r),
        None => SegMask::from_logits(g.side, &g.logits),
    });
    let (fw, fh) = (w.max(1) as f32, h.max(1) as f32);
    Ok(SkinFound { seg, faces: faces.iter().map(|f| FoundFace::from_pixels(f, fw, fh)).collect() })
}

impl Session {
    /// The People (Selfie Multiclass) and face (Face Landmarker) model folders, or
    /// [`crate::EngineError::ModelMissing`] naming the one to download.
    pub(crate) fn skin_models(&self) -> crate::Result<(std::path::PathBuf, std::path::PathBuf)> {
        let face = self.segmenter.require_open_model("face", crate::AiModel::FaceDetector)?;
        let people = self.segmenter.require_open_model("people", crate::AiModel::People)?;
        Ok((people, face))
    }

    /// The model input's render and its cache key for photo `id`.
    fn skin_render(&mut self, id: PhotoId) -> Result<(crate::media::RenderJob, u64), String> {
        let p = self.catalog.photo(id).ok_or("no such photo")?;
        let d = model_look(&p.develop);
        let mut look = d.hash64() ^ id.0.rotate_left(17);
        for b in crate::media::content_key(p).bytes() {
            look = (look ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
        let job = self.preview_job(id, INPUT_EDGE, INPUT_EDGE, false, &d).ok_or("no such photo")?;
        Ok((job, look))
    }

    fn skin_cached(&self, key: &SkinKey, look: u64) -> Option<SkinFound> {
        self.segmenter.skin.cache.iter().find(|(k, l, _)| k == key && *l == look).map(|(_, _, f)| f.clone())
    }

    #[cfg_attr(not(feature = "sam"), allow(dead_code))]
    fn skin_keep(&mut self, key: SkinKey, look: u64, found: &SkinFound) {
        let c = &mut self.segmenter.skin.cache;
        c.retain(|(k, l, _)| !(k == &key && *l == look));
        c.push((key, look, found.clone()));
        if c.len() > CACHE {
            c.remove(0);
        }
    }

    /// Find the skin of `area` (everyone's, or the `person`-th person's from the left) and every
    /// face on photo `id`, waiting for the models.
    pub fn segment_skin(&mut self, id: PhotoId, area: SkinArea, person: u32) -> crate::Result<SkinFound> {
        let (people, face) = self.skin_models()?;
        let orientation = self.develop_of(id).map(|d| d.orientation).unwrap_or_default();
        let key = SkinKey { photo: id, area, person, orientation };
        let (job, look) = self.skin_render(id).map_err(crate::EngineError::Other)?;
        if let Some(f) = self.skin_cached(&key, look) {
            return Ok(f);
        }
        #[cfg(feature = "sam")]
        {
            let img = job.run().rendered.map_err(crate::EngineError::Other)?.image;
            let rgb: Vec<u8> = img.data.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
            let found = analyse(&rgb, img.width, img.height, area, person, &people, &face).map_err(crate::EngineError::Other)?;
            self.skin_keep(key, look, &found);
            Ok(found)
        }
        #[cfg(not(feature = "sam"))]
        {
            let _ = (job, people, face);
            Err(crate::EngineError::Other("AI masks are not available in this build".into()))
        }
    }

    /// Queue skin request `key` for `purpose` on a background thread (one activity row);
    /// [`Session::skin_poll`] applies it.
    pub(crate) fn segment_skin_later(&mut self, key: SkinKey, purpose: SkinPurpose) -> crate::Result<()> {
        let (people, face) = self.skin_models()?;
        let (job, look) = self.skin_render(key.photo).map_err(crate::EngineError::Other)?;
        #[cfg(feature = "sam")]
        {
            let results = self.segmenter.skin.results.get_or_insert_with(std::sync::mpsc::channel);
            let tx = results.0.clone();
            if let Some(found) = self.skin_cached(&key, look) {
                let _ = tx.send(SkinDone { key, look, purpose, result: Ok(found) });
                return Ok(());
            }
            let label = match purpose {
                SkinPurpose::Smooth => "Finding skin",
                SkinPurpose::Blemishes(_) => "Finding blemishes",
            };
            let task = self.activity.start("retouch", label, crate::activity::Cancel::No);
            let spawned = std::thread::Builder::new().name("skin".into()).spawn(move || {
                let _task = task;
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let img = job.run().rendered?.image;
                    let rgb: Vec<u8> = img.data.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
                    analyse(&rgb, img.width, img.height, key.area, key.person, &people, &face)
                }))
                .unwrap_or_else(|_| Err("the AI model failed unexpectedly".into()));
                let _ = tx.send(SkinDone { key, look, purpose, result });
            });
            spawned.map(|_| ()).map_err(|e| crate::EngineError::Other(format!("could not start finding the skin: {e}")))
        }
        #[cfg(not(feature = "sam"))]
        {
            let _ = (key, purpose, people, face, job, look);
            Err(crate::EngineError::Other("AI masks are not available in this build".into()))
        }
    }

    /// Give photo `id` Smooth Skin's analysis `a` when its settings want it (an edit made
    /// meanwhile wins). Saved like any change, without an undo step: the app found it, not the user.
    pub(crate) fn store_skin_analysis(&mut self, id: PhotoId, a: &SkinAnalysis) -> crate::Result<()> {
        let Some(photo) = self.catalog.photo(id) else { return Ok(()) };
        let mut d = (*photo.develop).clone();
        if !a.is_for(&d.skin, d.orientation) || d.skin_analysis.as_ref() == Some(a) {
            return Ok(());
        }
        d.skin_analysis = Some(a.clone());
        let op = Op::SetDevelop { id, settings: Arc::new(d), label: "Find Skin".into(), edited: photo.edited.clone() };
        self.apply_unrecorded(op)
    }

    /// Whether photo `id`'s Smooth Skin wants an analysis it doesn't have (`Some(key)`).
    pub(crate) fn skin_wanted(&self, id: PhotoId) -> Option<SkinKey> {
        let d = self.develop_of(id)?;
        if !d.skin.active() || !d.retouch_section_on("skin") {
            return None;
        }
        if d.skin_analysis.as_ref().is_some_and(|a| a.is_for(&d.skin, d.orientation)) {
            return None;
        }
        Some(SkinKey::of(id, &d))
    }

    /// Get photo `id` the analysis its Smooth Skin wants, if it lacks one: queued in background
    /// mode, waited for otherwise. `force` asks again after a failure. `Ok(true)`: it has one now
    /// (or it is on its way); `Ok(false)`: nothing was wanted.
    pub(crate) fn skin_ensure(&mut self, id: PhotoId, force: bool) -> crate::Result<bool> {
        let Some(key) = self.skin_wanted(id) else { return Ok(false) };
        if force {
            self.segmenter.skin.failed.retain(|k| *k != key);
        }
        if self.segmenter.skin.pending.contains(&key) {
            return Ok(true);
        }
        if self.segmenter.skin.failed.contains(&key) {
            return Err(crate::EngineError::Other("Smooth Skin couldn't find the skin on this photo before; use Find Skin to try again".into()));
        }
        if self.segmenter.background {
            match self.segment_skin_later(key, SkinPurpose::Smooth) {
                Ok(()) => {
                    self.segmenter.skin.pending.push(key);
                    Ok(true)
                }
                Err(e) => {
                    if !matches!(e, crate::EngineError::ModelMissing { .. }) {
                        self.segmenter.skin.failed.push(key);
                    }
                    Err(e)
                }
            }
        } else {
            match self.segment_skin(id, key.area, key.person) {
                Ok(found) => {
                    self.store_skin_analysis(id, &analysis_of(key.area, key.person, key.orientation, &found))?;
                    Ok(true)
                }
                Err(e) => {
                    if !matches!(e, crate::EngineError::ModelMissing { .. }) {
                        self.segmenter.skin.failed.push(key);
                    }
                    Err(e)
                }
            }
        }
    }

    /// After `op` was committed: every photo whose settings it set, and whose Smooth Skin wants an
    /// analysis, gets one. Failures are reported once, as messages of the next poll.
    pub(crate) fn skin_after(&mut self, op: &Op) {
        if !super::Segmenter::AVAILABLE {
            return;
        }
        let mut ids = Vec::new();
        developed(op, &mut ids, 0);
        for id in ids {
            if self.skin_wanted(id).is_none() {
                continue;
            }
            match self.skin_ensure(id, false) {
                Ok(_) => {}
                // (a missing model is the command's reply, and the Retouch section offers it)
                Err(crate::EngineError::ModelMissing { .. }) => {}
                Err(e) => {
                    let e = e.to_string();
                    if !e.starts_with("Smooth Skin couldn't") {
                        log::warn!("Smooth Skin: {e}");
                        let m = format!("Smooth Skin: {e}");
                        if !self.segmenter.messages.contains(&m) {
                            self.segmenter.messages.push(m);
                        }
                    }
                }
            }
        }
    }

    /// Whether Smooth Skin is looking for the skin of photo `id` right now.
    pub fn skin_pending(&self, id: PhotoId) -> bool {
        self.segmenter.skin.pending.iter().any(|k| k.photo == id)
    }

    /// Apply the background skin requests that finished (called by [`Session::segment_poll`]).
    pub(crate) fn skin_poll(&mut self, polled: &mut super::Polled) {
        #[cfg(feature = "sam")]
        {
            let done: Vec<SkinDone> = match &self.segmenter.skin.results {
                Some((_, rx)) => rx.try_iter().collect(),
                None => return,
            };
            for d in done {
                self.apply_skin_done(d, polled);
            }
        }
        #[cfg(not(feature = "sam"))]
        let _ = polled;
    }

    #[cfg(feature = "sam")]
    fn apply_skin_done(&mut self, d: SkinDone, polled: &mut super::Polled) {
        let SkinDone { key, look, purpose, result } = d;
        if purpose == SkinPurpose::Smooth {
            self.segmenter.skin.pending.retain(|k| *k != key);
        }
        let found = match result {
            Ok(f) => f,
            Err(e) => {
                if purpose == SkinPurpose::Smooth {
                    self.segmenter.skin.failed.push(key);
                    return polled.messages.push(format!("Smooth Skin: {e}"));
                }
                return polled.messages.push(format!("Find Blemishes: {e}"));
            }
        };
        self.skin_keep(key, look, &found);
        match purpose {
            SkinPurpose::Smooth => {
                if found.seg.is_none() {
                    polled.messages.push(match key.person {
                        0 => "Smooth Skin: no skin was found in this photo.".to_string(),
                        n => format!("Smooth Skin: person {n} wasn't found in this photo."),
                    });
                }
                let a = analysis_of(key.area, key.person, key.orientation, &found);
                match self.store_skin_analysis(key.photo, &a) {
                    Ok(()) => polled.changed = true,
                    Err(e) => polled.messages.push(e.to_string()),
                }
            }
            SkinPurpose::Blemishes(opts) => {
                let r = self.execute_fn("spot.findBlemishes", |s| crate::cmd::retouch::add_blemishes(s, key.photo, &found, &opts));
                match r {
                    Ok(v) => {
                        polled.changed = true;
                        polled.messages.push(crate::cmd::retouch::blemishes_message(&v));
                    }
                    Err(e) => polled.messages.push(e.to_string()),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(mesh: bool) -> FoundFace {
        let mut lm = vec![[0.5f32, 0.5]; 478];
        lm[IRIS[0]] = [0.4, 0.4];
        lm[IRIS[1]] = [0.6, 0.4];
        lm[NOSE_TIP] = [0.5, 0.5];
        lm[MOUTH_CORNERS[0]] = [0.42, 0.6];
        lm[MOUTH_CORNERS[1]] = [0.58, 0.6];
        FoundFace {
            rect: [0.3, 0.25, 0.7, 0.75],
            score: 0.9,
            keypoints: [[0.41, 0.41], [0.59, 0.41], [0.5, 0.5], [0.5, 0.6], [0.3, 0.45], [0.7, 0.45]],
            mesh: mesh.then_some(lm),
            aspect: 1.5,
        }
    }

    #[test]
    fn eye_distance_is_a_fraction_of_the_long_edge() {
        // 0.2 of the width apart on a 3:2 photo: 0.2 of the long edge
        assert!((face(true).eye_distance() - 0.2).abs() < 1e-6);
        // without landmarks, BlazeFace's eyes, but at least a third of the face's width
        assert!((face(false).eye_distance() - 0.18).abs() < 1e-6);
        let mut turned = face(false);
        turned.keypoints[1] = [0.43, 0.41];
        assert!((turned.eye_distance() - 0.4 * 0.33).abs() < 1e-6, "{}", turned.eye_distance());
        let mut f = face(true);
        f.aspect = f32::NAN;
        assert!(f.eye_distance().is_finite());
    }

    #[test]
    fn the_five_alignment_points_follow_the_recognisers_order() {
        let p = face(true).five();
        assert_eq!((p[0], p[1], p[2]), ((0.4, 0.4), (0.6, 0.4), (0.5, 0.5)));
        assert_eq!((p[3], p[4]), ((0.42, 0.6), (0.58, 0.6)));
        // without landmarks the mouth corners sit beside the mouth centre, image left first
        let q = face(false).five();
        assert!(q[3].0 < 0.5 && q[4].0 > 0.5 && (q[3].1 - 0.6).abs() < 1e-6);
    }

    #[test]
    fn an_analysis_takes_the_chosen_persons_face() {
        let mut left = face(true);
        left.rect = [0.1, 0.2, 0.3, 0.5];
        let found = SkinFound { seg: Some(SegMask::from_logits(2, &[1.0; 4])), faces: vec![left, face(true)] };
        assert_eq!(analysis_of(SkinArea::Face, 0, Orientation::Normal, &found).faces.len(), 2);
        let second = analysis_of(SkinArea::Face, 2, Orientation::Normal, &found);
        assert_eq!(second.faces.len(), 1);
        assert!((second.faces[0].x - 0.5).abs() < 1e-6);
        assert!(analysis_of(SkinArea::Face, 3, Orientation::Normal, &found).faces.is_empty());
        // nothing found: an empty skin, so nothing is smoothed and nothing is asked again
        let none = analysis_of(SkinArea::Body, 1, Orientation::Rotate90, &SkinFound::default());
        assert_eq!((none.area, none.person, none.orientation), (SkinArea::Body, 1, Orientation::Rotate90));
        assert!(none.seg.logits().is_some_and(|l| l.iter().all(|v| *v < 0.0)));
    }
}
