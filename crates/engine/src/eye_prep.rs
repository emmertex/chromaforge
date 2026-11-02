//! `eyes.analyze`: find the eyes of many photos ahead of culling — a folder browsed in the
//! Import tab, the photos asked for, or all in view — so the eye focus strip shows them at once.
//! Each photo's result is kept in the preview cache ([`crate::eye_store`]); a photo kept there
//! already is skipped. It runs in the background (a row in the activity stack, cancellable),
//! one photo at a time, waiting while the photo on screen is being made; `wait: true` runs it
//! there and then (the CLI's prep run, tests).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use lcc_catalog::{PhotoId, Source};
use serde_json::{Value, json};

use crate::eyes::EyeJob;
use crate::{Result, Session};

const C: &str = "eyes.analyze";
/// The activity row's label (translated by the frontend).
pub const LABEL: &str = "Finding eyes for culling";

/// A run in the background, and what it has done so far.
#[derive(Default)]
pub struct EyePrep {
    pub cancel: Arc<AtomicBool>,
    pub finished: Arc<AtomicBool>,
    pub photos: u64,
    pub kept: Arc<AtomicU64>,
    pub found: Arc<AtomicU64>,
    pub failed: Arc<AtomicU64>,
}

impl EyePrep {
    fn json(&self) -> Value {
        let finished = self.finished.load(Ordering::Relaxed);
        json!({
            "active": !finished,
            "done": finished,
            "photos": self.photos,
            "kept": self.kept.load(Ordering::Relaxed),
            "analyzed": self.found.load(Ordering::Relaxed),
            "failed": self.failed.load(Ordering::Relaxed),
            "cancelled": self.cancel.load(Ordering::Relaxed),
        })
    }
}

/// The photos of `p`: those in folder `path` (with its subfolders unless `subfolders: false`),
/// else `ids`, else the selection when more than one photo is selected, else all in view.
fn photos(s: &mut Session, p: &Value) -> Result<Vec<PhotoId>> {
    if let Some(path) = p.get("path").and_then(Value::as_str) {
        let dir = std::path::absolute(std::path::Path::new(path)).map_err(|e| crate::cmd::bad(C, e.to_string()))?;
        let dir = dir.to_string_lossy().trim_end_matches(['/', '\\']).to_string();
        let deep = p.get("subfolders").and_then(Value::as_bool).unwrap_or(true);
        let mut v: Vec<(String, PhotoId)> = s
            .catalog
            .photos()
            .filter(|ph| !ph.deleted)
            .filter_map(|ph| match &ph.source {
                Source::File { path } if lcc_catalog::query::in_folder(path, &dir, deep) => Some((path.clone(), ph.id)),
                _ => None,
            })
            .collect();
        v.sort();
        return Ok(v.into_iter().map(|x| x.1).collect());
    }
    if p.get("ids").is_some() || s.selection.ids.len() > 1 { s.targets(p) } else { Ok(s.visible_cloned()) }
}

/// `eyes.analyze`.
pub fn analyze(s: &mut Session, p: &Value) -> Result<Value> {
    let running = s.eye_prep.as_ref().filter(|r| !r.finished.load(Ordering::Relaxed));
    if p.get("status").and_then(Value::as_bool).unwrap_or(false) {
        return Ok(s.eye_prep.as_ref().map_or_else(|| json!({"active": false, "done": false}), |r| r.json()));
    }
    if p.get("cancel").and_then(Value::as_bool).unwrap_or(false) {
        if let Some(r) = running {
            r.cancel.store(true, Ordering::Relaxed);
        }
        return Ok(json!({"cancelled": running.is_some()}));
    }
    if running.is_some() {
        return Err(crate::cmd::bad(C, "a run is in progress already (`{\"cancel\": true}` stops it)"));
    }
    let ids = photos(s, p)?;
    let jobs: Vec<EyeJob> = ids.iter().filter_map(|id| s.eye_job(*id)).collect();
    let dir = s.eye_model_dir().ok_or_else(|| {
        crate::cmd::bad(C, "finding eyes needs the MediaPipe faces model: download it from the Masking panel or with `ai.models.download {\"id\": \"face\", \"acknowledged\": true}`")
    })?;
    let wait = p.get("wait").and_then(Value::as_bool).unwrap_or(false);
    // photos looked at side by side (each lane loads the model once): a prep run uses the computer,
    // a run beside someone culling one lane
    let lanes = p
        .get("lanes")
        .and_then(Value::as_u64)
        .map(|n| n.clamp(1, MAX_LANES as u64) as usize)
        .unwrap_or(if wait { default_lanes() } else { 1 })
        .min(jobs.len().max(1));
    let run = EyePrep { photos: jobs.len() as u64, ..Default::default() };
    let count = Counter { kept: run.kept.clone(), found: run.found.clone(), failed: run.failed.clone(), seen: Arc::default(), total: run.photos };
    let lanes = split(jobs, lanes);
    if wait {
        let t0 = web_time::Instant::now();
        std::thread::scope(|sc| {
            let mut inline = Vec::new();
            let mut handles = Vec::new();
            for lane in &lanes {
                let (dir, cancel, count) = (&dir, &run.cancel, count.clone());
                let job = move || {
                    judge(dir, lane, cancel, false, |_, r, cached| count.add(&r, cached));
                };
                match std::thread::Builder::new().name("lightcraft-eye-prep".into()).spawn_scoped(sc, job) {
                    Ok(h) => handles.push(h),
                    Err(_) => inline.push(lane),
                }
            }
            for lane in inline {
                let count = count.clone();
                judge(&dir, lane, &run.cancel, false, |_, r, cached| count.add(&r, cached));
            }
            for h in handles {
                if h.join().is_err() {
                    log::error!("eyes.analyze: a lane stopped unexpectedly");
                }
            }
        });
        run.finished.store(true, Ordering::Relaxed);
        let mut out = run.json();
        out["ms"] = json!(t0.elapsed().as_millis() as u64);
        out["lanes"] = json!(lanes.len());
        return Ok(out);
    }
    let task = s.activity.start_batch("eyes", LABEL, crate::activity::Cancel::Flag(run.cancel.clone()));
    task.progress(0, run.photos);
    let (flag, finished) = (run.cancel.clone(), run.finished.clone());
    std::thread::Builder::new()
        .name("lightcraft-eye-prep".into())
        .spawn(move || {
            let mut handles = Vec::new();
            let mut inline = Vec::new();
            for lane in lanes {
                let (dir, flag, count, row) = (dir.clone(), flag.clone(), count.clone(), task.handle());
                let job = {
                    let lane = lane.clone();
                    move || {
                        judge(&dir, &lane, &flag, true, |_, r, cached| {
                            count.add(&r, cached);
                            match &r {
                                Ok(_) => row.item_done(),
                                Err(e) => row.item_failed(e),
                            }
                        });
                    }
                };
                match std::thread::Builder::new().name("lightcraft-eye-prep".into()).spawn(job) {
                    Ok(h) => handles.push(h),
                    Err(_) => inline.push(lane),
                }
            }
            for lane in inline {
                judge(&dir, &lane, &flag, true, |_, r, cached| {
                    count.add(&r, cached);
                    match &r {
                        Ok(_) => task.item_done(),
                        Err(e) => task.item_failed(e),
                    }
                });
            }
            for h in handles {
                if h.join().is_err() {
                    log::error!("eyes.analyze: a lane stopped unexpectedly");
                }
            }
            finished.store(true, Ordering::Relaxed);
            drop(task);
        })
        .map_err(|e| crate::cmd::bad(C, format!("could not start ({e})")))?;
    let out = run.json();
    s.eye_prep = Some(Arc::new(run));
    Ok(out)
}

/// One lane's photos looked at ([`crate::eye_cull::judge_counted`]): work done ahead of culling,
/// run as background work, behind the photo on screen.
fn judge(
    dir: &std::path::Path,
    lane: &[EyeJob],
    cancel: &AtomicBool,
    polite: bool,
    each: impl FnMut(PhotoId, std::result::Result<Arc<crate::eyes::EyeReport>, String>, bool),
) {
    let _ = crate::eye_cull::judge_counted(dir, lane, cancel, polite, each);
}

/// The most lanes a run takes.
const MAX_LANES: usize = 8;

/// Lanes of a run that waits (a prep run): an eighth of the computer's threads, 1–4 (each lane
/// holds one full-size camera JPEG at a time, ~200 MB for 45 MP).
fn default_lanes() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get() / 8).clamp(1, 4)
}

/// `jobs` dealt out to `lanes` lanes in turn, so the first photos are looked at first.
fn split(jobs: Vec<EyeJob>, lanes: usize) -> Vec<Vec<EyeJob>> {
    let lanes = lanes.max(1);
    let mut out: Vec<Vec<EyeJob>> = (0..lanes).map(|_| Vec::new()).collect();
    for (i, j) in jobs.into_iter().enumerate() {
        if let Some(lane) = out.get_mut(i % lanes) {
            lane.push(j);
        }
    }
    out.retain(|l| !l.is_empty());
    out
}

/// What a run counts, shared by its lanes.
#[derive(Clone)]
struct Counter {
    kept: Arc<AtomicU64>,
    found: Arc<AtomicU64>,
    failed: Arc<AtomicU64>,
    seen: Arc<AtomicU64>,
    total: u64,
}

impl Counter {
    fn add(&self, r: &std::result::Result<Arc<crate::eyes::EyeReport>, String>, from_cache: bool) {
        match r {
            Ok(_) if from_cache => self.kept.fetch_add(1, Ordering::Relaxed),
            Ok(_) => self.found.fetch_add(1, Ordering::Relaxed),
            Err(_) => self.failed.fetch_add(1, Ordering::Relaxed),
        };
        let n = self.seen.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        if n.is_multiple_of(100) {
            log::info!("eyes.analyze: {n} of {} photos", self.total);
        }
    }
}
