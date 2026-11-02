//! Camera sensor maps and Remove Dust: commands to look at, build, extend and clear a camera body's hot-pixel and
//! dust maps (`crate::sensor_maps`), calibrate from dark frames, see what a photo's decode corrected, and remove the
//! dust of a photo or a selection (`spot.findDust`, `crate::dust_map`). Builds over the library and batch Remove
//! Dust run on a worker thread with a row in the activity stack; `dust.pump` (called every frame by the app) applies
//! finished Remove Dust results as develop edits.

use std::collections::{BTreeSet, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use lcc_catalog::{PhotoId, Source};
use lcc_develop::DevelopSettings;
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, f64_or, has_active, str_param};
use crate::activity::{Cancel, TaskGuard};
use crate::dust_map::{self, PhotoContext, PhotoDust};
use crate::media::RenderJob;
use crate::sensor_maps::{self, Shot};
use crate::{Result, Session};

/// A map build or batch Remove Dust in the background.
#[derive(Default)]
pub struct SensorJob {
    /// `pixels` (hot pixel map), `dust` (dust map) or `removeDust`.
    pub what: &'static str,
    pub total: usize,
    pub done: AtomicUsize,
    pub failed: AtomicUsize,
    pub cancel: Arc<AtomicBool>,
    pub finished: AtomicBool,
    /// Bodies the photos came from.
    pub bodies: Mutex<BTreeSet<String>>,
    /// `[photoId, why]` of photos that couldn't be read.
    pub errors: Mutex<Vec<Value>>,
    /// Remove Dust results waiting to be applied (`dust.pump`): photo, spots, replace the earlier auto-dust spots.
    pending: Mutex<Vec<(PhotoId, PhotoDust)>>,
    replace: bool,
    /// Remove Dust: photos changed and spots added so far.
    pub applied: AtomicUsize,
    pub added: AtomicUsize,
}

impl SensorJob {
    pub fn json(&self) -> Value {
        json!({
            "what": self.what,
            "total": self.total,
            "done": self.done.load(Ordering::Relaxed),
            "failed": self.failed.load(Ordering::Relaxed),
            "running": !self.finished.load(Ordering::Relaxed),
            "cancelled": self.cancel.load(Ordering::Relaxed),
            "bodies": self.bodies.lock().unwrap_or_else(PoisonError::into_inner).iter().cloned().collect::<Vec<_>>(),
            "errors": self.errors.lock().unwrap_or_else(PoisonError::into_inner).iter().take(20).cloned().collect::<Vec<_>>(),
            "applied": self.applied.load(Ordering::Relaxed),
            "added": self.added.load(Ordering::Relaxed),
        })
    }

    fn fail(&self, id: PhotoId, why: impl Into<String>) {
        self.failed.fetch_add(1, Ordering::Relaxed);
        let mut e = self.errors.lock().unwrap_or_else(PoisonError::into_inner);
        if e.len() < 200 {
            e.push(json!([id.0, why.into()]));
        }
    }

    fn step(&self, task: &TaskGuard) {
        let done = self.done.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        task.progress(done as u64, self.total as u64);
    }

    fn running(&self) -> bool {
        !self.finished.load(Ordering::Relaxed)
    }
}

/// The photos a build or a batch covers: `ids`, else the selection (more than one), else every photo in view.
fn photos(s: &mut Session, p: &Value) -> Result<Vec<PhotoId>> {
    if p.get("ids").is_some() || p.get("id").is_some() || s.selection.ids.len() > 1 { s.targets(p) } else { Ok(s.visible_cloned()) }
}

/// A body named in `p` (`body`), else the active photo's.
fn body_param(s: &Session, p: &Value) -> Option<String> {
    str_param(p, "body").map(str::to_string).or_else(|| s.active().and_then(|id| photo_body(s, id)))
}

/// The body of photo `id`: known from its decode, else read from its file (serial), else its file name.
pub fn photo_body(s: &Session, id: PhotoId) -> Option<String> {
    let p = s.catalog.photo(id)?;
    let Source::File { path } = &p.source else { return None };
    if let Some(b) = sensor_maps::body_of_path(path) {
        return Some(b);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let serial = std::fs::read(path).ok().and_then(|b| lcc_raw::probe_info(&b).ok()).and_then(|i| i.metadata.serial_number);
        let b = sensor_maps::body_key(serial.as_deref(), Some(&p.file_name));
        if let Some(b) = &b {
            sensor_maps::remember_path(path, b);
        }
        b
    }
    #[cfg(target_arch = "wasm32")]
    {
        sensor_maps::body_key(None, Some(&p.file_name))
    }
}

/// The capture facts of a catalogued photo (for its dust).
fn shot_of_photo(p: &lcc_catalog::Photo) -> Shot {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in crate::media::content_key(p).bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    Shot { fingerprint: h, captured: p.captured.clone(), exposure: None, iso: p.meta.iso, f_number: p.meta.aperture }
}

fn status(what: &str, s: &Session, p: &Value) -> Result<Value> {
    let all = str_param(p, "body") == Some("all");
    let bodies: Vec<String> = if all { sensor_maps::bodies() } else { body_param(s, p).into_iter().collect() };
    let maps: Vec<Value> = bodies
        .iter()
        .map(|b| match sensor_maps::get(b) {
            Some(m) => {
                let mut v = m.summary();
                if what == "pixels" && p.get("list").and_then(Value::as_bool).unwrap_or(false) {
                    v["list"] = json!(m.pixels.iter().filter(|e| e.confirmed).take(2000).map(|e| json!({"x": e.x, "y": e.y, "s": e.s, "kind": e.kind, "hits": e.hits, "first": e.first, "last": e.last, "source": e.source})).collect::<Vec<_>>());
                }
                if what == "dust" && p.get("list").and_then(Value::as_bool).unwrap_or(false) {
                    v["list"] = json!(m.dust.iter().take(500).map(|d| json!({"x": d.x, "y": d.y, "confirmed": d.confirmed, "windows": d.windows, "apertures": d.apertures})).collect::<Vec<_>>());
                }
                v
            }
            None => json!({"body": b, "label": sensor_maps::body_label(b), "photos": 0}),
        })
        .collect();
    let job = s.sensor_job.as_ref().map(|j| j.json());
    Ok(json!({"bodies": maps, "folder": sensor_maps::dir().map(|d| d.to_string_lossy().into_owned()), "job": job}))
}

/// The worker's lanes and the gate: `work` per item, at most `lanes` at once, until cancelled.
fn run_lanes<T: Send>(items: Vec<T>, lanes: usize, job: &SensorJob, work: impl Fn(T) + Sync) {
    let queue = Mutex::new(items.into_iter());
    let lane = || {
        loop {
            if job.cancel.load(Ordering::Relaxed) {
                break;
            }
            let next = queue.lock().unwrap_or_else(PoisonError::into_inner).next();
            let Some(item) = next else { break };
            crate::memory::in_background(|| work(item));
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::scope(|sc| {
        for k in 1..lanes.max(1) {
            if std::thread::Builder::new().name(format!("sensor-map-{k}")).spawn_scoped(sc, lane).is_err() {
                break;
            }
        }
        lane();
    });
    #[cfg(target_arch = "wasm32")]
    {
        let _ = lanes;
        lane();
    }
}

/// Start `run` on a worker thread holding the activity row (or run it here with `wait`).
fn start(
    s: &mut Session,
    c: &str,
    job: Arc<SensorJob>,
    label: &str,
    wait: bool,
    run: impl FnOnce(&SensorJob, &TaskGuard) + Send + 'static,
) -> Result<Value> {
    if s.sensor_job.as_ref().is_some_and(|j| j.running()) {
        return Err(bad(c, "a camera sensor map build or Remove Dust is already running"));
    }
    s.sensor_job = Some(job.clone());
    let task = s.activity.start("sensorMaps", label, Cancel::Flag(job.cancel.clone()));
    task.progress(0, job.total as u64);
    let body = move |job: Arc<SensorJob>, task: TaskGuard| {
        let r = crate::guard::catch("camera sensor maps", || run(&job, &task));
        if r.is_err() {
            job.errors.lock().unwrap_or_else(PoisonError::into_inner).push(json!([0, "the build stopped on an internal error"]));
        }
        sensor_maps::flush();
        job.finished.store(true, Ordering::Relaxed);
        drop(task);
    };
    if wait || cfg!(target_arch = "wasm32") {
        body(job.clone(), task);
        if job.what == "removeDust" {
            apply_pending(s);
        }
        return Ok(job.json());
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let j = job.clone();
        std::thread::Builder::new()
            .name("lc-sensor-maps".into())
            .spawn(move || body(j, task))
            .map_err(|e| bad(c, format!("could not start: {e}")))?;
    }
    Ok(job.json())
}

/// `camera.pixelMap.rebuild` / `learn`: decode the raws and record their pixel candidates per body.
fn pixel_build(s: &mut Session, p: &Value, rebuild: bool) -> Result<Value> {
    let c = if rebuild { "camera.pixelMap.rebuild" } else { "camera.pixelMap.learn" };
    let only = str_param(p, "body").map(str::to_string);
    let clear = rebuild && p.get("clear").and_then(Value::as_bool).unwrap_or(true);
    let mut items: Vec<(PhotoId, String, Option<String>)> = photos(s, p)?
        .into_iter()
        .filter_map(|id| {
            let ph = s.catalog.photo(id)?;
            let Source::File { path } = &ph.source else { return None };
            ph.develops_raw().then(|| (id, path.clone(), ph.captured.clone()))
        })
        .collect();
    items.sort_by(|a, b| a.2.cmp(&b.2));
    if items.is_empty() {
        return Err(bad(c, "no raw photos to read (select some, or show the folder or album that holds them)"));
    }
    let job = Arc::new(SensorJob { what: "pixels", total: items.len(), ..Default::default() });
    let wait = p.get("wait").and_then(Value::as_bool).unwrap_or(false);
    start(s, c, job, "Mapping hot pixels", wait, move |job, task| {
        let cleared = Mutex::new(HashSet::<String>::new());
        let lanes = (std::thread::available_parallelism().map_or(2, |n| n.get()) / 4).clamp(1, 3);
        run_lanes(items, lanes, job, |(id, path, _)| {
            match learn_pixels(&path, only.as_deref(), clear, &cleared) {
                Ok(Some(body)) => {
                    job.bodies.lock().unwrap_or_else(PoisonError::into_inner).insert(body);
                }
                Ok(None) => {}
                Err(e) => job.fail(id, e),
            }
            job.step(task);
        });
    })
}

/// Decode one raw and record its candidates; `Ok(None)`: not the body asked for (or no body).
fn learn_pixels(path: &str, only: Option<&str>, clear: bool, cleared: &Mutex<HashSet<String>>) -> std::result::Result<Option<String>, String> {
    use lcc_raw::defects::{Thresholds, detect};
    let len = std::fs::metadata(path).map(|m| usize::try_from(m.len()).unwrap_or(usize::MAX)).map_err(|e| format!("{path}: {e}"))?;
    let _permit = crate::memory::work_gate().acquire(len.saturating_mul(3));
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let raw = lcc_raw::decode(&bytes).map_err(|e| e.to_string())?;
    drop(bytes);
    let name = std::path::Path::new(path).file_name().and_then(|n| n.to_str());
    let m = &raw.metadata;
    let Some(body) = sensor_maps::body_key(m.serial_number.as_deref(), name) else { return Ok(None) };
    sensor_maps::remember_path(path, &body);
    if only.is_some_and(|o| o != body) {
        return Ok(None);
    }
    if clear && cleared.lock().unwrap_or_else(PoisonError::into_inner).insert(body.clone()) {
        sensor_maps::clear(&body, true, false)?;
    }
    let found = detect(&raw, &Thresholds::LEARN);
    if !found.overflow {
        let (make, model) = (m.make.clone().unwrap_or_default(), m.model.clone().unwrap_or_default());
        let _ = sensor_maps::record_pixels(&body, &make, &model, [raw.width, raw.height], &crate::files::shot_of(&raw), &found.defects);
    }
    crate::memory::release();
    Ok(Some(body))
}

/// One photo's dust analysis to run off the UI thread: its render job and what the catalog says about it.
struct DustItem {
    id: PhotoId,
    job: RenderJob,
    user: lcc_geom::Orientation,
    shot: Shot,
}

/// The render jobs of `ids` for dust analysis (uncropped, without auto-dust spots), oldest first.
fn dust_items(s: &mut Session, ids: &[PhotoId]) -> Vec<DustItem> {
    let mut items: Vec<(Option<String>, DustItem)> = Vec::new();
    for &id in ids {
        let Some(ph) = s.catalog.photo(id).cloned() else { continue };
        let settings = dust_map::without_auto_dust(&s.develop_of(id).unwrap_or_default());
        let Some(job) = s.preview_job(id, dust_map::EDGE, dust_map::EDGE, false, &settings) else { continue };
        items.push((ph.captured.clone(), DustItem { id, job, user: settings.orientation, shot: shot_of_photo(&ph) }));
    }
    items.sort_by(|a, b| a.0.cmp(&b.0));
    items.into_iter().map(|(_, i)| i).collect()
}

/// Render and analyse one photo. `Err` when it can't be rendered.
fn analyse_item(
    item: DustItem,
    sensitivity: f32,
    use_map: bool,
    learn: bool,
    only: Option<&str>,
) -> std::result::Result<(PhotoId, PhotoDust, Option<String>), String> {
    let from_cache = match &item.job.source {
        crate::media::SourceRef::Loaded(d) => d.info.as_ref().and_then(|i| i.pixels.clone()),
        _ => None,
    };
    let r = item.job.run();
    let img = r.rendered?.image;
    let pixels = from_cache.or_else(|| r.loaded.as_ref().and_then(|d| d.info.as_ref()).and_then(|i| i.pixels.clone()));
    let ctx = match &pixels {
        Some(px) => PhotoContext {
            body: px.body.clone(),
            model: px.model.clone(),
            sensor: px.sensor,
            exif: px.orientation,
            user: item.user,
            developed: px.developed,
            shot: item.shot,
        },
        None => PhotoContext { user: item.user, shot: item.shot, ..Default::default() },
    };
    let body = ctx.body.clone();
    let learn = learn && only.is_none_or(|o| body.as_deref() == Some(o));
    Ok((item.id, dust_map::analyse(&img, &ctx, sensitivity, use_map, learn), body))
}

/// Remove Dust over several photos in the background (or here with `wait`): the results wait for `dust.pump`.
fn remove_dust_batch(s: &mut Session, p: &Value, ids: Vec<PhotoId>) -> Result<Value> {
    let c = "spot.findDust";
    let items = dust_items(s, &ids);
    let sensitivity = f64_or(p, "sensitivity", 50.0) as f32;
    let use_map = p.get("useMap").and_then(Value::as_bool).unwrap_or(true);
    let replace = p.get("replace").and_then(Value::as_bool).unwrap_or(true);
    let job = Arc::new(SensorJob { what: "removeDust", total: items.len(), replace, ..Default::default() });
    let wait = p.get("wait").and_then(Value::as_bool).unwrap_or(false);
    start(s, c, job, "Removing dust", wait, move |job, task| {
        for item in items {
            if job.cancel.load(Ordering::Relaxed) {
                break;
            }
            let id = item.id;
            match crate::memory::in_background(|| analyse_item(item, sensitivity, use_map, true, None)) {
                Ok((id, dust, body)) => {
                    if let Some(b) = body {
                        job.bodies.lock().unwrap_or_else(PoisonError::into_inner).insert(b);
                    }
                    job.pending.lock().unwrap_or_else(PoisonError::into_inner).push((id, dust));
                }
                Err(e) => job.fail(id, e),
            }
            job.step(task);
        }
    })
}

/// After an import with Settings → Import → Remove dust on import: Remove Dust on the new raws, in the background.
/// Nothing when another build or Remove Dust is running (the import is never held up or failed by it).
pub fn remove_dust_after_import(s: &mut Session, ids: Vec<PhotoId>) {
    if ids.is_empty() {
        return;
    }
    if let Err(e) = remove_dust_batch(s, &json!({}), ids) {
        log::info!("remove dust on import: {e}");
    }
}

/// Put finished Remove Dust results on their photos (one undo step each). Returns how many photos changed.
fn apply_pending(s: &mut Session) -> usize {
    let Some(job) = s.sensor_job.clone() else { return 0 };
    let ready: Vec<(PhotoId, PhotoDust)> = std::mem::take(&mut *job.pending.lock().unwrap_or_else(PoisonError::into_inner));
    let mut n = 0;
    for (id, dust) in ready {
        if s.catalog.photo(id).is_none() {
            continue;
        }
        if apply_dust(s, id, &dust, job.replace).is_ok_and(|changed| changed) {
            n += 1;
            job.applied.fetch_add(1, Ordering::Relaxed);
            job.added.fetch_add(dust.spots.len(), Ordering::Relaxed);
        }
    }
    n
}

/// Replace photo `id`'s auto-dust spots with `dust`'s (one undo step). `Ok(false)`: nothing changed.
fn apply_dust(s: &mut Session, id: PhotoId, dust: &PhotoDust, replace: bool) -> Result<bool> {
    let current = s.develop_of(id).unwrap_or_default();
    let mut dd: DevelopSettings = if replace { dust_map::without_auto_dust(&current) } else { (*current).clone() };
    dd.spots.extend(dust.spots.iter().cloned());
    if dd == *current {
        return Ok(false);
    }
    s.set_develop(id, dd, "Remove Dust")?;
    Ok(true)
}

fn spot_json(sp: &lcc_develop::Spot) -> Value {
    let at = sp.points.first().copied().unwrap_or(lcc_geom::Point::new(0.0, 0.0));
    json!({
        "x": at.x,
        "y": at.y,
        "size": sp.size,
        "mode": sp.mode,
        "confirmed": sp.auto.is_some_and(|a| a.confirmed),
        "confidence": sp.auto.map_or(0.0, |a| a.confidence),
    })
}

/// `spot.findDust` (Remove Dust): the active photo here and now; a selection (or `ids`) in the background.
fn find_dust(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "spot.findDust";
    let ids =
        if p.get("ids").is_some() || p.get("id").is_some() || s.selection.ids.len() > 1 { s.targets(p)? } else { s.active().into_iter().collect() };
    if ids.len() > 1 {
        return remove_dust_batch(s, p, ids);
    }
    let id = *ids.first().ok_or_else(|| bad(c, "no active photo"))?;
    let sensitivity = f64_or(p, "sensitivity", 50.0) as f32;
    let use_map = p.get("useMap").and_then(Value::as_bool).unwrap_or(true);
    let replace = p.get("replace").and_then(Value::as_bool).unwrap_or(true);
    let add = p.get("add").and_then(Value::as_bool).unwrap_or(true);
    let item = dust_items(s, &[id]).into_iter().next().ok_or_else(|| bad(c, "no photo"))?;
    let (_, dust, body) = analyse_item(item, sensitivity, use_map, add, None).map_err(|e| bad(c, e))?;
    let out: Vec<Value> = dust.spots.iter().map(spot_json).collect();
    let added = if add && (!dust.spots.is_empty() || replace) && apply_dust(s, id, &dust, replace)? { dust.spots.len() } else { 0 };
    Ok(json!({
        "spots": out,
        "added": added,
        "found": dust.found,
        "confirmed": dust.confirmed,
        "fromMap": dust.from_map,
        "skippedOnDetail": dust.skipped,
        "body": body,
        "learnt": dust.learnt,
    }))
}

/// `camera.pixelMap.calibrate`: dark frames → confirmed hot pixels (and the frames' noise figures, kept as JSON).
fn calibrate(_s: &mut Session, p: &Value) -> Result<Value> {
    let c = "camera.pixelMap.calibrate";
    let paths: Vec<String> = match p.get("paths").or_else(|| p.get("path")) {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        Some(Value::String(one)) => vec![one.clone()],
        _ => return Err(bad(c, "give `paths`: raw files shot with the lens cap on")),
    };
    let mut out = Vec::new();
    for path in paths.iter().take(200) {
        let r = (|| -> std::result::Result<Value, String> {
            let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
            let raw = lcc_raw::decode(&bytes).map_err(|e| e.to_string())?;
            let df = lcc_raw::darkframe::analyze(&raw)?;
            let name = std::path::Path::new(path).file_name().and_then(|n| n.to_str());
            let body = match str_param(p, "body") {
                Some(b) => b.to_string(),
                None => sensor_maps::body_key(df.serial.as_deref(), name).ok_or("the file names no camera body (no serial number); give `body`")?,
            };
            let (make, model) = (df.make.clone().unwrap_or_default(), df.model.clone().unwrap_or_default());
            let n = sensor_maps::update(&body, &make, &model, [raw.width, raw.height], |m| m.record_dark_frame(&df))?;
            let kept = sensor_maps::save_dark_frame(&body, &df);
            Ok(json!({
                "file": path,
                "body": body,
                "hot": n,
                "iso": df.iso,
                "exposure": df.exposure,
                "offset": df.offset,
                "noise": df.noise,
                "rowNoise": df.row_noise,
                "columnNoise": df.column_noise,
                "saved": kept.ok().map(|p| p.to_string_lossy().into_owned()),
            }))
        })();
        out.push(r.unwrap_or_else(|e| json!({"file": path, "error": e})));
    }
    Ok(json!({"frames": out}))
}

/// `photo.pixelDefects`: what decoding photo `id` corrects (reads the file; nothing is changed).
fn pixel_defects(s: &mut Session, p: &Value) -> Result<Value> {
    use lcc_raw::defects::{Thresholds, detect, refine};
    let c = "photo.pixelDefects";
    let id = s.targets(p)?.first().copied().ok_or_else(|| bad(c, "no photo"))?;
    let ph = s.catalog.photo(id).ok_or_else(|| bad(c, "no such photo"))?;
    let Source::File { path } = &ph.source else { return Err(bad(c, "not a file")) };
    let on = ph.develop.sensor.fixes_pixels();
    let bytes = std::fs::read(path).map_err(|e| bad(c, format!("{path}: {e}")))?;
    let raw = lcc_raw::decode(&bytes).map_err(|e| bad(c, e.to_string()))?;
    let m = &raw.metadata;
    let body = sensor_maps::body_key(m.serial_number.as_deref(), Some(&ph.file_name));
    let found = detect(&raw, &Thresholds::LEARN);
    let strict = refine(&raw, &found, &Thresholds::CORRECT);
    let map = body.as_deref().map(|b| sensor_maps::fixes_for(b, m.model.as_deref().unwrap_or(""), [raw.width, raw.height])).unwrap_or_default();
    let limit = p.get("limit").and_then(Value::as_u64).unwrap_or(500).min(20_000) as usize;
    Ok(json!({
        "on": on,
        "body": body,
        "fromMap": map.iter().take(limit).map(|f| json!([f.x, f.y, f.s])).collect::<Vec<_>>(),
        "fromMapCount": map.len(),
        "found": strict.iter().take(limit).map(|d| json!({"x": d.x, "y": d.y, "s": d.s, "kind": d.kind, "level": d.level})).collect::<Vec<_>>(),
        "candidates": found.defects.len(),
        "noise": found.noise,
        "overflow": found.overflow,
    }))
}

fn clear_map(s: &mut Session, p: &Value, pixels: bool) -> Result<Value> {
    let c = if pixels { "camera.pixelMap.clear" } else { "camera.dustMap.clear" };
    let body = body_param(s, p).ok_or_else(|| bad(c, "give `body` (see camera.pixelMap.status {body: \"all\"})"))?;
    sensor_maps::clear(&body, pixels, !pixels).map_err(|e| bad(c, e))?;
    Ok(json!({"body": body, "cleared": true}))
}

/// `dust.pump`: called every frame by the app; applies finished Remove Dust results.
fn pump(s: &mut Session, _p: &Value) -> Result<Value> {
    let applied = apply_pending(s);
    let job = s.sensor_job.as_ref().map(|j| j.json());
    Ok(json!({"applied": applied, "job": job}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "camera.pixelMap.status",
            "Hot Pixel Map",
            [],
            None,
            "{body?: bodyKey | \"all\" (default: the active photo's camera body), list?: bool} → {bodies: [{body, label, model, photos, pixels: {confirmed, candidates, hot, dead, stuck, fromDarkFrames}, dustPhotos, dust, list?}], folder, job} — what the app remembers about a camera body's hot, dead and stuck pixels (bodies: the serial number, else file:XX from the file names)",
            always,
            |s, p| status("pixels", s, p)
        ),
        cmd!(
            query "camera.pixelMap.rebuild",
            "Rebuild Hot Pixel Map",
            ["Photo", "Camera Sensor"],
            None,
            "{body?: bodyKey (only that body's photos), ids?: [photoId] (default: the selection, else the photos in view), clear?: bool (true: start the map afresh), wait?: bool} → job {what, total, done, running, bodies…} — reads the raws in the background (activity row \"Mapping hot pixels\"); a pixel found in 3 photos from 3 different 5-minute periods is confirmed and fixed on every photo of the body",
            always,
            |s, p| pixel_build(s, p, true)
        ),
        cmd!(
            query "camera.pixelMap.learn",
            "Add Photos to Hot Pixel Map",
            [],
            None,
            "{ids?, body?, wait?} → job — like camera.pixelMap.rebuild without clearing: adds photos to the maps (photos already counted are skipped)",
            always,
            |s, p| pixel_build(s, p, false)
        ),
        cmd!(
            query "camera.pixelMap.clear",
            "Clear Hot Pixel Map",
            [],
            None,
            "{body?: bodyKey (default: the active photo's)} → {body, cleared} — forget a camera body's hot pixels",
            always,
            |s, p| clear_map(s, p, true)
        ),
        cmd!(
            query "camera.pixelMap.calibrate",
            "Calibrate from Dark Frames",
            [],
            None,
            "{paths: [file] (raws shot with the lens cap on, at the exposure and ISO you use), body?: bodyKey (default: from each file)} → {frames: [{file, body, hot, iso, exposure, offset, noise, rowNoise, columnNoise, saved} | {file, error}]} — every hot pixel of the frame is confirmed at once; the frame's noise figures are kept as JSON (docs/sensor-defects.md)",
            always,
            calibrate
        ),
        cmd!(
            query "camera.dustMap.status",
            "Sensor Dust Map",
            [],
            None,
            "{body?: bodyKey | \"all\", list?: bool} → {bodies: [{…, dust: {confirmed, suggested, windows}, list?: [{x, y, confirmed, windows, apertures}]}], job} — the dust the app remembers per camera body (sensor positions, time windows, size per aperture)",
            always,
            |s, p| status("dust", s, p)
        ),
        cmd!(
            query "camera.dustMap.rebuild",
            "Rebuild Sensor Dust Map",
            ["Photo", "Camera Sensor"],
            None,
            "{body?, ids?, clear?: bool (true), sensitivity?: 0..100 (50), wait?} → job — analyses the photos for dust in the background, oldest first (activity row \"Mapping sensor dust\"); a spot in 3 photos over 2 periods is confirmed",
            always,
            |s, p| dust_rebuild(s, p, true)
        ),
        cmd!(
            query "camera.dustMap.learn",
            "Add Photos to Sensor Dust Map",
            [],
            None,
            "{ids?, body?, sensitivity?, wait?} → job — like camera.dustMap.rebuild without clearing",
            always,
            |s, p| dust_rebuild(s, p, false)
        ),
        cmd!(
            query "camera.dustMap.clear",
            "Clear Sensor Dust Map",
            [],
            None,
            "{body?} → {body, cleared} — forget a camera body's dust",
            always,
            |s, p| clear_map(s, p, false)
        ),
        cmd!(
            query "camera.sensorMaps.cancel",
            "Stop Camera Sensor Map Build",
            [],
            None,
            "{} → job — stop the map build or batch Remove Dust (what it did so far is kept)",
            always,
            |s, _| {
                if let Some(j) = &s.sensor_job {
                    j.cancel.store(true, Ordering::Relaxed);
                }
                Ok(s.sensor_job.as_ref().map(|j| j.json()).unwrap_or(Value::Null))
            }
        ),
        cmd!(
            query "photo.pixelDefects",
            "Hot Pixels of Photo",
            [],
            None,
            "{id?, limit?: 500} → {on, body, fromMap: [[x, y, s]], fromMapCount, found: [{x, y, s, kind, level}], candidates, noise, overflow} — what decoding the photo corrects: the body's confirmed pixels and the ones found in it (raw data coordinates); reads the file, changes nothing",
            has_active,
            pixel_defects
        ),
        cmd!(
            query "dust.pump",
            "Remove Dust Pump",
            [],
            None,
            "{} → {applied, job} — called every frame by the app: puts finished background Remove Dust results on their photos",
            always,
            pump
        ),
        cmd!(
            "spot.findDust",
            "Remove Dust",
            ["Photo", "Camera Sensor"],
            None,
            "{sensitivity?: 0..100 (50), add?: bool (true), useMap?: bool (true: add the camera body's known dust, re-checked on this photo), replace?: bool (true: earlier auto-dust spots go), ids?: [photoId] (several: in the background, activity row \"Removing dust\"; default: the selection when more than one, else the active photo), wait?: bool} → one photo: {spots: [{x, y, size, mode, confirmed, confidence}], added, found, confirmed, fromMap, skippedOnDetail, body, learnt}; several: job — find sensor dust (small soft dark spots on smooth areas) and heal each (or even it out: large faint ones); the spots are tagged auto dust; the photo also teaches its body's dust map",
            has_active,
            find_dust
        ),
    ]
}

/// The dust map builds: analyse and learn, oldest first.
fn dust_rebuild(s: &mut Session, p: &Value, rebuild: bool) -> Result<Value> {
    let c = if rebuild { "camera.dustMap.rebuild" } else { "camera.dustMap.learn" };
    let only = str_param(p, "body").map(str::to_string);
    let clear = rebuild && p.get("clear").and_then(Value::as_bool).unwrap_or(true);
    let ids: Vec<PhotoId> = photos(s, p)?.into_iter().filter(|id| s.catalog.photo(*id).is_some_and(|ph| ph.develops_raw())).collect();
    if ids.is_empty() {
        return Err(bad(c, "no raw photos to read (select some, or show the folder or album that holds them)"));
    }
    // a rebuild clears each body's dust before its first photo is learnt: bodies whose photos are in the list
    if clear {
        let mut seen = HashSet::new();
        for id in &ids {
            if let Some(b) = photo_body(s, *id).filter(|b| only.as_ref().is_none_or(|o| o == b))
                && seen.insert(b.clone())
            {
                sensor_maps::clear(&b, false, true).map_err(|e| bad(c, e))?;
            }
        }
    }
    let items = dust_items(s, &ids);
    let sensitivity = f64_or(p, "sensitivity", 50.0) as f32;
    let job = Arc::new(SensorJob { what: "dust", total: items.len(), ..Default::default() });
    let wait = p.get("wait").and_then(Value::as_bool).unwrap_or(false);
    start(s, c, job, "Mapping sensor dust", wait, move |job, task| {
        // (one photo at a time, oldest first: a spot's time windows follow the order photos were taken in)
        for item in items {
            if job.cancel.load(Ordering::Relaxed) {
                break;
            }
            let id = item.id;
            let wanted = |b: &Option<String>| only.is_none() || *b == only;
            match crate::memory::in_background(|| analyse_item(item, sensitivity, false, true, only.as_deref())) {
                Ok((_, _, body)) if wanted(&body) => {
                    if let Some(b) = body {
                        job.bodies.lock().unwrap_or_else(PoisonError::into_inner).insert(b);
                    }
                }
                Ok(_) => {}
                Err(e) => job.fail(id, e),
            }
            job.step(task);
        }
    })
}
