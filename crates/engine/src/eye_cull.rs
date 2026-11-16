//! Assisted Culling by the eyes: reject the photos where a main subject has their eyes closed or
//! too soft to keep (`photo.rejectEyes`). The eyes are judged as the culling strip judges them
//! ([`crate::eyes`]), on the camera's full-size JPEG; that takes about a second a photo, so a
//! wedding's worth runs in the background (a row in the activity stack, cancellable) and the
//! rejects land in one undo step when it is done (`photo.rejectEyesPump`, called every frame by the
//! app). `wait: true` runs it there and then (the CLI, MCP, tests).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex, PoisonError};

use lcc_catalog::{Flag, Op, PhotoId};
use serde_json::{Value, json};

use crate::eyes::{EyeJob, EyeReport};
use crate::{Result, Session};

const C: &str = "photo.rejectEyes";

/// What makes a photo a reject.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EyeRules {
    /// A main subject's eyes are closed.
    pub closed: bool,
    /// A main subject's eyes are blurry (the strip's red).
    pub blurry: bool,
    /// With `blurry`: soft eyes (the strip's amber) count as blurry too (the stricter threshold).
    pub soft: bool,
    /// Only the main subjects count (faces at least half as wide as the largest); else everyone.
    pub main_only: bool,
}

/// Why one photo is a reject: (people with closed eyes, people with blurry eyes) under `rules`.
pub fn reasons(r: &EyeReport, rules: EyeRules) -> (usize, usize) {
    use crate::eyes::{Focus, Lids};
    let people = r.eyes.iter().filter(|e| e.main || !rules.main_only);
    let closed = if rules.closed { people.clone().filter(|e| e.lids == Lids::Closed).count() } else { 0 };
    let too_soft = |f: Focus| f == Focus::Blurry || (rules.soft && f == Focus::Soft);
    let blurry = if rules.blurry { people.filter(|e| !matches!(e.lids, Lids::Closed | Lids::Hidden) && too_soft(e.focus)).count() } else { 0 };
    (closed, blurry)
}

/// A background run: its results arrive on `rx`; `cancel` stops it between photos.
pub struct EyeCullRun {
    rx: Mutex<Receiver<Done>>,
    cancel: Arc<AtomicBool>,
    rules: EyeRules,
    total: usize,
}

/// The run's outcome: each photo's report (or why it has none), then the end.
enum Done {
    Photo(PhotoId, std::result::Result<Arc<EyeReport>, String>),
    End { cancelled: bool },
}

/// The model folder, or an error that says how to get it.
fn model_dir(s: &mut Session) -> Result<std::path::PathBuf> {
    s.eye_model_dir().ok_or_else(|| {
        crate::cmd::bad(C, "finding eyes needs the MediaPipe faces model: download it from the Masking panel or with `ai.models.download {\"id\": \"face\", \"acknowledged\": true}`")
    })
}

/// Judge `jobs` one after the other, sending each result; stops early when `cancel` is set.
pub(crate) fn judge(
    model_dir: &std::path::Path,
    jobs: &[EyeJob],
    cancel: &AtomicBool,
    mut each: impl FnMut(PhotoId, std::result::Result<Arc<EyeReport>, String>),
) -> bool {
    judge_counted(model_dir, jobs, cancel, false, |id, r, _| each(id, r))
}

/// [`judge`], telling `each` whether the result was read from the preview cache. A photo whose
/// eyes are kept in the preview cache ([`crate::eye_store`]) is read from there, the others are
/// looked at and kept. The model is loaded for the first photo that needs it. `polite`: each photo looked at waits while the photo on screen is being made or
/// something on screen lacks its first picture (a background run beside someone culling).
pub(crate) fn judge_counted(
    model_dir: &std::path::Path,
    jobs: &[EyeJob],
    cancel: &AtomicBool,
    polite: bool,
    mut each: impl FnMut(PhotoId, std::result::Result<Arc<EyeReport>, String>, bool),
) -> bool {
    #[cfg(feature = "sam")]
    let mut model: Option<std::result::Result<(lcc_segment::people::FaceModel, Option<lcc_segment::people::PeopleModel>), String>> = None;
    for j in jobs {
        if cancel.load(Ordering::Relaxed) {
            return true;
        }
        let kept = j.store.as_ref().and_then(|st| Some((st.cache.clone(), st.key(model_dir)?)));
        if let Some((cache, key)) = &kept
            && let Some(r) = crate::eye_store::read(cache, *key, j.photo)
        {
            each(j.photo, Ok(Arc::new(r)), true);
            continue;
        }
        if polite {
            let stop = || cancel.load(Ordering::Relaxed);
            if crate::eyes::wait_for_foreground(cancel) || crate::eyes::wait_for_pictures(&stop, std::time::Duration::from_secs(5)) {
                return true;
            }
        }
        #[cfg(feature = "sam")]
        let r = {
            let m = model.get_or_insert_with(|| {
                crate::guard::catch("loading the faces model", || {
                    lcc_segment::people::FaceModel::load(model_dir).map(|m| (m, crate::eyes::people_beside(model_dir))).map_err(|e| e.to_string())
                })
                .and_then(|r| r)
            });
            match m {
                Err(e) => Err(e.clone()),
                Ok((model, people)) => crate::guard::catch("finding eyes", || {
                    let img =
                        crate::memory::in_background(|| crate::eyes::load(&j.source)).ok_or_else(|| "the picture could not be read".to_string())?;
                    let r = crate::eyes::analyze(model, people.as_ref(), &img, j.photo).map(Arc::new);
                    drop(img);
                    crate::memory::release();
                    r
                })
                .and_then(|r| r),
            }
        };
        #[cfg(not(feature = "sam"))]
        let r: std::result::Result<Arc<EyeReport>, String> = Err("eye focus needs a build with AI masks".to_string());
        if let (Ok(r), Some((cache, key))) = (&r, &kept) {
            crate::eye_store::write(cache, *key, r);
        }
        each(j.photo, r, false);
    }
    false
}

/// Flag the rejects among `reports` in one undo step (a photo the user picked is never rejected,
/// one already rejected stays as it is) and say what was done.
fn apply(s: &mut Session, rules: EyeRules, reports: &[(PhotoId, std::result::Result<Arc<EyeReport>, String>)], cancelled: bool) -> Result<Value> {
    let mut ops = Vec::new();
    let (mut closed_n, mut blurry_n, mut no_faces, mut failed) = (0usize, 0usize, 0usize, Vec::new());
    for (id, r) in reports {
        let r = match r {
            Ok(r) => r,
            Err(e) => {
                failed.push(json!([id.0, e]));
                continue;
            }
        };
        if r.eyes.is_empty() {
            no_faces += 1;
            continue;
        }
        let (c, b) = reasons(r, rules);
        if c == 0 && b == 0 {
            continue;
        }
        let flag = s.catalog.photo(*id).map(|p| p.flag).unwrap_or_default();
        if flag != Flag::None {
            continue;
        }
        ops.push(Op::SetFlag { id: *id, flag: Flag::Reject });
        // a photo counts once, for its first reason: closed eyes
        if c > 0 {
            closed_n += 1;
        } else {
            blurry_n += 1;
        }
    }
    let rejected = ops.len();
    if !ops.is_empty() {
        s.commit("Reject Closed and Blurry Eyes", Op::Batch { ops })?;
    }
    Ok(json!({
        "done": true,
        "rejected": rejected,
        "closedEyes": closed_n,
        "blurryEyes": blurry_n,
        "noFaces": no_faces,
        "looked": reports.len(),
        "failed": failed,
        "cancelled": cancelled,
    }))
}

/// `photo.rejectEyes`.
pub fn reject_eyes(s: &mut Session, p: &Value) -> Result<Value> {
    let rules = EyeRules {
        closed: p.get("closed").and_then(Value::as_bool).unwrap_or(true),
        blurry: p.get("blurry").and_then(Value::as_bool).unwrap_or(true),
        soft: p.get("soft").and_then(Value::as_bool).unwrap_or(false),
        main_only: p.get("mainOnly").and_then(Value::as_bool).unwrap_or(true),
    };
    if !rules.closed && !rules.blurry {
        return Ok(json!({"done": true, "rejected": 0}));
    }
    if s.eye_cull.as_ref().is_some_and(|r| !r.cancel.load(Ordering::Relaxed)) {
        return Err(crate::cmd::bad(C, "a run is in progress already"));
    }
    let ids: Vec<PhotoId> = if p.get("ids").is_some() || s.selection.ids.len() > 1 { s.targets(p)? } else { s.visible_cloned() };
    let jobs: Vec<EyeJob> = ids.iter().filter_map(|id| s.eye_job(*id)).collect();
    let dir = model_dir(s)?;
    if p.get("wait").and_then(Value::as_bool).unwrap_or(false) {
        let mut reports = Vec::new();
        judge(&dir, &jobs, &AtomicBool::new(false), |id, r| reports.push((id, r)));
        return apply(s, rules, &reports, false);
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let task = s.activity.start("cull", "Finding closed and blurry eyes", crate::activity::Cancel::Flag(cancel.clone()));
    let total = jobs.len();
    task.progress(0, total as u64);
    let (tx, rx) = channel();
    let flag = cancel.clone();
    std::thread::Builder::new()
        .name("eye-cull".into())
        .spawn(move || {
            let mut done = 0u64;
            let cancelled = judge(&dir, &jobs, &flag, |id, r| {
                done += 1;
                task.progress(done, total as u64);
                let _ = tx.send(Done::Photo(id, r));
            });
            let _ = tx.send(Done::End { cancelled: cancelled || flag.load(Ordering::Relaxed) });
            drop(task);
        })
        .map_err(|e| crate::cmd::bad(C, format!("could not start ({e})")))?;
    s.eye_cull = Some(Arc::new(EyeCullRun { rx: Mutex::new(rx), cancel, rules, total }));
    s.eye_cull_results.clear();
    Ok(json!({"done": false, "photos": total}))
}

/// `photo.rejectEyesPump`: cheap every frame. Takes in finished photos; when the run is over,
/// flags the rejects (one undo step) and returns the summary once (`done: true`).
pub fn pump(s: &mut Session, _: &Value) -> Result<Value> {
    let Some(run) = s.eye_cull.clone() else { return Ok(json!({"active": false})) };
    let mut end = None;
    {
        let rx = run.rx.lock().unwrap_or_else(PoisonError::into_inner);
        for d in rx.try_iter() {
            match d {
                Done::Photo(id, r) => s.eye_cull_results.push((id, r)),
                Done::End { cancelled } => end = Some(cancelled),
            }
        }
    }
    let Some(cancelled) = end else {
        return Ok(json!({"active": true, "done": false, "looked": s.eye_cull_results.len(), "photos": run.total}));
    };
    s.eye_cull = None;
    let reports = std::mem::take(&mut s.eye_cull_results);
    let mut out = apply(s, run.rules, &reports, cancelled)?;
    out["active"] = json!(false);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eyes::{EyeShot, Focus, Lids};

    fn shot(main: bool, lids: Lids, focus: Focus) -> EyeShot {
        EyeShot {
            face: [0.0, 0.0, 0.1, 0.1],
            center: [0.05, 0.05],
            crop: Arc::new(lcc_raster::Rgba8::new(1, 1)),
            sharpness: 40.0,
            relative: 100.0,
            focus,
            both_eyes: true,
            openness: None,
            iris: None,
            contrast: None,
            look: None,
            eye_width: 40.0,
            score: 0.9,
            main,
            landmarks: true,
            lids,
        }
    }

    /// Closed beats blurry, a hidden eye (sunglasses) is neither, guests further back count only
    /// when asked, and each rule can be switched off.
    #[test]
    fn rejects_follow_the_rules() {
        let r = EyeReport {
            eyes: vec![
                shot(true, Lids::Closed, Focus::Sharp),
                shot(true, Lids::Open, Focus::Blurry),
                shot(true, Lids::Hidden, Focus::Blurry),
                shot(false, Lids::Closed, Focus::Sharp),
                shot(true, Lids::Open, Focus::Sharp),
            ],
            ..EyeReport::empty()
        };
        let all = EyeRules { closed: true, blurry: true, soft: false, main_only: true };
        assert_eq!(reasons(&r, all), (1, 1));
        assert_eq!(reasons(&r, EyeRules { main_only: false, ..all }), (2, 1));
        assert_eq!(reasons(&r, EyeRules { closed: false, ..all }), (0, 1));
        assert_eq!(reasons(&r, EyeRules { blurry: false, ..all }), (1, 0));
        // the stricter threshold: soft eyes are rejected too
        let soft = EyeReport { eyes: vec![shot(true, Lids::Open, Focus::Soft)], ..EyeReport::empty() };
        assert_eq!(reasons(&soft, all), (0, 0));
        assert_eq!(reasons(&soft, EyeRules { soft: true, ..all }), (0, 1));
        assert_eq!(reasons(&soft, EyeRules { soft: true, blurry: false, ..all }), (0, 0));
        let good = EyeReport { eyes: vec![shot(true, Lids::Open, Focus::Sharp), shot(true, Lids::Hidden, Focus::Blurry)], ..EyeReport::empty() };
        assert_eq!(reasons(&good, all), (0, 0));
    }
}
