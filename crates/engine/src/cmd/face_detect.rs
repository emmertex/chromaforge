//! `faces.detect`: find faces with the open MediaPipe face model (Face Landmarker: BlazeFace and FaceMesh-V2,
//! Apache-2.0, `ai.models.download {"id": "face"}`), the one Retouch uses. It replaced YuNet, whose weights were
//! trained on WIDER FACE (CC BY-NC-ND); a YuNet installed by an older version is left where it is and not used.
//!
//! The photo is rendered upright and uncropped with default settings (so edits, crops and rotations you have
//! made do not matter), the detector runs on it, and the faces come back as fractions of the photo's width
//! and height, the same frame face regions from XMP use. Unless `apply` is false, they are added to the photo as
//! unnamed face regions in one undoable step; earlier detections are replaced, and regions that came from XMP (or
//! that you drew or named) are never touched. Like every region edit it is catalog-only: no sidecar is written.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use lcc_catalog::Op;
use lcc_develop::DevelopSettings;
use lcc_faces::yunet::Face;
use lcc_meta::{Region, RegionKind};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, has_selection};
use crate::{AiModel, EngineError, Result, Session};

const C: &str = "faces.detect";
/// Regions made by this command say so in their description; only those are replaced on a new run.
pub(crate) const MARK: &str = "Detected by ";
pub(crate) const LABEL: &str = "MediaPipe Face Landmarker";
/// Long edge of the picture the detector looks at.
const LOOK_EDGE: usize = 1280;
/// How much a detection must overlap a region the photo already has to count as the same face.
const EXISTING_IOU: f64 = 0.3;
/// The default score a face must reach (BlazeFace's own floor is 0.65).
const DEFAULT_SCORE: f32 = 0.65;
/// Most faces one photo gives.
const DEFAULT_MAX_FACES: usize = 200;

/// Why [`detector`] has no detector to give.
#[derive(Debug)]
pub(crate) enum NoDetector {
    /// Not downloaded yet: a frontend offers the download ([`crate::ErrorKind::ModelMissing`]).
    #[cfg_attr(not(feature = "sam"), allow(dead_code))]
    NotInstalled(String),
    /// Not in this build, or there but unusable: says what to do.
    Unusable(String),
}

impl NoDetector {
    /// Command `cmd`'s error for it (an invalid-parameters message, as before kinds existed).
    pub(crate) fn into_error(self, cmd: &str) -> EngineError {
        match self {
            Self::NotInstalled(why) => EngineError::ModelMissing { model: AiModel::FaceDetector, msg: bad(cmd, why).to_string() },
            Self::Unusable(why) => bad(cmd, why),
        }
    }
}

/// The open face model, loaded.
pub(crate) struct Finder {
    #[cfg(feature = "sam")]
    model: Arc<lcc_segment::people::FaceModel>,
}

impl Finder {
    /// The faces in an 8-bit RGB picture (`w × h`), left to right: boxes and five landmarks as fractions of the
    /// picture (the eyes, the nose tip and the mouth corners, from FaceMesh where it placed its landmarks). `fine`
    /// also looks in overlapping tiles of half the picture, which finds smaller faces (group photos) at about four
    /// times the cost.
    pub(crate) fn detect(&self, rgb: &[u8], w: usize, h: usize, fine: bool) -> std::result::Result<Vec<Face>, String> {
        #[cfg(feature = "sam")]
        {
            use lcc_tflite::mediapipe;
            if w == 0 || h == 0 || rgb.len() < w.saturating_mul(h).saturating_mul(3) {
                return Ok(Vec::new());
            }
            let mut dets = self.model.detect(rgb, w, h).map_err(|e| e.to_string())?;
            if fine && w.min(h) >= 512 {
                // half-size tiles, overlapping by half: each is searched like a whole photo
                let (tw, th) = (w / 2, h / 2);
                let tiles = [0, w / 4, w - tw].into_iter().flat_map(|x| [0, h / 4, h - th].into_iter().map(move |y| (x, y)));
                for (x0, y0) in tiles {
                    let mut tile = Vec::with_capacity(tw * th * 3);
                    for y in y0..y0 + th {
                        tile.extend_from_slice(rgb.get((y * w + x0) * 3..(y * w + x0 + tw) * 3).unwrap_or(&[]));
                    }
                    if tile.len() != tw * th * 3 {
                        continue;
                    }
                    for d in self.model.detect(&tile, tw, th).map_err(|e| e.to_string())? {
                        dets.push(d.from_window(x0 as f32, y0 as f32, 1.0));
                    }
                }
                dets = mediapipe::merge_windows(dets, 0.3);
                dets.sort_by(|a, b| a.center()[0].total_cmp(&b.center()[0]));
            }
            let mut out = Vec::new();
            for d in dets {
                let landmarks = self.model.landmarks(rgb, w, h, &d).map_err(|e| e.to_string())?;
                // FaceMesh not seeing a face there, and the detector only fairly sure: a pattern, a paw, a circuit
                // board (the detector alone takes those for faces, mostly in the tiles of `fine`)
                if landmarks.is_none() && d.score < lcc_segment::people::FACE_THRESHOLD_NO_MESH {
                    continue;
                }
                let found = crate::segment::FoundFace::from_pixels(&lcc_segment::people::Face { detection: d, landmarks }, w as f32, h as f32);
                let [x0, y0, x1, y1] = found.rect;
                out.push(Face { x0, y0, x1, y1, score: found.score, landmarks: found.five() });
            }
            Ok(out)
        }
        #[cfg(not(feature = "sam"))]
        {
            let _ = (rgb, w, h, fine);
            Err(NOT_AVAILABLE.into())
        }
    }
}

#[cfg(not(feature = "sam"))]
const NOT_AVAILABLE: &str = "finding faces needs the open AI models, which this build does not include";

/// The installed face model, loaded once and kept (a new one is loaded when its folder changes).
/// (also used by the background scan, which aligns faces by the landmarks and finds faces in photos that have none)
pub(crate) fn detector(s: &Session) -> std::result::Result<Arc<Finder>, NoDetector> {
    #[cfg(feature = "sam")]
    {
        let dir = match s.segmenter.require_open_model("face", AiModel::FaceDetector) {
            Ok(d) => d,
            Err(EngineError::ModelMissing { msg, .. }) => return Err(NoDetector::NotInstalled(msg)),
            Err(e) => return Err(NoDetector::Unusable(e.to_string())),
        };
        let model = crate::segment::face_model(&dir).map_err(NoDetector::Unusable)?;
        Ok(Arc::new(Finder { model }))
    }
    #[cfg(not(feature = "sam"))]
    {
        let _ = s;
        Err(NoDetector::Unusable(NOT_AVAILABLE.into()))
    }
}

/// Where a face model runs, for Settings ▸ Faces and `faces.gpu`.
#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GpuState {
    /// The background check is still running.
    pub checking: bool,
    /// The GPU it runs on; `None` when it runs on the CPU.
    pub device: Option<String>,
    /// Why it runs on the CPU.
    pub why: Option<String>,
}

/// Each model's [`GpuState`], by what it does ("detection", "recognition").
static GPU_STATE: Mutex<Vec<(&'static str, GpuState)>> = Mutex::new(Vec::new());

pub(crate) fn gpu_state(what: &str) -> Option<GpuState> {
    GPU_STATE.lock().unwrap_or_else(PoisonError::into_inner).iter().find(|(w, _)| *w == what).map(|(_, st)| st.clone())
}

fn set_gpu_state(what: &'static str, st: GpuState) {
    let mut all = GPU_STATE.lock().unwrap_or_else(PoisonError::into_inner);
    match all.iter_mut().find(|(w, _)| *w == what) {
        Some((_, old)) => *old = st,
        None => all.push((what, st)),
    }
}

/// The user chose the CPU for face models (Settings ▸ Faces).
pub(crate) fn cpu_only(s: &Session) -> bool {
    s.face_models_dir.as_deref().is_some_and(|d| super::face_models::read_settings(d).cpu_only)
}

/// Check in the background whether the GPU runs a face model faster than the CPU and gives its results; the model
/// runs on the CPU meanwhile, and stays there when the GPU is off, missing, wrong or slower, or the user chose the CPU.
/// The check shows in the activity stack while it runs.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn offer_gpu(
    s: &Session,
    what: &'static str,
    attach: impl FnOnce(Arc<dyn lcc_faces::net::Accelerator>) -> std::result::Result<(), String> + Send + 'static,
) {
    if cpu_only(s) {
        set_gpu_state(what, GpuState { why: Some("the CPU is chosen in Settings".into()), ..GpuState::default() });
        return;
    }
    let task = s.activity.start("facesGpu", "Checking the GPU for faces", crate::activity::Cancel::No);
    task.detail(what);
    set_gpu_state(what, GpuState { checking: true, ..GpuState::default() });
    let spawned = std::thread::Builder::new().name(format!("faces-gpu-{what}")).spawn(move || {
        let _task = task;
        let gpu = match lcc_gpu::faces::accelerator() {
            Ok(gpu) => gpu,
            Err(why) => {
                log::info!("faces: {what} runs on the CPU ({why})");
                return set_gpu_state(what, GpuState { why: Some(why), ..GpuState::default() });
            }
        };
        let device = lcc_faces::net::Accelerator::device(&*gpu);
        match attach(gpu) {
            Ok(()) => {
                log::info!("faces: {what} runs on {device}");
                set_gpu_state(what, GpuState { device: Some(device), ..GpuState::default() });
            }
            Err(why) => {
                log::info!("faces: {what} stays on the CPU: {why}");
                set_gpu_state(what, GpuState { why: Some(why), ..GpuState::default() });
            }
        }
    });
    if let Err(e) = spawned {
        log::warn!("faces: could not check the GPU for {what}: {e}");
        set_gpu_state(what, GpuState { why: Some(format!("could not check the GPU: {e}")), ..GpuState::default() });
    }
}

/// `faces.gpu`: where the face models run, and the switch that keeps them on the CPU.
fn gpu(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "faces.gpu";
    if let Some(cpu) = p.get("cpuOnly") {
        let cpu = cpu.as_bool().ok_or_else(|| bad(C, "`cpuOnly` is true or false"))?;
        let dir = s.face_models_dir.clone().ok_or_else(|| bad(C, "this build has nowhere to keep face models (the desktop app does)"))?;
        super::face_models::set_cpu_only(&dir, cpu)?;
        #[cfg(not(target_arch = "wasm32"))]
        let embedder = s.faces.embedder.as_ref().map(|(_, e)| e.clone());
        #[cfg(target_arch = "wasm32")]
        let embedder: Option<Arc<lcc_faces::runtime::Embedder>> = None;
        if cpu {
            if let Some(e) = &embedder {
                e.net().detach();
            }
            set_gpu_state("recognition", GpuState { why: Some("the CPU is chosen in Settings".into()), ..GpuState::default() });
        } else {
            #[cfg(not(target_arch = "wasm32"))]
            {
                if let Some(e) = embedder {
                    offer_gpu(s, "recognition", move |gpu| e.accelerate(gpu, true).map_err(|e| e.to_string()));
                }
            }
        }
    }
    Ok(gpu_json(s))
}

/// What `faces.gpu` and `faces.models.list` say about the GPU.
pub(crate) fn gpu_json(s: &Session) -> Value {
    json!({
        "cpuOnly": cpu_only(s),
        "detection": gpu_state("detection"),
        "recognition": gpu_state("recognition"),
    })
}

fn face_json(f: &Face) -> Value {
    json!({
        "rect": {"x0": f.x0, "y0": f.y0, "x1": f.x1, "y1": f.y1},
        "score": f.score,
        "landmarks": f.landmarks.iter().map(|(x, y)| json!([x, y])).collect::<Vec<_>>(),
    })
}

/// A detected face that lies on a region the photo already has (from XMP, drawn or named) is that region, not a new face.
fn overlaps_existing(f: &Face, regions: &[Region]) -> bool {
    regions.iter().any(|r| {
        let (w, h) = (
            (r.rect.x1.min(f64::from(f.x1)) - r.rect.x0.max(f64::from(f.x0))).max(0.0),
            (r.rect.y1.min(f64::from(f.y1)) - r.rect.y0.max(f64::from(f.y0))).max(0.0),
        );
        let inter = w * h;
        let union = (r.rect.x1 - r.rect.x0) * (r.rect.y1 - r.rect.y0) + f64::from((f.x1 - f.x0) * (f.y1 - f.y0)) - inter;
        union > 0.0 && inter / union >= EXISTING_IOU
    })
}

/// A region Detect Faces may replace: one it found earlier and the user has not ignored (an ignored face is kept,
/// and its box stops the same face being found again).
pub(crate) fn is_detected(r: &Region) -> bool {
    r.kind != RegionKind::IgnoredFace && r.description.as_deref().is_some_and(|d| d.starts_with(MARK))
}

/// The photo as the faces see it: upright, uncropped, default settings, as 8-bit RGB with its size, its long edge at
/// most `edge`.
pub(crate) fn render_rgb(s: &mut Session, id: lcc_catalog::PhotoId, edge: usize) -> std::result::Result<(Vec<u8>, usize, usize), String> {
    let settings = DevelopSettings::default();
    let job = s.preview_job(id, edge, edge, false, &settings).ok_or("no such photo")?;
    let image = job.run().rendered?.image;
    let rgb: Vec<u8> = image.data.iter().flat_map(|px| [px[0], px[1], px[2]]).collect();
    Ok((rgb, image.width, image.height))
}

fn detect(s: &mut Session, p: &Value) -> Result<Value> {
    let apply = p.get("apply").and_then(Value::as_bool).unwrap_or(true);
    let score = p.get("score").and_then(Value::as_f64).filter(|v| v.is_finite()).map_or(DEFAULT_SCORE, |v| v as f32);
    let max_faces = p.get("maxFaces").and_then(Value::as_u64).map_or(DEFAULT_MAX_FACES, |v| usize::try_from(v).unwrap_or(DEFAULT_MAX_FACES));
    let fine = p.get("fine").and_then(Value::as_bool).unwrap_or(true);
    let detector = detector(s).map_err(|e| e.into_error(C))?;
    let started = Instant::now();
    let (mut results, mut ops) = (Vec::new(), Vec::new());
    for id in s.targets(p)? {
        let (rgb, width, height) = match render_rgb(s, id, LOOK_EDGE) {
            Ok(r) => r,
            Err(e) => {
                results.push(json!({"id": id.0, "error": e}));
                continue;
            }
        };
        let faces = match detector.detect(&rgb, width, height, fine) {
            Ok(mut f) => {
                f.retain(|f| f.score >= score);
                f.truncate(max_faces);
                f
            }
            Err(e) => {
                results.push(json!({"id": id.0, "error": e.to_string()}));
                continue;
            }
        };
        if apply && let Some(photo) = s.catalog.photo(id) {
            let mut meta = photo.meta.clone();
            meta.regions.retain(|r| !is_detected(r));
            let fresh: Vec<&Face> = faces.iter().filter(|f| !overlaps_existing(f, &meta.regions)).collect();
            for f in fresh {
                meta.regions.push(Region {
                    rect: lcc_geom::Rect { x0: f64::from(f.x0), y0: f64::from(f.y0), x1: f64::from(f.x1), y1: f64::from(f.y1) },
                    kind: RegionKind::Face,
                    name: None,
                    description: Some(format!("{MARK}{LABEL}")),
                });
            }
            ops.push(Op::SetMeta { id, meta: Box::new(meta) });
        }
        results.push(json!({"id": id.0, "faces": faces.iter().map(face_json).collect::<Vec<_>>()}));
    }
    let applied = !ops.is_empty();
    if applied {
        s.commit("Detect Faces", Op::Batch { ops })?;
        // regions are not written to XMP, so a sidecar must not be rewritten for this
        s.skip_auto_write = true;
    }
    Ok(json!({"detector": LABEL, "applied": applied, "photos": results, "ms": started.elapsed().as_secs_f64() * 1000.0}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "faces.detect",
            "Detect Faces",
            ["Library"],
            None,
            "{ids?, apply?: bool (default true), score?: 0.65, maxFaces?: 200, fine?: bool (true: also looks in tiles, for the small faces of group photos)} → {photos: [{id, faces: [{rect, score, landmarks}]}]}. Finds faces with the open MediaPipe face model (Apache-2.0; download it first: `ai.models.download {id: \"face\", acknowledged: true}`) and, unless `apply` is false, adds them as unnamed face regions (one undo step), replacing earlier detections but never regions from XMP or ones you named; the sidecar is not touched",
            has_selection,
            detect
        ),
        cmd!(
            query "faces.gpu",
            "Face Models on the GPU",
            [],
            None,
            "{cpuOnly?: bool} → {cpuOnly, detection, recognition}: where the face models run, each as {checking, device, why} (null until it is loaded). The GPU is used only after a background check shows it gives the CPU's results faster; `cpuOnly: true` keeps every face model on the CPU (Settings ▸ Faces), `false` checks the GPU again",
            always,
            gpu
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(x0: f32, y0: f32, x1: f32, y1: f32) -> Face {
        Face { x0, y0, x1, y1, score: 0.9, landmarks: [(0.0, 0.0); 5] }
    }
    fn region(x0: f64, y0: f64, x1: f64, y1: f64) -> Region {
        Region { rect: lcc_geom::Rect { x0, y0, x1, y1 }, kind: RegionKind::Face, name: Some("Ann".into()), description: None }
    }

    #[test]
    fn a_detection_on_an_existing_region_is_not_a_new_face() {
        let existing = [region(0.40, 0.20, 0.60, 0.50), region(0.10, 0.10, 0.20, 0.20)];
        // the same face, a bit larger or shifted (a Lightroom box is not the detector's box)
        assert!(overlaps_existing(&face(0.38, 0.18, 0.64, 0.54), &existing));
        assert!(overlaps_existing(&face(0.42, 0.22, 0.58, 0.48), &existing));
        // another face, a sliver of overlap, and no regions at all
        assert!(!overlaps_existing(&face(0.70, 0.20, 0.90, 0.50), &existing));
        assert!(!overlaps_existing(&face(0.59, 0.49, 0.80, 0.80), &existing));
        assert!(!overlaps_existing(&face(0.0, 0.0, 0.0, 0.0), &existing), "an empty box overlaps nothing");
        assert!(!overlaps_existing(&face(0.4, 0.2, 0.6, 0.5), &[]));
    }
}
