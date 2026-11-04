//! The session's side of the shared AI backend (`lcc_ml`, issue #491): its one job queue, the GPU probe it uses (the
//! same switches as GPU rendering: `{ENV_PREFIX}_GPU`, `{ENV_PREFIX}_GPU_BACKEND`, `app.gpu`), its batch row in the
//! activity stack ("Indexing photos · 37 of 200", kind `mlIndex`; its ✕ cancels the batch), model installation and
//! downloads (`ml.models.*`), the per-library embedding store (`ml-embeddings-<id>.bin`), the background indexing pump
//! (`ml.pump`, `ml.index`), and semantic search (`search.semantic`).
//!
//! `{ENV_PREFIX}_ML_DEVICE=cpu` keeps the AI jobs on the CPU (`auto`, the default, takes the GPU when there is one);
//! `ml.status {preference}` changes it for the session. See `docs/ml-backend.md`.

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use lcc_catalog::{Photo, PhotoId};
use lcc_develop::DevelopSettings;
use lcc_ml::{
    CLIP_VIT_B32_ID, Config, Ctx, Device, Download, Encoder, Job, JobError, ModelManifest, Outcome, Preference, Priority, Probe, Queue, RgbImage,
    Role, Row, Rows, Status, Store, SubmitError, fingerprint, store_file_name, valid_id,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Session;
use crate::activity::{Activity, Cancel, TaskGuard, TaskHandle, Unit};
use crate::cmd::bad;

/// What a job hands back; its owner (known by the job's key) downcasts it.
pub(crate) type Output = Box<dyn Any + Send>;

/// The activity row's kind and label.
pub(crate) const ROW_KIND: &str = "mlIndex";
pub(crate) const ROW_LABEL: &str = "Indexing photos";

/// Staging folder inside `ml_models_dir` for in-progress model downloads.
pub const STAGING_DIR: &str = ".downloads";

/// Settings file inside `ml_models_dir` recording the chosen models.
const SETTINGS_FILE: &str = "settings.json";

/// Long edge of the preview decoded for CLIP embedding (more than 2× the 224 px input).
const EMBED_PREVIEW_EDGE: usize = 512;

/// Minimum embedded raw preview edge accepted without running the develop pipeline.
const MIN_RAW_PREVIEW_EDGE: usize = 224;

/// How often dirty embeddings are flushed to disk while a batch is running.
const SAVE_EVERY: Duration = Duration::from_secs(3);

/// How long the cached model check is reused by per-frame `ml.pump`.
const MODEL_RECHECK: Duration = Duration::from_secs(1);

/// Rest between photos when `ml.pump` runs at `light` pace.
const LIGHT_REST: Duration = Duration::from_millis(25);

/// Poll interval when `ml.pump` pauses background work.
const PAUSE_POLL: Duration = Duration::from_millis(25);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Pace {
    Pause = 0,
    Light = 1,
    Normal = 2,
    Full = 3,
}

impl Pace {
    pub fn parse(name: Option<&str>) -> Self {
        match name {
            Some("pause") => Self::Pause,
            Some("light") => Self::Light,
            Some("full") => Self::Full,
            _ => Self::Normal,
        }
    }

    fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Pause,
            1 => Self::Light,
            3 => Self::Full,
            _ => Self::Normal,
        }
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct MlSettings {
    embedder: Option<String>,
}

fn read_settings(dir: &Path) -> MlSettings {
    std::fs::read(dir.join(SETTINGS_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn write_settings(dir: &Path, settings: &MlSettings) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let bytes = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
    let path = dir.join(SETTINGS_FILE);
    std::fs::write(&path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Output of a photo embedding job on [`Queue`].
struct EmbedOutput {
    epoch: u64,
    tag: String,
    photo_id: PhotoId,
    fingerprint: u64,
    embedding: Vec<f32>,
}

/// Output of a text query embedding job on [`Queue`].
struct QueryOutput(Vec<f32>);

/// State of an ML model download.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DownloadState {
    Running { bytes: u64, total: u64 },
    Installed,
    Failed(String),
    Cancelled,
}

struct DownloadShared {
    state: Mutex<DownloadState>,
    cancel: Arc<AtomicBool>,
    task: Option<TaskHandle>,
}

impl DownloadShared {
    fn set(&self, s: DownloadState) {
        if let (DownloadState::Running { bytes, total }, Some(task)) = (&s, &self.task) {
            task.progress(*bytes, *total);
        }
        *self.state.lock().unwrap_or_else(PoisonError::into_inner) = s;
    }

    fn get(&self) -> DownloadState {
        self.state.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

#[derive(Default)]
pub struct Downloads {
    jobs: HashMap<String, Arc<DownloadShared>>,
}

impl Downloads {
    pub fn snapshot(&self) -> Vec<(String, DownloadState)> {
        let mut out: Vec<_> = self.jobs.iter().map(|(k, v)| (k.clone(), v.get())).filter(|(_, s)| *s != DownloadState::Cancelled).collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    pub fn get(&self, id: &str) -> Option<DownloadState> {
        self.jobs.get(id).map(|j| j.get())
    }

    pub fn cancel(&mut self, id: &str) -> bool {
        let Some(job) = self.jobs.get(id) else {
            return false;
        };
        job.cancel.store(true, Ordering::Relaxed);
        if !matches!(job.get(), DownloadState::Running { .. }) {
            self.jobs.remove(id);
        }
        true
    }
}

impl Drop for Downloads {
    fn drop(&mut self) {
        for job in self.jobs.values() {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }
}

pub(crate) struct State {
    /// Made on the first job (no thread, no GPU probe before).
    queue: Option<Queue<Output>>,
    /// Set by `ml.status {preference}` before the queue exists.
    preference: Option<Preference>,
    /// Whether background indexing is enabled (`ml.enable`).
    pub enabled: bool,
    /// Set when the user clicks ✕ on the `mlIndex` activity row; cleared on library change, import, or `ml.index`.
    pub paused_by_user: bool,
    /// Bumped whenever the library or active model changes so stale in-flight results are ignored.
    pub epoch: u64,
    /// Current pace set by `ml.pump`.
    pace: Arc<AtomicU8>,
    /// Active or completed model downloads.
    pub downloads: Downloads,
    /// Loaded encoder (`(tag, encoder)`).
    #[cfg(not(target_arch = "wasm32"))]
    encoder: Option<(String, Arc<dyn Encoder>)>,
    /// Active model's embedding store.
    pub store: Option<Store>,
    /// Path to `ml-embeddings-<id>.bin` when the library is on disk.
    pub store_path: Option<PathBuf>,
    /// Photos currently waiting or running in `queue`.
    pub in_flight: HashSet<PhotoId>,
    /// Photos `(id, fingerprint)` whose decode/inference failed this session.
    pub failed_photos: HashSet<(u64, u64)>,
    /// Last time `store` was saved to disk.
    last_saved: Option<web_time::Instant>,
    /// Last time `ml_models_dir` was checked by `ml.pump`.
    last_checked: Option<web_time::Instant>,
    /// Monotonic counter for interactive text query keys.
    query_seq: u64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            queue: None,
            preference: None,
            enabled: true,
            paused_by_user: false,
            epoch: 1,
            pace: Arc::new(AtomicU8::new(Pace::Normal as u8)),
            downloads: Downloads::default(),
            #[cfg(not(target_arch = "wasm32"))]
            encoder: None,
            store: None,
            store_path: None,
            in_flight: HashSet::new(),
            failed_photos: HashSet::new(),
            last_saved: None,
            last_checked: None,
            query_seq: 0,
        }
    }
}

impl State {
    /// Flush dirty embeddings and reset per-library indexing state when opening or closing a library.
    pub fn library_changed(&mut self) {
        self.save_store_now();
        if let Some(q) = &self.queue {
            for id in self.in_flight.drain() {
                q.cancel(&format!("embed:{}", id.0));
            }
        } else {
            self.in_flight.clear();
        }
        self.failed_photos.clear();
        self.paused_by_user = false;
        self.store = None;
        self.store_path = None;
        self.last_checked = None;
        self.epoch = self.epoch.wrapping_add(1);
    }

    /// Forget a photo's cached embedding when its file on disk was replaced (`Op::SetContent`).
    pub fn content_replaced(&mut self, id: PhotoId) {
        if let Some(store) = &mut self.store {
            store.remove(id.0);
        }
        if let Some(q) = &self.queue {
            q.cancel(&format!("embed:{}", id.0));
        }
        self.in_flight.remove(&id);
        self.failed_photos.retain(|&(pid, _)| pid != id.0);
        self.paused_by_user = false;
    }

    /// Wake background indexing when new photos are added to the library.
    pub fn photo_added(&mut self) {
        self.paused_by_user = false;
    }

    /// Save the embedding store immediately if it is dirty and backed by a file on disk.
    pub fn save_store_now(&mut self) {
        if let (Some(store), Some(path)) = (&mut self.store, &self.store_path)
            && store.is_dirty()
        {
            if let Err(e) = store.save_atomic(path) {
                log::warn!("cannot save ML embedding store {}: {e}", path.display());
            } else {
                self.last_saved = Some(web_time::Instant::now());
            }
        }
    }
}

/// The preference from `{ENV_PREFIX}_ML_DEVICE` (`auto` when unset or not understood).
fn env_preference() -> Preference {
    lcc_brand::env("ML_DEVICE").and_then(|v| Preference::parse(&v.to_lowercase())).unwrap_or_default()
}

/// The GPU the AI jobs may use, or why not: the rendering GPU's switches apply.
fn gpu_probe() -> Result<String, String> {
    if let Some(why) = lcc_gpu::env_off_reason() {
        return Err(why);
    }
    if !lcc_gpu::enabled() {
        return Err("disabled by the GPU rendering preference (app.gpu)".into());
    }
    lcc_gpu::adapter_name().ok_or_else(|| lcc_gpu::unavailable_reason().unwrap_or_else(|| "no usable GPU".into()))
}

struct ActivityRows(Activity);

struct ActivityRow(TaskGuard);

impl Rows for ActivityRows {
    fn begin(&self, cancel: Arc<AtomicBool>) -> Box<dyn Row> {
        Box::new(ActivityRow(self.0.start_batch(ROW_KIND, ROW_LABEL, Cancel::Flag(cancel))))
    }
}

impl Row for ActivityRow {
    fn progress(&self, done: u64, total: u64) {
        self.0.progress(done, total);
    }
    fn detail(&self, name: &str) {
        self.0.detail(name);
    }
    fn item_done(&self) {
        self.0.item_done();
    }
    fn item_failed(&self, why: &str) {
        self.0.item_failed(why);
    }
}

fn device_json(d: &Device) -> Value {
    json!({"kind": d.kind.as_str(), "name": d.name, "whyCpu": d.why_cpu})
}

pub(crate) fn status_json(st: &Status) -> Value {
    json!({
        "device": st.device.as_ref().map(device_json),
        "preference": st.preference.as_str(),
        "pending": st.pending,
        "running": st.running,
        "done": st.done,
        "failed": st.failed,
        "cancelled": st.cancelled,
        "threaded": st.threaded,
    })
}

fn photo_fingerprint(photo: &Photo) -> u64 {
    fingerprint(&crate::media::content_key(photo))
}

fn installed_manifests(models_dir: &Path) -> Vec<ModelManifest> {
    let Ok(rd) = std::fs::read_dir(models_dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in rd.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(dir_name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if !valid_id(dir_name) {
            continue;
        }
        if let Ok(m) = ModelManifest::read_dir(&path)
            && m.id == dir_name
            && m.files.iter().all(|f| path.join(&f.name).is_file())
        {
            out.push(m);
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

fn active_embedder_id(models_dir: &Path) -> Option<String> {
    let installed = installed_manifests(models_dir);
    let settings = read_settings(models_dir);
    if let Some(chosen) = settings.embedder
        && installed.iter().any(|m| m.id == chosen && m.role == Role::Embedder)
    {
        return Some(chosen);
    }
    if installed.iter().any(|m| m.id == CLIP_VIT_B32_ID && m.role == Role::Embedder) {
        return Some(CLIP_VIT_B32_ID.to_string());
    }
    installed.into_iter().find(|m| m.role == Role::Embedder).map(|m| m.id)
}

fn manifest_json(m: &ModelManifest, installed: bool, selected: bool, dl: Option<&DownloadState>) -> Value {
    let dl_val = dl.map(|state| match state {
        DownloadState::Running { bytes, total } => {
            json!({"state": "running", "bytes": bytes, "total": total})
        }
        DownloadState::Installed => json!({"state": "installed"}),
        DownloadState::Failed(err) => json!({"state": "failed", "error": err}),
        DownloadState::Cancelled => json!({"state": "cancelled"}),
    });
    json!({
        "id": m.id,
        "name": m.name,
        "role": m.role.as_str(),
        "versionTag": m.version_tag,
        "embedDim": m.embed_dim,
        "sizeBytes": m.total_bytes(),
        "licence": {
            "spdx": m.licence.spdx,
            "commercial": m.licence.commercial.as_str(),
            "url": m.licence.url,
            "attribution": m.licence.attribution,
        },
        "notes": m.notes,
        "installed": installed,
        "selected": selected,
        "download": dl_val,
    })
}

impl Session {
    /// The queue, made on first use with the engine's GPU probe and activity row.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn ml_queue(&mut self) -> &Queue<Output> {
        let preference = self.ml.preference.unwrap_or_else(env_preference);
        self.ml_queue_with(Config { preference, ..Config::default() }, Box::new(gpu_probe))
    }

    /// The queue, made with `probe` and `config` if it doesn't exist yet (tests pick the GPU this way).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn ml_queue_with(&mut self, config: Config, probe: Probe) -> &Queue<Output> {
        let activity = self.activity.clone();
        self.ml.queue.get_or_insert_with(|| Queue::new(config, probe, Some(Arc::new(ActivityRows(activity)) as Arc<dyn Rows>)))
    }

    /// Queue an AI job.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn ml_submit(&mut self, job: Job<Output>) -> Result<(), SubmitError> {
        self.ml_queue().submit(job)
    }

    /// `ml.status {preference?}`.
    pub(crate) fn ml_status(&mut self, p: &Value) -> crate::Result<Value> {
        if let Some(v) = p.get("preference") {
            let pref = v
                .as_str()
                .and_then(Preference::parse)
                .ok_or_else(|| bad("ml.status", "preference is `auto` (the GPU when there is one) or `cpu`"))?;
            self.ml.preference = Some(pref);
            if let Some(q) = &self.ml.queue {
                q.set_preference(pref);
            }
        }
        let mut base = match &self.ml.queue {
            Some(q) => status_json(&q.status()),
            None => status_json(&Status {
                device: None,
                preference: self.ml.preference.unwrap_or_else(env_preference),
                pending: 0,
                running: None,
                done: 0,
                failed: 0,
                cancelled: 0,
                threaded: false,
            }),
        };
        let model_id = self.ml_models_dir.as_deref().and_then(active_embedder_id);
        #[cfg(not(target_arch = "wasm32"))]
        if model_id.is_some() && self.ml.store.is_none() {
            let _ = self.ensure_ml_encoder(false);
        }
        let indexed = self.ml.store.as_ref().map_or(0, Store::len);
        let unindexed = self.unindexed_photo_count();
        if let Some(obj) = base.as_object_mut() {
            obj.insert("enabled".into(), Value::Bool(self.ml.enabled));
            obj.insert("modelId".into(), model_id.as_ref().map(|s| Value::String(s.clone())).unwrap_or(Value::Null));
            obj.insert("modelInstalled".into(), Value::Bool(model_id.is_some()));
            obj.insert("indexedPhotos".into(), json!(indexed));
            obj.insert("unindexedPhotos".into(), json!(unindexed));
        }
        Ok(base)
    }

    fn unindexed_photo_count(&self) -> usize {
        let Some(store) = &self.ml.store else {
            return self.catalog.photos().filter(|p| p.in_library()).count();
        };
        self.catalog.photos().filter(|p| p.in_library()).filter(|p| !store.has(p.id.0, photo_fingerprint(p))).count()
    }

    /// `ml.enable {enabled: bool}`.
    pub(crate) fn ml_enable(&mut self, p: &Value) -> crate::Result<Value> {
        const C: &str = "ml.enable";
        let enabled = p.get("enabled").and_then(Value::as_bool).ok_or_else(|| bad(C, "missing boolean `enabled`"))?;
        self.ml.enabled = enabled;
        if enabled {
            self.ml.paused_by_user = false;
        } else if let Some(q) = &self.ml.queue {
            for id in self.ml.in_flight.drain() {
                q.cancel(&format!("embed:{}", id.0));
            }
        } else {
            self.ml.in_flight.clear();
        }
        Ok(json!({"enabled": self.ml.enabled}))
    }

    /// `ml.models.list {}`.
    pub(crate) fn ml_models_list(&mut self, _p: &Value) -> crate::Result<Value> {
        let installed = self.ml_models_dir.as_deref().map(installed_manifests).unwrap_or_default();
        let selected = self.ml_models_dir.as_deref().and_then(active_embedder_id);
        let mut models = Vec::new();
        let mut seen = HashSet::new();

        for known in lcc_ml::known::all() {
            let is_installed = installed.iter().any(|m| m.id == known.id);
            let is_selected = selected.as_deref() == Some(known.id.as_str());
            let dl = self.ml.downloads.get(&known.id);
            models.push(manifest_json(&known, is_installed, is_selected, dl.as_ref()));
            seen.insert(known.id);
        }
        for inst in installed {
            if seen.insert(inst.id.clone()) {
                let is_selected = selected.as_deref() == Some(inst.id.as_str());
                let dl = self.ml.downloads.get(&inst.id);
                models.push(manifest_json(&inst, true, is_selected, dl.as_ref()));
            }
        }
        Ok(json!({
            "enabled": self.ml.enabled,
            "runtime": cfg!(not(target_arch = "wasm32")),
            "models": models,
            "selectedEmbedder": selected,
        }))
    }

    /// `ml.models.install {path, acknowledged: true, select?: true}`.
    pub(crate) fn ml_models_install(&mut self, p: &Value) -> crate::Result<Value> {
        const C: &str = "ml.models.install";
        let models_dir = self.ml_models_dir.clone().ok_or_else(|| bad(C, "this session has no ML models folder configured"))?;
        let src_str = p
            .get("path")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| bad(C, "missing `path` to model directory"))?;
        let acknowledged =
            p.get("acknowledged").or_else(|| p.get("acceptLicence")).or_else(|| p.get("acceptLicense")).and_then(Value::as_bool).unwrap_or(false);
        if !acknowledged {
            return Err(bad(C, "pass `acknowledged: true` to confirm the model's licence before installing"));
        }
        let select = p.get("select").and_then(Value::as_bool).unwrap_or(true);
        let src_dir = PathBuf::from(src_str);

        let manifest = ModelManifest::verify_dir(&src_dir).map_err(|e| bad(C, e.to_string()))?;
        let dst_dir = models_dir.join(&manifest.id);
        let same_dir = std::fs::canonicalize(&src_dir).ok().zip(std::fs::canonicalize(&dst_dir).ok()).is_some_and(|(a, b)| a == b);

        if !same_dir {
            let tmp_dir = models_dir.join(format!(".install-{}-{}", manifest.id, std::process::id()));
            let _ = std::fs::remove_dir_all(&tmp_dir);
            std::fs::create_dir_all(&tmp_dir).map_err(|e| bad(C, format!("cannot create {}: {e}", tmp_dir.display())))?;
            for f in &manifest.files {
                std::fs::copy(src_dir.join(&f.name), tmp_dir.join(&f.name)).map_err(|e| {
                    let _ = std::fs::remove_dir_all(&tmp_dir);
                    bad(C, format!("cannot copy {}: {e}", f.name))
                })?;
            }
            let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|e| bad(C, e.to_string()))?;
            std::fs::write(tmp_dir.join(lcc_ml::MANIFEST_FILE), manifest_bytes).map_err(|e| {
                let _ = std::fs::remove_dir_all(&tmp_dir);
                bad(C, format!("cannot write manifest.json: {e}"))
            })?;
            ModelManifest::verify_dir(&tmp_dir).map_err(|e| {
                let _ = std::fs::remove_dir_all(&tmp_dir);
                bad(C, e.to_string())
            })?;
            let _ = std::fs::remove_dir_all(&dst_dir);
            std::fs::rename(&tmp_dir, &dst_dir).map_err(|e| {
                let _ = std::fs::remove_dir_all(&tmp_dir);
                bad(C, format!("cannot install model to {}: {e}", dst_dir.display()))
            })?;
        }

        if select && manifest.role == Role::Embedder {
            let mut settings = read_settings(&models_dir);
            settings.embedder = Some(manifest.id.clone());
            write_settings(&models_dir, &settings).map_err(|e| bad(C, e))?;
            self.ml.last_checked = None;
            self.ml.paused_by_user = false;
        }
        let selected_id = active_embedder_id(&models_dir);
        let selected = selected_id.as_deref() == Some(manifest.id.as_str());
        let info = manifest_json(&manifest, true, selected, None);
        Ok(json!({
            "installed": info,
            "selected": selected_id,
        }))
    }

    /// `ml.models.select {id}`.
    pub(crate) fn ml_models_select(&mut self, p: &Value) -> crate::Result<Value> {
        const C: &str = "ml.models.select";
        let models_dir = self.ml_models_dir.clone().ok_or_else(|| bad(C, "this session has no ML models folder configured"))?;
        let id = match p.get("id") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
            _ => return Err(bad(C, "`id` must be a model id string or null")),
        };
        if let Some(ref chosen) = id {
            if !valid_id(chosen) {
                return Err(bad(C, format!("invalid model id {chosen:?}")));
            }
            let installed = installed_manifests(&models_dir);
            if !installed.iter().any(|m| &m.id == chosen && m.role == Role::Embedder) {
                return Err(bad(C, format!("embedder model {chosen:?} is not installed")));
            }
        }
        let mut settings = read_settings(&models_dir);
        settings.embedder = id.clone();
        write_settings(&models_dir, &settings).map_err(|e| bad(C, e))?;
        self.ml.last_checked = None;
        self.ml.paused_by_user = false;
        Ok(json!({"selectedEmbedder": id}))
    }

    /// `ml.models.remove {id}`.
    pub(crate) fn ml_models_remove(&mut self, p: &Value) -> crate::Result<Value> {
        const C: &str = "ml.models.remove";
        let models_dir = self.ml_models_dir.clone().ok_or_else(|| bad(C, "this session has no ML models folder configured"))?;
        let id = p.get("id").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| bad(C, "missing `id`"))?;
        if !valid_id(id) {
            return Err(bad(C, format!("invalid model id {id:?}")));
        }
        let target = models_dir.join(id);
        let removed = if target.is_dir() {
            std::fs::remove_dir_all(&target).map_err(|e| bad(C, format!("cannot remove {}: {e}", target.display())))?;
            true
        } else {
            false
        };
        let mut settings = read_settings(&models_dir);
        if settings.embedder.as_deref() == Some(id) {
            settings.embedder = None;
            let _ = write_settings(&models_dir, &settings);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if self.ml.encoder.as_ref().is_some_and(|(_, e)| e.manifest().id == id) {
            self.ml.save_store_now();
            self.ml.encoder = None;
            self.ml.store = None;
            self.ml.store_path = None;
            if let Some(q) = &self.ml.queue {
                for pid in self.ml.in_flight.drain() {
                    q.cancel(&format!("embed:{}", pid.0));
                }
            }
        }
        self.ml.last_checked = None;
        Ok(json!({"id": id, "removed": removed}))
    }

    /// `ml.models.download {id, acceptLicence?: true}`.
    pub(crate) fn ml_models_download(&mut self, p: &Value) -> crate::Result<Value> {
        const C: &str = "ml.models.download";
        let models_dir = self.ml_models_dir.clone().ok_or_else(|| bad(C, "this session has no ML models folder configured"))?;
        let id = p.get("id").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| bad(C, "missing `id`"))?;
        let spec = lcc_ml::known::download(id).ok_or_else(|| bad(C, format!("unknown downloadable ML model {id:?}")))?;
        let accepted = p.get("acceptLicence").or_else(|| p.get("acceptLicense")).and_then(Value::as_bool).unwrap_or(false);
        if !accepted {
            return Err(bad(
                C,
                format!(
                    "pass `acceptLicence: true` to confirm downloading {} ({}, {} bytes)",
                    spec.manifest.name,
                    spec.manifest.licence.spdx,
                    spec.manifest.total_bytes()
                ),
            ));
        }
        if matches!(self.ml.downloads.get(id), Some(DownloadState::Running { .. })) {
            return Err(bad(C, format!("{id} is already downloading")));
        }
        start_model_download(&mut self.ml.downloads, spec.clone(), &models_dir, &self.activity).map_err(|e| bad(C, e))?;
        Ok(json!({
            "id": spec.manifest.id,
            "name": spec.manifest.name,
            "sizeBytes": spec.manifest.total_bytes(),
            "licence": spec.manifest.licence.spdx,
            "state": "running",
        }))
    }

    /// `ml.models.downloads {}`.
    pub(crate) fn ml_models_downloads(&mut self, _p: &Value) -> crate::Result<Value> {
        let list: Vec<Value> = self
            .ml
            .downloads
            .snapshot()
            .into_iter()
            .map(|(id, state)| match state {
                DownloadState::Running { bytes, total } => {
                    json!({"id": id, "state": "running", "bytes": bytes, "total": total})
                }
                DownloadState::Installed => json!({"id": id, "state": "installed"}),
                DownloadState::Failed(err) => json!({"id": id, "state": "failed", "error": err}),
                DownloadState::Cancelled => json!({"id": id, "state": "cancelled"}),
            })
            .collect();
        Ok(json!({"downloads": list}))
    }

    /// `ml.models.downloadCancel {id}`.
    pub(crate) fn ml_models_download_cancel(&mut self, p: &Value) -> crate::Result<Value> {
        const C: &str = "ml.models.downloadCancel";
        let id = p.get("id").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| bad(C, "missing `id`"))?;
        let cancelled = self.ml.downloads.cancel(id);
        Ok(json!({"id": id, "cancelled": cancelled}))
    }
}

fn start_model_download(downloads: &mut Downloads, spec: Download, models_dir: &Path, activity: &Activity) -> Result<(), String> {
    let staging_root = models_dir.join(STAGING_DIR).join(&spec.manifest.id);
    std::fs::create_dir_all(&staging_root).map_err(|e| format!("could not create download folder: {e}"))?;
    let total = spec.manifest.total_bytes();
    let cancel = Arc::new(AtomicBool::new(false));
    let task = activity.start("download", "Downloading AI model", Cancel::Flag(cancel.clone()));
    task.set_unit(Unit::Bytes);
    task.detail(&spec.manifest.name);
    task.progress(0, total);
    let shared = Arc::new(DownloadShared { state: Mutex::new(DownloadState::Running { bytes: 0, total }), cancel, task: Some(task.handle()) });
    let worker = Arc::clone(&shared);
    let id = spec.manifest.id.clone();
    let dst_models_dir = models_dir.to_path_buf();

    std::thread::Builder::new()
        .name("ml-model-download".into())
        .spawn(move || {
            let _task = task;
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_model_download(&spec, &staging_root, &dst_models_dir, &worker);
            }));
            if outcome.is_err() {
                worker.set(DownloadState::Failed("the model download stopped unexpectedly".into()));
            }
        })
        .map_err(|e| format!("could not start download thread: {e}"))?;
    downloads.jobs.insert(id, shared);
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn run_model_download(spec: &Download, staging_dir: &Path, models_dir: &Path, shared: &DownloadShared) {
    let total = spec.manifest.total_bytes();
    let file_specs: Vec<lcc_fetch::FileSpec<'_>> = spec
        .files
        .iter()
        .map(|f| lcc_fetch::FileSpec { name: f.name, size: Some(f.size_bytes), sha256: Some(f.sha256), max: f.size_bytes })
        .collect();
    let base_mirror = lcc_ml::known::CLIP_VIT_B32_BASE_URL.to_string();
    let res = lcc_fetch::download(&file_specs, &[base_mirror], staging_dir, &lcc_fetch::Options::default(), &shared.cancel, &mut |p| {
        shared.set(DownloadState::Running { bytes: p.done.min(total), total });
    });
    match res {
        Ok(()) => {
            let manifest_bytes = match serde_json::to_vec_pretty(&spec.manifest) {
                Ok(b) => b,
                Err(e) => {
                    shared.set(DownloadState::Failed(e.to_string()));
                    return;
                }
            };
            if let Err(e) = std::fs::write(staging_dir.join(lcc_ml::MANIFEST_FILE), manifest_bytes) {
                shared.set(DownloadState::Failed(format!("cannot write manifest.json: {e}")));
                return;
            }
            if let Err(e) = ModelManifest::verify_dir(staging_dir) {
                shared.set(DownloadState::Failed(e.to_string()));
                return;
            }
            let final_dir = models_dir.join(&spec.manifest.id);
            let _ = std::fs::remove_dir_all(&final_dir);
            if let Err(e) = std::fs::rename(staging_dir, &final_dir) {
                shared.set(DownloadState::Failed(format!("cannot move verified model into place: {e}")));
                return;
            }
            let mut settings = read_settings(models_dir);
            if settings.embedder.is_none() {
                settings.embedder = Some(spec.manifest.id.clone());
                let _ = write_settings(models_dir, &settings);
            }
            shared.set(DownloadState::Installed);
        }
        Err(lcc_fetch::DownloadError::Cancelled) => {
            let _ = std::fs::remove_dir_all(staging_dir);
            shared.set(DownloadState::Cancelled);
        }
        Err(e) => {
            shared.set(DownloadState::Failed(e.to_string()));
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn run_model_download(_spec: &Download, _staging_dir: &Path, _models_dir: &Path, shared: &DownloadShared) {
    shared.set(DownloadState::Failed("the web build does not download ML models".into()));
}

#[cfg(not(target_arch = "wasm32"))]
impl Session {
    /// Ensure the active embedder model is loaded and its [`Store`] is attached for the current library.
    fn ensure_ml_encoder(&mut self, fresh: bool) -> Result<Arc<dyn Encoder>, String> {
        if !fresh
            && let Some((_, enc)) = &self.ml.encoder
            && self.ml.last_checked.is_some_and(|t| t.elapsed() < MODEL_RECHECK)
        {
            return Ok(Arc::clone(enc));
        }
        let models_dir = self.ml_models_dir.clone().ok_or_else(|| "no ML models folder is configured".to_string())?;
        let id = active_embedder_id(&models_dir).ok_or_else(|| {
            "no AI embedding model is installed (install or download one with `ml.models.install` or `ml.models.download`)".to_string()
        })?;
        let model_dir = models_dir.join(&id);
        let manifest = ModelManifest::read_dir(&model_dir).map_err(|e| e.to_string())?;
        let tag = manifest.tag();
        self.ml.last_checked = Some(web_time::Instant::now());

        let expected_store_path = self.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.join(store_file_name(&id)));

        if let Some((loaded_tag, enc)) = &self.ml.encoder
            && *loaded_tag == tag
        {
            let enc = Arc::clone(enc);
            self.ensure_ml_store(&tag, manifest.embed_dim, expected_store_path)?;
            return Ok(enc);
        }

        let clip = lcc_ml::Clip::load(&model_dir).map_err(|e| e.to_string())?;
        let enc: Arc<dyn Encoder> = Arc::new(clip);
        self.ml.save_store_now();
        self.ml.encoder = Some((tag.clone(), Arc::clone(&enc)));
        self.ml.store = None;
        self.ml.store_path = None;
        self.ml.in_flight.clear();
        self.ml.failed_photos.clear();
        self.ml.epoch = self.ml.epoch.wrapping_add(1);
        self.ensure_ml_store(&tag, manifest.embed_dim, expected_store_path)?;
        Ok(enc)
    }

    fn ensure_ml_store(&mut self, tag: &str, dim: u32, expected_path: Option<PathBuf>) -> Result<(), String> {
        let needs_reload = match &self.ml.store {
            None => true,
            Some(st) => st.tag() != tag || st.dim() != dim || self.ml.store_path != expected_path,
        };
        if needs_reload {
            self.ml.save_store_now();
            let store = match &expected_path {
                Some(path) => Store::load_or_reset(path, tag, dim),
                None => Store::new(tag, dim).map_err(|e| e.to_string())?,
            };
            self.ml.store = Some(store);
            self.ml.store_path = expected_path;
        }
        Ok(())
    }

    /// Drain finished `embed:*` jobs from `ml.queue` into `ml.store`.
    pub(crate) fn drain_ml_results(&mut self) -> (usize, usize) {
        let Some(q) = &self.ml.queue else {
            return (0, 0);
        };
        let finished = q.drain();
        let busy = q.busy();
        let mut added = 0usize;
        let mut failed = 0usize;
        for f in finished {
            if let Some(rest) = f.key.strip_prefix("embed:")
                && let Ok(raw_id) = rest.parse::<u64>()
            {
                let pid = PhotoId(raw_id);
                self.ml.in_flight.remove(&pid);
                match f.outcome {
                    Outcome::Done(any) => {
                        if let Ok(out) = any.downcast::<EmbedOutput>()
                            && out.epoch == self.ml.epoch
                            && let Some(photo) = self.catalog.photo(out.photo_id)
                            && photo.in_library()
                            && photo_fingerprint(photo) == out.fingerprint
                            && let Some(store) = &mut self.ml.store
                            && store.tag() == out.tag
                            && store.insert(out.photo_id.0, out.fingerprint, out.embedding).is_ok()
                        {
                            added += 1;
                        }
                    }
                    Outcome::Failed(_) => {
                        failed += 1;
                        if let Some(photo) = self.catalog.photo(pid) {
                            self.ml.failed_photos.insert((pid.0, photo_fingerprint(photo)));
                        }
                    }
                    Outcome::Cancelled => {
                        self.ml.paused_by_user = true;
                        self.ml.in_flight.clear();
                    }
                }
            }
        }
        let should_save =
            self.ml.store.as_ref().is_some_and(Store::is_dirty) && (!busy || self.ml.last_saved.is_none_or(|t| t.elapsed() >= SAVE_EVERY));
        if should_save {
            self.ml.save_store_now();
        }
        (added, failed)
    }

    /// Submit unindexed photos to `ml_queue`.
    fn queue_ml_photos(&mut self, encoder: &Arc<dyn Encoder>, targets: Option<&[PhotoId]>, priority: Priority) -> usize {
        let Some(store) = &self.ml.store else {
            return 0;
        };
        let tag = store.tag().to_string();
        let epoch = self.ml.epoch;
        let candidate_ids: Vec<PhotoId> = match targets {
            Some(ids) => ids.to_vec(),
            None => self.catalog.photos().filter(|p| p.in_library()).map(|p| p.id).collect(),
        };

        let mut to_submit = Vec::new();
        for id in candidate_ids {
            if self.ml.in_flight.contains(&id) {
                continue;
            }
            let Some(photo) = self.catalog.photo(id) else {
                continue;
            };
            if !photo.in_library() {
                continue;
            }
            let fp = photo_fingerprint(photo);
            if self.ml.store.as_ref().is_some_and(|st| st.has(id.0, fp)) {
                continue;
            }
            if targets.is_none() && self.ml.failed_photos.contains(&(id.0, fp)) {
                continue;
            }
            to_submit.push((id, fp, photo.file_name.clone()));
        }

        let mut submitted = 0usize;
        for (id, fp, name) in to_submit {
            let Some(photo) = self.catalog.photo(id).cloned() else {
                continue;
            };
            let Some(render_job) = self.preview_job(id, EMBED_PREVIEW_EDGE, EMBED_PREVIEW_EDGE, false, &DevelopSettings::default()) else {
                continue;
            };
            let mut render_job = Some(render_job);
            let mut decoded: Option<RgbImage> = None;
            let preview = self.scan_preview_of(&photo);
            let enc = Arc::clone(encoder);
            let job_tag = tag.clone();
            let pace_ref = Arc::clone(&self.ml.pace);
            let key = format!("embed:{}", id.0);

            let job = Job::new(key, move |ctx: &Ctx| -> Result<Output, JobError> {
                ctx.check()?;
                if priority == Priority::Background {
                    while Pace::from_u8(pace_ref.load(Ordering::Relaxed)) == Pace::Pause {
                        ctx.check()?;
                        std::thread::sleep(PAUSE_POLL);
                    }
                    if Pace::from_u8(pace_ref.load(Ordering::Relaxed)) == Pace::Light {
                        std::thread::sleep(LIGHT_REST);
                        ctx.check()?;
                    }
                }
                let rgb_img = if let Some(img) = decoded.as_ref() {
                    img.clone()
                } else {
                    let rgba = preview
                        .as_ref()
                        .and_then(|(path, load)| load(path, EMBED_PREVIEW_EDGE))
                        .filter(|img| img.width.max(img.height) >= MIN_RAW_PREVIEW_EDGE)
                        .or_else(|| render_job.take().and_then(|j| j.run().rendered.ok().map(|r| r.image)))
                        .ok_or_else(|| JobError::Failed(format!("cannot decode preview for photo {}", id.0)))?;
                    ctx.check()?;

                    let mut rgb = Vec::with_capacity(rgba.data.len() * 3);
                    for px in &rgba.data {
                        rgb.extend_from_slice(&[px[0], px[1], px[2]]);
                    }
                    let img = RgbImage::new(rgba.width as u32, rgba.height as u32, rgb).map_err(JobError::Failed)?;
                    decoded = Some(img.clone());
                    img
                };
                let mut vecs = enc.encode_images(&[rgb_img], ctx)?;
                let embedding = vecs.pop().ok_or_else(|| JobError::Failed("empty embedding batch output".into()))?;
                Ok(Box::new(EmbedOutput { epoch, tag: job_tag.clone(), photo_id: id, fingerprint: fp, embedding }))
            })
            .name(name)
            .priority(priority);

            if self.ml_submit(job).is_ok() {
                self.ml.in_flight.insert(id);
                submitted += 1;
            } else {
                break;
            }
        }
        submitted
    }

    /// `ml.pump {pace?: pause | light | normal | full}`.
    pub(crate) fn ml_pump(&mut self, p: &Value) -> crate::Result<Value> {
        let pace = Pace::parse(p.get("pace").and_then(Value::as_str));
        self.ml.pace.store(pace as u8, Ordering::Relaxed);
        let (added, _) = self.drain_ml_results();

        let mut ready = false;
        if self.ml.enabled
            && !self.ml.paused_by_user
            && let Ok(enc) = self.ensure_ml_encoder(false)
        {
            ready = true;
            if pace != Pace::Pause {
                let _ = self.queue_ml_photos(&enc, None, Priority::Background);
            }
        }
        let indexed = self.ml.store.as_ref().map_or(0, Store::len);
        let unindexed = self.unindexed_photo_count();
        let in_flight = self.ml.in_flight.len();
        let (pending, running, done, failed) = match &self.ml.queue {
            Some(q) => {
                let st = q.status();
                (st.pending, st.running, st.done, st.failed)
            }
            None => (0, None, 0, 0),
        };
        let active = ready && self.ml.enabled && !self.ml.paused_by_user;
        Ok(json!({
            "active": active,
            "ready": ready,
            "enabled": self.ml.enabled,
            "paused": self.ml.paused_by_user,
            "added": added,
            "indexedPhotos": indexed,
            "unindexedPhotos": unindexed,
            "pendingPhotos": unindexed,
            "inFlight": in_flight,
            "pending": pending,
            "running": running,
            "done": done,
            "failed": failed,
        }))
    }

    /// `ml.index {ids?, wait?: false, budgetMs?: 30000}`.
    pub(crate) fn ml_index(&mut self, p: &Value) -> crate::Result<Value> {
        const C: &str = "ml.index";
        let explicit_ids: Option<Vec<PhotoId>> = match p.get("ids") {
            None | Some(Value::Null) => None,
            Some(Value::Array(arr)) => {
                if let Some(why) = crate::bad_ids_entry(arr) {
                    return Err(bad(C, why));
                }
                Some(arr.iter().filter_map(|v| v.as_u64().map(PhotoId)).collect())
            }
            Some(_) => return Err(bad(C, "`ids` must be an array of photo ids")),
        };
        let wait = p.get("wait").and_then(Value::as_bool).unwrap_or(false) || p.get("budgetMs").is_some();
        let budget_ms = p.get("budgetMs").and_then(Value::as_u64).unwrap_or(30_000).clamp(1, 300_000);

        let enc = self.ensure_ml_encoder(true).map_err(|e| bad(C, e))?;
        self.ml.paused_by_user = false;
        self.ml.pace.store(Pace::Normal as u8, Ordering::Relaxed);
        match &explicit_ids {
            Some(ids) => {
                for id in ids {
                    self.ml.failed_photos.retain(|&(pid, _)| pid != id.0);
                }
            }
            None => self.ml.failed_photos.clear(),
        }
        self.drain_ml_results();
        let priority = if wait { Priority::Interactive } else { Priority::Background };
        let queued = self.queue_ml_photos(&enc, explicit_ids.as_deref(), priority);

        if wait {
            let started = web_time::Instant::now();
            let budget = Duration::from_millis(budget_ms);
            while started.elapsed() < budget {
                self.drain_ml_results();
                let done = match &explicit_ids {
                    Some(ids) => ids.iter().all(|id| !self.ml.in_flight.contains(id)),
                    None => self.ml.in_flight.is_empty() && self.ml.queue.as_ref().is_none_or(|q| !q.busy()),
                };
                if done {
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            self.drain_ml_results();
            self.ml.save_store_now();
        }

        let indexed = self.ml.store.as_ref().map_or(0, Store::len);
        let unindexed = self.unindexed_photo_count();
        Ok(json!({
            "queued": queued,
            "indexedPhotos": indexed,
            "unindexedPhotos": unindexed,
            "inFlight": self.ml.in_flight.len(),
        }))
    }

    /// `search.semantic {text, topK?: 50, threshold?: -1.0, scope?: "all" | "filtered" | "selected"}`.
    pub(crate) fn search_semantic(&mut self, p: &Value) -> crate::Result<Value> {
        const C: &str = "search.semantic";
        let text = p
            .get("text")
            .or_else(|| p.get("query"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| bad(C, "missing non-empty `text` query"))?
            .to_string();
        let top_k = match p.get("topK") {
            None | Some(Value::Null) => 50usize,
            Some(v) => v
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .filter(|&n| (1..=1000).contains(&n))
                .ok_or_else(|| bad(C, "`topK` must be an integer between 1 and 1000"))?,
        };
        let threshold = match p.get("threshold") {
            None | Some(Value::Null) => -1.0f32,
            Some(v) => v
                .as_f64()
                .map(|x| x as f32)
                .filter(|x| x.is_finite() && (-1.0..=1.0).contains(x))
                .ok_or_else(|| bad(C, "`threshold` must be a number between -1.0 and 1.0"))?,
        };
        let scope = p.get("scope").and_then(Value::as_str).unwrap_or("all");
        let allowed: HashSet<u64> = match scope {
            "all" => self.catalog.photos().filter(|ph| ph.in_library()).map(|ph| ph.id.0).collect(),
            "filtered" => self.visible().iter().map(|id| id.0).collect(),
            "selected" => self.selected().into_iter().map(|id| id.0).collect(),
            other => {
                return Err(bad(C, format!("`scope` must be `all`, `filtered` or `selected` (got {other:?})")));
            }
        };

        let enc = self.ensure_ml_encoder(false).map_err(|e| bad(C, e))?;
        let model_id = enc.manifest().id.clone();
        self.drain_ml_results();

        self.ml.query_seq = self.ml.query_seq.wrapping_add(1);
        let qkey = format!("query:{}", self.ml.query_seq);
        let qtext = text.clone();
        let qenc = Arc::clone(&enc);
        self.ml_submit(
            Job::new(qkey.clone(), move |ctx: &Ctx| -> Result<Output, JobError> {
                let vec = qenc.encode_text(&qtext, ctx)?;
                Ok(Box::new(QueryOutput(vec)))
            })
            .priority(Priority::Interactive),
        )
        .map_err(|e| bad(C, e.to_string()))?;

        let started = web_time::Instant::now();
        let mut query_vec: Option<Vec<f32>> = None;
        while started.elapsed() < Duration::from_secs(30) {
            let finished = self.ml.queue.as_ref().map(|q| q.drain()).unwrap_or_default();
            for f in finished {
                if f.key == qkey {
                    match f.outcome {
                        Outcome::Done(any) => {
                            if let Ok(qout) = any.downcast::<QueryOutput>() {
                                query_vec = Some(qout.0);
                            }
                        }
                        Outcome::Failed(why) => return Err(bad(C, why)),
                        Outcome::Cancelled => {
                            return Err(crate::EngineError::Cancelled("semantic search cancelled".into()));
                        }
                    }
                } else if let Some(rest) = f.key.strip_prefix("embed:")
                    && let Ok(raw_id) = rest.parse::<u64>()
                {
                    let pid = PhotoId(raw_id);
                    self.ml.in_flight.remove(&pid);
                    if let Outcome::Done(any) = f.outcome
                        && let Ok(out) = any.downcast::<EmbedOutput>()
                        && out.epoch == self.ml.epoch
                        && let Some(photo) = self.catalog.photo(out.photo_id)
                        && photo.in_library()
                        && photo_fingerprint(photo) == out.fingerprint
                        && let Some(store) = &mut self.ml.store
                        && store.tag() == out.tag
                    {
                        let _ = store.insert(out.photo_id.0, out.fingerprint, out.embedding);
                    }
                }
            }
            if query_vec.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }

        let qvec = query_vec.ok_or_else(|| bad(C, "semantic search timed out waiting for query embedding"))?;
        let hits = self
            .ml
            .store
            .as_ref()
            .map(|st| st.search(&qvec, top_k, threshold, Some(&allowed)))
            .transpose()
            .map_err(|e| bad(C, e.to_string()))?
            .unwrap_or_default();
        let indexed = self.ml.store.as_ref().map_or(0, Store::len);
        let pending = self.unindexed_photo_count();

        let rows: Vec<Value> = hits.iter().map(|h| json!({"id": h.photo_id, "score": h.score})).collect();
        Ok(json!({
            "query": text,
            "modelId": model_id,
            "hits": rows,
            "matches": rows,
            "indexedPhotos": indexed,
            "pendingPhotos": pending,
        }))
    }
}

#[cfg(target_arch = "wasm32")]
impl Session {
    pub(crate) fn ml_pump(&mut self, _p: &Value) -> crate::Result<Value> {
        Ok(json!({
            "active": false,
            "ready": false,
            "enabled": self.ml.enabled,
            "paused": false,
            "added": 0,
            "indexedPhotos": 0,
            "unindexedPhotos": 0,
            "pendingPhotos": 0,
            "inFlight": 0,
            "pending": 0,
            "running": null,
            "done": 0,
            "failed": 0,
        }))
    }

    pub(crate) fn ml_index(&mut self, _p: &Value) -> crate::Result<Value> {
        Err(bad("ml.index", "ML indexing is not available in the web build"))
    }

    pub(crate) fn search_semantic(&mut self, _p: &Value) -> crate::Result<Value> {
        Err(bad("search.semantic", "semantic search is not available in the web build"))
    }
}
