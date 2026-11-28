//! AI orientation detection (issue #490) in the session: `photo.autoOrient` finds photos shown the wrong way up and
//! turns them (one undoable "Auto Orient" step, like Rotate Left/Right: the file is never touched), `ml.orientation.fit`
//! fits the classifier on a library whose photos stand upright, and `ml.orientation.status` says which classifier is
//! used and how the last run went.
//!
//! Each photo is rendered as it is shown now (its orientation, nothing else), turned four ways and embedded by the
//! active CLIP model on the shared AI queue (`lcc_ml::Queue`, the GPU when there is one: ONNX Runtime, then our wgpu
//! kernels, each checked against the CPU encoder), then `lcc_ml::orient` pools the four answers. The run has its own
//! row in the activity stack ("Detecting orientation · 3 of 20"); its ✕ stops it and nothing is turned. See
//! `docs/ml-backend.md`.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use lcc_catalog::{Op, PhotoId};
use lcc_develop::DevelopSettings;
use lcc_geom::Orientation;
use lcc_ml::orient::{self, Example, FitOptions, Head, Verdict};
use lcc_ml::{Ctx, Encoder, Job, JobError, Outcome, Priority, RgbImage};
use serde_json::{Value, json};

use crate::Session;
use crate::activity::{Cancel, TaskGuard};
use crate::cmd::bad;
use crate::ml::Output;

/// The activity row's kind and labels.
pub(crate) const ROW_KIND: &str = "mlOrient";
const ROW_LABEL: &str = "Detecting orientation";
const FIT_ROW_LABEL: &str = "Fitting the orientation classifier";
/// The jobs' key prefix on the AI queue.
pub(crate) const KEY: &str = "orient:";
/// The long edge each photo is rendered at: CLIP's 224 px crop with room to downscale from.
const VIEW_EDGE: usize = 448;
/// How long `wait: true` waits by default, and at most (`photo.autoOrient`; the fit has its own).
const WAIT_MS: u64 = 60_000;
const WAIT_MAX_MS: u64 = 3_600_000;
const FIT_WAIT_MS: u64 = 6 * 3_600_000;
const FIT_WAIT_MAX_MS: u64 = 48 * 3_600_000;

/// What one photo's job gives back: its four views' embeddings and the orientation it was shown in.
struct ViewsOutput {
    id: PhotoId,
    shown: Orientation,
    embeddings: Vec<Vec<f32>>,
}

struct Found {
    id: PhotoId,
    shown: Orientation,
    verdict: Verdict,
}

/// A `photo.autoOrient` run.
struct Run {
    waiting: HashSet<PhotoId>,
    total: usize,
    apply: bool,
    min_confidence: f32,
    head: Head,
    found: Vec<Found>,
    failed: Vec<(PhotoId, String)>,
    row: TaskGuard,
    cancel: Arc<AtomicBool>,
}

/// An `ml.orientation.fit` run.
struct Fit {
    waiting: HashSet<PhotoId>,
    total: usize,
    examples: Vec<Example>,
    failed: usize,
    row: TaskGuard,
}

#[derive(Default)]
pub(crate) struct State {
    run: Option<Run>,
    fit: Option<Fit>,
    /// The last finished run's answer (`ml.orientation.status`).
    last: Option<Value>,
    /// Photos turned by finished runs since the frontend last asked (`ml.pump` reports and clears it).
    pub(crate) turned_unseen: usize,
}

impl State {
    pub(crate) fn busy(&self) -> bool {
        self.run.is_some() || self.fit.is_some()
    }
}

/// The classifier fitted for an embedding model, saved beside the models (`<models>/orientation-<model id>.json`).
fn fitted_head_path(models_dir: &std::path::Path, model_id: &str) -> PathBuf {
    models_dir.join(format!("orientation-{model_id}.json"))
}

fn read_capped(path: &std::path::Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    f.take(orient::MAX_HEAD_BYTES + 1).read_to_end(&mut bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(bytes)
}

fn verdict_json(f: &Found, applied: bool) -> Value {
    json!({
        "id": f.id.0,
        "turn": f.verdict.degrees(),
        "confidence": f.verdict.confidence,
        "applied": applied,
    })
}

impl Session {
    /// The orientation classifier for `enc`, and where it comes from: one fitted with `ml.orientation.fit` for this
    /// exact model, else the one built in for it.
    fn orientation_head(&self, enc: &dyn Encoder) -> Result<(Head, &'static str), String> {
        let m = enc.manifest();
        let tag = m.tag();
        if let Some(dir) = &self.ml_models_dir {
            let path = fitted_head_path(dir, &m.id);
            if path.is_file() {
                match read_capped(&path).and_then(|b| Head::from_json(&b)) {
                    Ok(h) if h.embedder == tag => return Ok((h, "fitted")),
                    Ok(h) => log::warn!("ml: {} was fitted for {}, not {tag}; it is not used", path.display(), h.embedder),
                    Err(e) => log::warn!("ml: {} is not used: {e}", path.display()),
                }
            }
        }
        if let Some(h) = orient::builtin(&tag) {
            return Ok((h, "builtIn"));
        }
        Err(format!("there is no orientation classifier for {} yet: fit one on a library of upright photos with `ml.orientation.fit`", m.name))
    }

    /// Queues photo `id`'s four views on the AI queue. `false` when it can't be (not in the catalogue, queue full).
    fn queue_orient_views(&mut self, enc: &Arc<dyn Encoder>, id: PhotoId, priority: Priority) -> bool {
        let Some(dev) = self.develop_of(id) else {
            return false;
        };
        let shown = dev.orientation;
        // as it is shown: turned as the photo is, nothing else applied
        let settings = DevelopSettings { orientation: shown, ..DevelopSettings::default() };
        let Some(job) = self.preview_job(id, VIEW_EDGE, VIEW_EDGE, false, &settings) else {
            return false;
        };
        let name = self.catalog.photo(id).map(|p| p.file_name.clone()).unwrap_or_default();
        let enc = Arc::clone(enc);
        let mut job = Some(job);
        let key = format!("{KEY}{}", id.0);
        let work = Job::new(key, move |ctx: &Ctx| -> Result<Output, JobError> {
            ctx.check()?;
            let rendered = job
                .take()
                .ok_or_else(|| JobError::Failed("the photo's render was already used".into()))?
                .run()
                .rendered
                .map_err(|e| JobError::Failed(format!("the photo could not be rendered: {e}")))?;
            ctx.check()?;
            let img = &rendered.image;
            let mut rgb = Vec::with_capacity(img.data.len() * 3);
            for px in &img.data {
                rgb.extend_from_slice(&[px[0], px[1], px[2]]);
            }
            let rgb = RgbImage::new(img.width as u32, img.height as u32, rgb).map_err(JobError::Failed)?;
            let embeddings = enc.encode_images(&orient::views(&rgb), ctx)?;
            Ok(Box::new(ViewsOutput { id, shown, embeddings }))
        })
        .name(name)
        .priority(priority);
        self.ml_submit(work).is_ok()
    }

    /// A finished `orient:` job from the AI queue (`crate::ml`'s drain hands them here).
    pub(crate) fn orient_finished(&mut self, id: PhotoId, outcome: Outcome<Output>) {
        let result: Result<ViewsOutput, String> = match outcome {
            Outcome::Done(any) => any.downcast::<ViewsOutput>().map(|b| *b).map_err(|_| "an answer of the wrong kind".to_string()),
            Outcome::Failed(why) => Err(why),
            Outcome::Cancelled => Err("cancelled".into()),
        };
        if let Some(fit) = self.ml.orient.fit.as_mut()
            && fit.waiting.remove(&id)
        {
            match result {
                Ok(out) if out.embeddings.len() == orient::CLASSES => fit.examples.push(Example { group: out.id.0, views: out.embeddings }),
                Ok(_) => fit.failed += 1,
                Err(why) => {
                    fit.failed += 1;
                    log::warn!("ml: photo {} left out of the orientation fit: {why}", id.0);
                }
            }
            let done = fit.total - fit.waiting.len();
            fit.row.progress(done as u64, fit.total as u64);
            return;
        }
        let Some(run) = self.ml.orient.run.as_mut() else { return };
        if !run.waiting.remove(&id) {
            return;
        }
        match result.and_then(|out| orient::decide(&run.head, &out.embeddings).map(|v| (out, v))) {
            Ok((out, verdict)) => run.found.push(Found { id: out.id, shown: out.shown, verdict }),
            Err(why) => run.failed.push((id, why)),
        }
        let done = run.total - run.waiting.len();
        run.row.progress(done as u64, run.total as u64);
        if let Some(name) = self.catalog.photo(id).map(|p| p.file_name.clone()) {
            run.row.detail(&name);
        }
        if run.waiting.is_empty() {
            self.finish_orient_run();
        }
    }

    /// Called on every drain: a run whose ✕ was clicked stops here (its waiting jobs cancelled, nothing turned).
    pub(crate) fn orient_check_cancel(&mut self) {
        let cancelled = self.ml.orient.run.as_ref().is_some_and(|r| r.cancel.load(Ordering::Relaxed));
        if !cancelled {
            return;
        }
        if let Some(run) = self.ml.orient.run.take() {
            if let Some(q) = self.ml.queue.as_ref() {
                for id in &run.waiting {
                    q.cancel(&format!("{KEY}{}", id.0));
                }
            }
            self.ml.orient.last = Some(json!({"cancelled": true, "checked": run.total - run.waiting.len(), "total": run.total, "turned": []}));
        }
    }

    /// Turns the run's photos whose answer is sure enough (still shown as they were when they were looked at) in one
    /// undoable step.
    fn finish_orient_run(&mut self) {
        let Some(run) = self.ml.orient.run.take() else { return };
        let mut ops = Vec::new();
        let mut results = Vec::new();
        let mut uncertain = 0usize;
        for f in &run.found {
            let sure = f.verdict.confidence >= run.min_confidence;
            let wants_turn = f.verdict.correction != 0;
            if !sure && wants_turn {
                uncertain += 1;
            }
            let mut applied = false;
            if run.apply
                && sure
                && wants_turn
                && let Some(before) = self.develop_of(f.id)
                // turned by hand meanwhile: the answer is about how it was
                && before.orientation == f.shown
            {
                let d = crate::cmd::library::turned(self, f.id, &before, true, f.verdict.correction);
                if let Some(op) = self.develop_op(f.id, d, "Auto Orient") {
                    ops.push(op);
                    applied = true;
                }
            }
            results.push(verdict_json(f, applied));
        }
        let turned = ops.len();
        if turned > 0 {
            if let Err(e) = self.commit("Auto Orient", Op::Batch { ops }) {
                log::warn!("ml: the orientation changes could not be saved: {e}");
                for r in &mut results {
                    if let Some(a) = r.get_mut("applied") {
                        *a = json!(false);
                    }
                }
            } else {
                self.ml.orient.turned_unseen = self.ml.orient.turned_unseen.saturating_add(turned);
            }
        }
        let failed: Vec<Value> = run.failed.iter().map(|(id, why)| json!({"id": id.0, "error": why})).collect();
        self.ml.orient.last = Some(json!({
            "done": true,
            "checked": run.found.len(),
            "total": run.total,
            "turnedCount": turned,
            "uncertain": uncertain,
            "minConfidence": run.min_confidence,
            "photos": results,
            "failed": failed,
        }));
    }

    /// `photo.autoOrient {ids?, minConfidence?, apply?: true, wait?: false, budgetMs?: 60000}`.
    pub(crate) fn photo_auto_orient(&mut self, p: &Value) -> crate::Result<Value> {
        const C: &str = "photo.autoOrient";
        let targets = self.targets(p).map_err(|e| bad(C, e.to_string()))?;
        if targets.is_empty() {
            return Err(bad(C, "no photos are selected"));
        }
        let min_confidence = match p.get("minConfidence") {
            None | Some(Value::Null) => None,
            Some(v) => match v.as_f64() {
                Some(c) if (0.0..=1.0).contains(&c) => Some(c as f32),
                _ => return Err(bad(C, "`minConfidence` must be a number from 0 to 1")),
            },
        };
        let apply = p.get("apply").and_then(Value::as_bool).unwrap_or(true);
        let wait = p.get("wait").and_then(Value::as_bool).unwrap_or(false);
        let budget = p.get("budgetMs").and_then(Value::as_u64).unwrap_or(WAIT_MS).clamp(1, WAIT_MAX_MS);
        if self.ml.orient.busy() {
            return Err(bad(C, "orientation detection is already running; wait for it, or stop it in the activity stack"));
        }
        let enc = self.ensure_ml_encoder(true).map_err(|e| bad(C, e))?;
        let (head, _) = self.orientation_head(&*enc).map_err(|e| bad(C, e))?;
        let min_confidence = min_confidence.unwrap_or(head.min_confidence);

        let cancel = Arc::new(AtomicBool::new(false));
        let row = self.activity.start(ROW_KIND, ROW_LABEL, Cancel::Flag(Arc::clone(&cancel)));
        let priority = if wait { Priority::Interactive } else { Priority::Background };
        let mut waiting = HashSet::new();
        let mut skipped = 0usize;
        for id in targets {
            if waiting.contains(&id) {
                continue;
            }
            if self.queue_orient_views(&enc, id, priority) {
                waiting.insert(id);
            } else {
                skipped += 1;
            }
        }
        if waiting.is_empty() {
            return Err(bad(C, "none of the photos could be looked at"));
        }
        let total = waiting.len();
        row.progress(0, total as u64);
        self.ml.orient.last = None;
        self.ml.orient.run = Some(Run { waiting, total, apply, min_confidence, head, found: Vec::new(), failed: Vec::new(), row, cancel });
        if wait {
            self.wait_orient(Duration::from_millis(budget), |s| s.ml.orient.run.is_none());
        }
        Ok(match (&self.ml.orient.run, &self.ml.orient.last) {
            (None, Some(last)) => {
                let mut v = last.clone();
                if let Some(o) = v.as_object_mut() {
                    o.insert("skipped".into(), json!(skipped));
                }
                v
            }
            (Some(run), _) => {
                json!({"done": false, "queued": run.total, "checked": run.total - run.waiting.len(), "skipped": skipped, "minConfidence": min_confidence})
            }
            (None, None) => json!({"done": false, "queued": 0, "skipped": skipped}),
        })
    }

    /// Drains the AI queue until `done` (or `budget` runs out).
    fn wait_orient(&mut self, budget: Duration, done: impl Fn(&Session) -> bool) {
        let started = web_time::Instant::now();
        while started.elapsed() < budget {
            self.drain_ml_results();
            if done(self) {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        self.drain_ml_results();
    }

    /// `ml.orientation.fit {ids?, out?, steps?: 600, budgetMs?}`: fits the classifier on the library's photos (or
    /// `ids`) as they are shown, taken to be upright, and saves it where `photo.autoOrient` finds it (or at `out`).
    pub(crate) fn ml_orientation_fit(&mut self, p: &Value) -> crate::Result<Value> {
        const C: &str = "ml.orientation.fit";
        let ids: Vec<PhotoId> = match p.get("ids") {
            None | Some(Value::Null) => self.catalog.photos().filter(|ph| ph.in_library()).map(|ph| ph.id).collect(),
            Some(_) => self.targets(p).map_err(|e| bad(C, e.to_string()))?,
        };
        let steps = p.get("steps").and_then(Value::as_u64).unwrap_or(600).clamp(10, 20_000) as usize;
        let budget = p.get("budgetMs").and_then(Value::as_u64).unwrap_or(FIT_WAIT_MS).clamp(1, FIT_WAIT_MAX_MS);
        if self.ml.orient.busy() {
            return Err(bad(C, "orientation detection is already running"));
        }
        let enc = self.ensure_ml_encoder(true).map_err(|e| bad(C, e))?;
        let m = enc.manifest().clone();
        let out = match p.get("out").and_then(Value::as_str) {
            Some(path) => PathBuf::from(path),
            None => fitted_head_path(self.ml_models_dir.as_deref().ok_or_else(|| bad(C, "no AI models folder is configured"))?, &m.id),
        };
        let row = self.activity.start(ROW_KIND, FIT_ROW_LABEL, Cancel::No);
        let mut waiting = HashSet::new();
        for id in ids {
            if !waiting.contains(&id) && self.queue_orient_views(&enc, id, Priority::Interactive) {
                waiting.insert(id);
            }
        }
        let total = waiting.len();
        row.progress(0, total as u64);
        self.ml.orient.fit = Some(Fit { waiting, total, examples: Vec::new(), failed: 0, row });
        self.wait_orient(Duration::from_millis(budget), |s| s.ml.orient.fit.as_ref().is_none_or(|f| f.waiting.is_empty()));
        let fit = self.ml.orient.fit.take().ok_or_else(|| bad(C, "the fit was lost"))?;
        if let Some(q) = self.ml.queue.as_ref() {
            for id in &fit.waiting {
                q.cancel(&format!("{KEY}{}", id.0));
            }
        }
        fit.row.detail("Fitting");
        let source = format!("{} photos in {}", fit.examples.len(), self.library.as_ref().map(|l| l.dir.display().to_string()).unwrap_or_default());
        let opts = FitOptions { steps, ..FitOptions::default() };
        let head = orient::fit(&fit.examples, &m.tag(), m.embed_dim as usize, &source, &opts).map_err(|e| bad(C, e))?;
        let bytes = head.to_json().map_err(|e| bad(C, e))?;
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| bad(C, format!("{}: {e}", parent.display())))?;
        }
        crate::denoise::write_atomic(&out, &bytes).map_err(|e| bad(C, e))?;
        Ok(json!({
            "path": out.display().to_string(),
            "embedder": head.embedder,
            "photos": fit.examples.len(),
            "failed": fit.failed,
            "notReached": fit.waiting.len(),
            "minConfidence": head.min_confidence,
            "report": head.report,
        }))
    }

    /// `ml.orientation.status {}`.
    pub(crate) fn ml_orientation_status(&mut self, _p: &Value) -> crate::Result<Value> {
        let head = match self.ensure_ml_encoder(false) {
            Ok(enc) => match self.orientation_head(&*enc) {
                Ok((h, origin)) => json!({
                    "available": true,
                    "origin": origin,
                    "embedder": h.embedder,
                    "minConfidence": h.min_confidence,
                    "source": h.source,
                    "report": h.report,
                }),
                Err(why) => json!({"available": false, "reason": why}),
            },
            Err(why) => json!({"available": false, "reason": why}),
        };
        let running = self.ml.orient.run.as_ref().map(|r| json!({"checked": r.total - r.waiting.len(), "total": r.total}));
        Ok(json!({"classifier": head, "running": running, "last": self.ml.orient.last}))
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use lcc_ml::{Config, ModelManifest, Preference};

    use super::*;

    /// A stand-in for CLIP that sees only which edge of the picture is bright: axis `c` is the brightness of the
    /// edge a bright top lands on when the picture is turned `c` quarter turns clockwise (top, right, bottom, left).
    struct EdgeEncoder(ModelManifest);

    impl Encoder for EdgeEncoder {
        fn manifest(&self) -> &ModelManifest {
            &self.0
        }
        fn encode_images(&self, batch: &[RgbImage], _ctx: &Ctx) -> Result<Vec<Vec<f32>>, JobError> {
            Ok(batch
                .iter()
                .map(|img| {
                    let (w, h) = (img.width as usize, img.height as usize);
                    let mut sums = [0f32; 4];
                    let mut counts = [0f32; 4];
                    for y in 0..h {
                        for x in 0..w {
                            let o = (y * w + x) * 3;
                            let l = img.rgb[o..o + 3].iter().map(|&v| f32::from(v)).sum::<f32>() / 765.0;
                            for (edge, on) in [y < h / 4, x >= w - w / 4, y >= h - h / 4, x < w / 4].into_iter().enumerate() {
                                if on {
                                    sums[edge] += l;
                                    counts[edge] += 1.0;
                                }
                            }
                        }
                    }
                    let means: Vec<f32> = sums.iter().zip(counts).map(|(s, c)| s / c.max(1.0)).collect();
                    let avg = means.iter().sum::<f32>() / 4.0;
                    let mut e: Vec<f32> = means.iter().map(|m| m - avg).collect();
                    let n = e.iter().map(|v| v * v).sum::<f32>().sqrt();
                    if n > 1e-6 {
                        e.iter_mut().for_each(|v| *v /= n);
                    } else {
                        e = vec![0.5; 4];
                    }
                    e
                })
                .collect())
        }
        fn encode_text(&self, _text: &str, _ctx: &Ctx) -> Result<Vec<f32>, JobError> {
            Err(JobError::Failed("no text".into()))
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lc-orient-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A landscape picture with a bright band along its top (a sky), turned `turns` quarter turns clockwise.
    fn write_sky_png(path: &Path, turns: u8, shade: u8) {
        let (w, h) = (48usize, 32usize);
        let mut rgb = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let v = if y < h / 3 { 235 } else { shade.saturating_add((x % 7) as u8 * 3) };
                rgb.extend_from_slice(&[v, v, v]);
            }
        }
        let img = orient::rotate(&RgbImage::new(w as u32, h as u32, rgb).unwrap(), turns);
        let data = img.rgb.chunks(3).map(|p| [p[0], p[1], p[2], 255]).collect();
        let out = lcc_raster::Rgba8 { width: img.width as usize, height: img.height as usize, data };
        let bytes = lcc_codecs::encode_png(&lcc_codecs::EncodeImage::rgba8(&out), &lcc_codecs::EncodeMeta::default()).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    /// A library with `photos` (`(name, turns)`), the synthetic CLIP installed and [`EdgeEncoder`] standing in for it.
    fn session_with(tag: &str, photos: &[(&str, u8)]) -> (Session, PathBuf, String) {
        let d = temp_dir(tag);
        let models_dir = d.join("ml-models");
        let src = d.join("clip-src");
        lcc_ml::Clip::write_synthetic_dir(&src, "test-clip").unwrap();
        let mut s = Session::new().with_fs();
        s.ml_models_dir = Some(models_dir.clone());
        s.open_library(d.join("lib"), false).unwrap();
        let paths: Vec<String> = photos
            .iter()
            .enumerate()
            .map(|(i, (name, turns))| {
                let p = d.join("photos").join(name);
                write_sky_png(&p, *turns, 30 + i as u8 * 4);
                p.to_string_lossy().to_string()
            })
            .collect();
        s.execute("library.import", &json!({"paths": paths})).unwrap();
        s.execute("ml.models.install", &json!({"path": src.to_string_lossy(), "acknowledged": true})).unwrap();
        s.ml_queue_with(Config { preference: Preference::Cpu, ..Config::default() }, Box::new(|| Err("cpu test".into())));
        let manifest = ModelManifest::read_dir(&models_dir.join("test-clip")).unwrap();
        let tag = manifest.tag();
        s.ml.encoder = Some((tag.clone(), Arc::new(EdgeEncoder(manifest))));
        (s, models_dir, tag)
    }

    fn id_of(s: &Session, name: &str) -> PhotoId {
        s.catalog.photos().find(|p| p.file_name == name).unwrap().id
    }

    fn orientation(s: &Session, name: &str) -> Orientation {
        s.develop_of(id_of(s, name)).unwrap().orientation
    }

    fn write_head(models_dir: &Path, tag: &str) {
        let w = (0..4).map(|c| (0..4).map(|i| if i == c { 8.0 } else { 0.0 }).collect()).collect();
        let head = Head {
            version: orient::HEAD_VERSION,
            embedder: tag.to_string(),
            embed_dim: 4,
            weights: w,
            bias: vec![0.0; 4],
            min_confidence: 0.8,
            source: "test".into(),
            report: None,
        };
        std::fs::write(fitted_head_path(models_dir, "test-clip"), head.to_json().unwrap()).unwrap();
    }

    #[test]
    fn without_a_classifier_auto_orient_says_how_to_get_one() {
        let (mut s, _, _) = session_with("nohead", &[("a.png", 0)]);
        let err = s.execute("photo.autoOrient", &json!({"ids": [id_of(&s, "a.png").0], "wait": true})).unwrap_err().to_string();
        assert!(err.contains("ml.orientation.fit"), "{err}");
        let st = s.execute("ml.orientation.status", &json!({})).unwrap();
        assert_eq!(st["classifier"]["available"], false, "{st}");
        // a head for another model is not used
        let (mut s, models, _) = session_with("otherhead", &[("a.png", 0)]);
        write_head(&models, "some-other-model@0");
        assert!(s.execute("photo.autoOrient", &json!({"ids": [id_of(&s, "a.png").0], "wait": true})).is_err());
        // nor is a damaged one
        std::fs::write(fitted_head_path(&models, "test-clip"), b"{not json").unwrap();
        assert!(s.execute("photo.autoOrient", &json!({"ids": [id_of(&s, "a.png").0], "wait": true})).is_err());
    }

    #[test]
    fn auto_orient_turns_sideways_photos_upright_in_one_undoable_step() {
        let (mut s, models, tag) = session_with("apply", &[("upright.png", 0), ("cw.png", 1), ("down.png", 2), ("ccw.png", 3)]);
        write_head(&models, &tag);
        let ids: Vec<u64> = ["upright.png", "cw.png", "down.png", "ccw.png"].iter().map(|n| id_of(&s, n).0).collect();
        // a report only changes nothing
        let r = s.execute("photo.autoOrient", &json!({"ids": ids, "wait": true, "apply": false})).unwrap();
        assert_eq!(r["done"], true, "{r}");
        assert_eq!(r["turnedCount"], 0, "{r}");
        let turn_of = |r: &Value, name: &str, s: &Session| {
            r["photos"].as_array().unwrap().iter().find(|p| p["id"] == id_of(s, name).0).map(|p| p["turn"].clone()).unwrap()
        };
        assert_eq!(turn_of(&r, "upright.png", &s), 0, "{r}");
        assert_eq!(turn_of(&r, "cw.png", &s), 270, "{r}");
        assert_eq!(turn_of(&r, "down.png", &s), 180, "{r}");
        assert_eq!(turn_of(&r, "ccw.png", &s), 90, "{r}");
        assert_eq!(orientation(&s, "cw.png"), Orientation::Normal);

        let r = s.execute("photo.autoOrient", &json!({"ids": ids, "wait": true})).unwrap();
        assert_eq!(r["turnedCount"], 3, "{r}");
        assert_eq!(orientation(&s, "upright.png"), Orientation::Normal);
        assert_eq!(orientation(&s, "cw.png"), Orientation::Normal.rotated(false));
        assert_eq!(orientation(&s, "down.png"), Orientation::Rotate180);
        assert_eq!(orientation(&s, "ccw.png"), Orientation::Normal.rotated(true));
        assert!(s.activity.list().iter().all(|t| t.kind != ROW_KIND), "the row goes when the run is done");
        let st = s.execute("ml.orientation.status", &json!({})).unwrap();
        assert_eq!(st["classifier"]["origin"], "fitted", "{st}");
        assert_eq!(st["last"]["turnedCount"], 3, "{st}");

        // once upright, a second run leaves them alone
        let r = s.execute("photo.autoOrient", &json!({"ids": ids, "wait": true})).unwrap();
        assert_eq!(r["turnedCount"], 0, "{r}");

        // one step undoes all three
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(orientation(&s, "cw.png"), Orientation::Normal);
        assert_eq!(orientation(&s, "down.png"), Orientation::Normal);
        assert_eq!(orientation(&s, "ccw.png"), Orientation::Normal);
    }

    #[test]
    fn an_uncertain_photo_and_hostile_params_change_nothing() {
        let (mut s, models, tag) = session_with("unsure", &[("cw.png", 1)]);
        write_head(&models, &tag);
        let id = id_of(&s, "cw.png").0;
        // a threshold above anything the pooled answer reaches
        let r = s.execute("photo.autoOrient", &json!({"ids": [id], "wait": true, "minConfidence": 1.0})).unwrap();
        assert_eq!(r["turnedCount"], 0, "{r}");
        assert_eq!(r["uncertain"], 1, "{r}");
        assert_eq!(orientation(&s, "cw.png"), Orientation::Normal);
        for bad in [json!({"ids": [id], "minConfidence": 2}), json!({"ids": [id], "minConfidence": "x"}), json!({"ids": ["x"]}), json!({"ids": []})] {
            assert!(s.execute("photo.autoOrient", &bad).is_err(), "{bad}");
        }
        // a photo that isn't there is skipped
        let r = s.execute("photo.autoOrient", &json!({"ids": [id, 999_999], "wait": true})).unwrap();
        assert_eq!(r["skipped"], 1, "{r}");
    }

    #[test]
    fn fitting_on_upright_photos_makes_a_classifier_auto_orient_uses() {
        let names: Vec<String> = (0..20).map(|i| format!("p{i:02}.png")).collect();
        let photos: Vec<(&str, u8)> = names.iter().map(|n| (n.as_str(), 0)).collect();
        let (mut s, models, tag) = session_with("fit", &photos);
        let r = s.execute("ml.orientation.fit", &json!({"steps": 200})).unwrap();
        assert_eq!(r["photos"], 20, "{r}");
        assert_eq!(r["embedder"], tag.as_str(), "{r}");
        assert!(r["report"]["photoAccuracy"].as_f64().unwrap() > 0.9, "{r}");
        let path = fitted_head_path(&models, "test-clip");
        assert!(Head::from_json(&std::fs::read(&path).unwrap()).is_ok());

        // turn one by hand, and the fitted classifier puts it back
        let id = id_of(&s, "p03.png");
        s.execute("photo.rotateRight", &json!({"ids": [id.0]})).unwrap();
        let r = s.execute("photo.autoOrient", &json!({"ids": [id.0], "wait": true})).unwrap();
        assert_eq!(r["turnedCount"], 1, "{r}");
        assert_eq!(orientation(&s, "p03.png"), Orientation::Normal);
    }
}
