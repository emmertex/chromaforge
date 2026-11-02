//! Every AI mask model as the session sees it (`ai.models.*`): SAM 3 and the Subject model keep
//! their own folders and downloads; the openly licensed models of [`lcc_segment::models`]
//! (SAM 2.1, Depth Anything V2 Small) live in `<models folder>/<dir_name>/`.
//!
//! Which model computes Object clicks is the user's choice ([`ObjectModel`]): `auto` keeps
//! SAM 3 when it is installed (as before) and otherwise uses SAM 2.1 when that is installed.
//! Describe (text) needs SAM 3 either way.
//!
//! Nothing is downloaded unless asked for (`ai.models.download {"id", "acknowledged": true}`);
//! each download is a row in the activity stack, can be cancelled, resumes, and every file is
//! checked against its pinned size and SHA-256.

use std::path::PathBuf;

use super::{DownloadStatus, Segmenter};

#[cfg(feature = "sam")]
use lcc_segment::models as reg;

/// Builds without AI masks know no openly licensed models.
#[cfg(not(feature = "sam"))]
mod reg {
    use std::path::Path;

    pub struct File {
        pub name: &'static str,
    }

    pub struct ModelInfo {
        pub key: &'static str,
        pub dir_name: &'static str,
        pub title: &'static str,
        pub purpose: &'static str,
        pub files: &'static [File],
        pub license_name: &'static str,
        pub license_url: &'static str,
        pub mirrors_env: &'static str,
        pub offered: bool,
    }

    impl ModelInfo {
        pub fn size(&self) -> u64 {
            0
        }
        pub fn installed_in(&self, _: &Path) -> bool {
            false
        }
        pub fn mirrors(&self, _: Option<&str>, _: Option<&Path>) -> Vec<String> {
            Vec::new()
        }
        pub fn mirrors_file_name(&self) -> String {
            String::new()
        }
    }

    pub const MODELS: &[&ModelInfo] = &[];

    pub fn by_key(_: &str) -> Option<&'static ModelInfo> {
        None
    }
}

/// Which model computes Object (click) selections.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ObjectModel {
    /// SAM 3 when installed, else SAM 2.1 when installed.
    #[default]
    Auto,
    Sam3,
    Sam2,
}

impl ObjectModel {
    pub fn key(self) -> &'static str {
        match self {
            ObjectModel::Auto => "auto",
            ObjectModel::Sam3 => "sam3",
            ObjectModel::Sam2 => "sam2",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "auto" => Some(ObjectModel::Auto),
            "sam3" => Some(ObjectModel::Sam3),
            "sam2" => Some(ObjectModel::Sam2),
            _ => None,
        }
    }
}

/// The persisted choices (`<models folder>/ai-models.json`).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Prefs {
    object_model: ObjectModel,
}

/// One model, for `ai.models.status` and the model dialog.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatus {
    /// The command key (`"sam3"`, `"subject"`, `"sam2"`, `"depth"`).
    pub id: String,
    pub title: String,
    /// What it is for.
    pub purpose: String,
    pub installed: bool,
    pub dir: Option<String>,
    pub size_bytes: u64,
    pub license: String,
    pub license_url: String,
    /// Shown in the model dialog (others are downloadable by command only).
    pub offered: bool,
    pub download: DownloadStatus,
}

#[cfg(feature = "sam")]
fn status_of(s: super::download::Status) -> DownloadStatus {
    DownloadStatus { running: s.running, done: s.done, total: s.total, file: s.file, error: s.error, cancelled: s.cancelled, finished: s.finished }
}

impl Segmenter {
    /// Where the openly licensed models live: next to SAM 3's folder unless set.
    fn open_root(&self) -> Option<PathBuf> {
        self.models_root.clone().or_else(|| self.dir.as_ref().and_then(|d| d.parent().map(PathBuf::from)))
    }

    /// The folder of openly licensed model `key` (`None`: unknown, or no models folder).
    pub fn open_model_dir(&self, key: &str) -> Option<PathBuf> {
        let m = reg::by_key(key)?;
        self.open_root().map(|r| r.join(m.dir_name))
    }

    /// Whether openly licensed model `key` is installed.
    pub fn open_model_installed(&self, key: &str) -> bool {
        match (reg::by_key(key), self.open_model_dir(key)) {
            (Some(m), Some(dir)) => m.installed_in(&dir),
            _ => false,
        }
    }

    fn open_mirrors(&self, m: &reg::ModelInfo) -> Vec<String> {
        let env = lcc_brand::env(m.mirrors_env);
        let file = self.open_root().map(|r| r.join(m.mirrors_file_name()));
        m.mirrors(env.as_deref(), file.as_deref())
    }

    /// The user's choice of Object model.
    pub fn object_model(&self) -> ObjectModel {
        self.object_model
    }

    /// Set (and, when the session has a models folder, save) the Object model.
    pub fn set_object_model(&mut self, m: ObjectModel) -> Result<(), String> {
        self.object_model = m;
        let Some(root) = self.open_root() else { return Ok(()) };
        let text = serde_json::to_string_pretty(&Prefs { object_model: m }).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&root).map_err(|e| format!("couldn't save the AI model choice: {e}"))?;
        std::fs::write(root.join(PREFS_FILE), text).map_err(|e| format!("couldn't save the AI model choice: {e}"))
    }

    /// Read the saved Object model choice (a missing or damaged file keeps the default).
    pub fn load_prefs(&mut self) {
        let Some(path) = self.open_root().map(|r| r.join(PREFS_FILE)) else { return };
        let Ok(meta) = std::fs::metadata(&path) else { return };
        if meta.len() > 64 * 1024 {
            return;
        }
        if let Some(p) = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<Prefs>(&t).ok()) {
            self.object_model = p.object_model;
        }
    }

    /// The model Object clicks use now: `"sam3"` or `"sam2"`.
    pub fn object_model_in_use(&self) -> &'static str {
        match self.object_model {
            ObjectModel::Sam3 => "sam3",
            ObjectModel::Sam2 => "sam2",
            ObjectModel::Auto if !self.installed() && self.remote_endpoint().is_none() && self.open_model_installed("sam2") => "sam2",
            ObjectModel::Auto => "sam3",
        }
    }

    /// The folder of the model Object clicks use, when it is installed; otherwise
    /// [`crate::EngineError::ModelMissing`] saying how to get it.
    pub fn require_object_model(&self) -> crate::Result<PathBuf> {
        if self.object_model_in_use() != "sam2" {
            return self.require_model();
        }
        self.require_open_model("sam2", crate::AiModel::Sam2)
    }

    /// The folder of openly licensed model `key` when it is installed; otherwise
    /// [`crate::EngineError::ModelMissing`] (`model`) saying how to get it.
    pub fn require_open_model(&self, key: &str, model: crate::AiModel) -> crate::Result<PathBuf> {
        if !Self::AVAILABLE {
            return Err(crate::EngineError::Other("AI masks are not available in this build".into()));
        }
        let m = reg::by_key(key).ok_or_else(|| crate::EngineError::Other("AI masks are not available in this build".into()))?;
        let dir = self.open_model_dir(m.key).ok_or_else(|| crate::EngineError::Other("no folder is set for AI models".into()))?;
        if m.installed_in(&dir) {
            return Ok(dir);
        }
        let d = self.open_download_status(m.key);
        let msg = if d.running {
            let pct = d.done.saturating_mul(100).checked_div(d.total).unwrap_or(0);
            format!("The {} model is not installed yet: it is downloading ({pct} %).", m.title)
        } else {
            format!(
                "The {} model is not installed. Download it (about {:.0} MB, {}) with `ai.models.download {{\"id\": \"{}\", \"acknowledged\": true}}`, or put {} in {}.",
                m.title,
                m.size() as f64 / 1e6,
                m.license_name,
                m.key,
                m.files.iter().map(|f| f.name).collect::<Vec<_>>().join(", "),
                dir.display()
            )
        };
        Err(crate::EngineError::ModelMissing { model, msg })
    }

    /// [`Segmenter::require_object_model`] as text.
    pub fn object_dir(&self) -> Result<PathBuf, String> {
        self.require_object_model().map_err(|e| e.to_string())
    }

    fn open_download_status(&self, key: &str) -> DownloadStatus {
        #[cfg(feature = "sam")]
        if let Some(d) = self.model_downloads.get(key) {
            return status_of(d.status());
        }
        let _ = key;
        DownloadStatus::default()
    }

    /// Every model, with its state.
    pub fn model_status(&self) -> Vec<ModelStatus> {
        let mut out = vec![
            ModelStatus {
                id: "sam3".into(),
                title: "SAM 3".into(),
                purpose: "Object and Describe masks (Meta's gated model)".into(),
                installed: self.installed(),
                dir: self.dir.as_ref().map(|d| d.display().to_string()),
                size_bytes: super::MODEL_BYTES,
                license: super::LICENSE_NAME.into(),
                license_url: super::LICENSE_URL.into(),
                offered: true,
                download: self.download_status(),
            },
            ModelStatus {
                id: "subject".into(),
                title: "BiRefNet_lite".into(),
                purpose: "Subject and Background masks".into(),
                installed: self.subject.installed(),
                dir: self.subject.dir.as_ref().map(|d| d.display().to_string()),
                size_bytes: super::subject::MODEL_BYTES,
                license: super::subject::LICENSE_NAME.into(),
                license_url: super::subject::LICENSE_URL.into(),
                offered: true,
                download: self.subject.download_status(),
            },
        ];
        out.extend(reg::MODELS.iter().map(|m| ModelStatus {
            id: m.key.into(),
            title: m.title.into(),
            purpose: m.purpose.into(),
            installed: self.open_model_installed(m.key),
            dir: self.open_model_dir(m.key).map(|d| d.display().to_string()),
            size_bytes: m.size(),
            license: m.license_name.into(),
            license_url: m.license_url.into(),
            offered: m.offered,
            download: self.open_download_status(m.key),
        }));
        out
    }

    /// Start downloading model `key` in the background. `Ok(false)` when it is installed or
    /// downloading already.
    pub fn start_model_download(&mut self, key: &str, activity: &crate::activity::Activity) -> Result<bool, String> {
        match key {
            "sam3" => return self.start_download(activity),
            "subject" => return self.subject.start_download(activity),
            _ => {}
        }
        if !Self::AVAILABLE {
            return Err("AI masks are not available in this build".into());
        }
        let m = reg::by_key(key).ok_or_else(|| format!("unknown AI model `{key}`"))?;
        let dir = self.open_model_dir(key).ok_or("no folder is set for AI models")?;
        #[cfg(feature = "sam")]
        let companions: Vec<_> = m.missing_companions(&dir).into_iter().map(|c| (&c.file, c.base)).collect();
        #[cfg(not(feature = "sam"))]
        let companions: Vec<()> = Vec::new();
        if m.installed_in(&dir) && companions.is_empty() {
            return Ok(false);
        }
        let mirrors = self.open_mirrors(m);
        if mirrors.is_empty() {
            return Err(format!("no download location is configured for {}", m.title));
        }
        #[cfg(feature = "sam")]
        {
            self.model_downloads.entry(m.key).or_default().start(
                m.files,
                companions,
                mirrors,
                dir,
                lcc_segment::fetch::Options::default(),
                activity,
                m.title,
            )
        }
        #[cfg(not(feature = "sam"))]
        {
            let _ = (dir, activity, companions);
            Ok(false)
        }
    }

    /// Stop model `key`'s download (it resumes next time). False when none runs.
    pub fn cancel_model_download(&self, key: &str) -> bool {
        match key {
            "sam3" => self.cancel_download(),
            "subject" => self.subject.cancel_download(),
            #[cfg(feature = "sam")]
            _ => self.model_downloads.get(key).is_some_and(|d| d.cancel()),
            #[cfg(not(feature = "sam"))]
            _ => false,
        }
    }
}

/// The choices file in the models folder.
const PREFS_FILE: &str = "ai-models.json";

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lcc-ai-models-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn auto_keeps_sam3_unless_only_sam2_is_installed() {
        let root = tmp("auto");
        let mut g = Segmenter { models_root: Some(root.clone()), dir: Some(root.join("sam3")), ..Segmenter::default() };
        assert_eq!(g.object_model_in_use(), "sam3");
        g.object_model = ObjectModel::Sam2;
        assert_eq!(g.object_model_in_use(), "sam2");
        // SAM 2.1 chosen but not installed: a model-missing error that names it
        let e = g.require_object_model().unwrap_err();
        assert!(matches!(e, crate::EngineError::ModelMissing { model: crate::AiModel::Sam2, .. }), "{e}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_choice_is_saved_and_a_damaged_file_keeps_the_default() {
        let root = tmp("prefs");
        let mut g = Segmenter { models_root: Some(root.clone()), ..Segmenter::default() };
        g.set_object_model(ObjectModel::Sam2).unwrap();
        let mut h = Segmenter { models_root: Some(root.clone()), ..Segmenter::default() };
        h.load_prefs();
        assert_eq!(h.object_model(), ObjectModel::Sam2);
        std::fs::write(root.join(PREFS_FILE), b"{not json").unwrap();
        let mut k = Segmenter { models_root: Some(root.clone()), ..Segmenter::default() };
        k.load_prefs();
        assert_eq!(k.object_model(), ObjectModel::Auto);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn status_lists_every_model_and_unknown_ids_are_errors() {
        let mut g = Segmenter::default();
        let ids: Vec<String> = g.model_status().into_iter().map(|m| m.id).collect();
        assert!(["sam3", "subject", "sam2", "depth"].iter().all(|k| ids.iter().any(|i| i == k)), "{ids:?}");
        let a = crate::activity::Activity::default();
        assert!(g.start_model_download("nope", &a).is_err());
        assert!(!g.cancel_model_download("nope"));
    }
}
