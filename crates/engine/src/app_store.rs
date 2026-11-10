//! The app-wide store: develop presets (and their At Import / hidden marks), imported LUT
//! profiles, local adjustment (mask effect) presets, import develop defaults and the user's way of
//! working ([`Habits`]: export presets, print templates, metadata / filter / curve / label /
//! keyword-set presets, slideshow settings, the last export), shared by every library on this
//! computer, like the other app's presets folder (LR-PREF-SHARED, LR-PREF-HABITS).
//!
//! ```text
//! <settings folder>/Library Store/
//!   shared.json    (everything but the develop presets, plus the preset marks and the merge record)
//!   presets.json   (develop presets, folders, favourites, profile favourites and recents)
//!   Profiles/      (the LUT profiles' .cube files)
//! ```
//!
//! A host turns it on ([`Session::with_default_app_store`]: the desktop app, and the CLI when it
//! opens a library or runs with `--shared-settings`); a bare [`Session`] keeps everything with its
//! library, so tests stay hermetic. A library can still keep its own (`store.useLibrary
//! {keep: true}`, `storeWithLibrary` in its `prefs.json`).
//!
//! **Migration.** Each library is merged into the store once: when it is opened, and on request
//! for any library folder (`store.mergeLibraries`). Its profile files are copied into `Profiles/`,
//! its presets, effects and named habits are added (the same one twice is kept once, the same name
//! with other content as "name 2"), its At Import / hidden marks join the store's (the marks lived
//! in each library's `presets.json` before), and nothing in the library is written. A profile or
//! preset id the store already uses for something else gets a new id; the library's photos follow a
//! profile's new id when it is opened ([`Merged::remap`]).
//!
//! While a library uses the store its own copies stay as they were read: `prefs.json` keeps those
//! keys unchanged (for older builds) and `presets.json` is not written.
//!
//! **Never a crash.** A store file that doesn't parse is kept as `<name>.corrupt-<time>` and the
//! defaults are used, with a warning that names the file; one that can't be read is never
//! overwritten this session. Every write is atomic ([`Store::write_atomic`]).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use lcc_catalog::{FsStore, Store};
use lcc_develop::Preset;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::cmd::lut_profiles::LutProfile;
use crate::cmd::mask_effects::MaskEffect;
use crate::import::ImportDefaults;
use crate::library::{PresetsFile, PresetsSaved};
use crate::{EngineError, Result, Session};

/// The store's folder in the settings folder.
pub const DIR_NAME: &str = "Library Store";
/// Everything but the develop presets.
pub const SHARED: &str = "shared.json";
/// Develop presets (a library's `presets.json` format).
pub const PRESETS: &str = "presets.json";
/// The profile files.
pub const PROFILES_DIR: &str = "Profiles";
/// Largest library `presets.json` read by a merge (an 11 000-preset collection is ~40 MB).
const MAX_PRESETS_FILE: u64 = 512 << 20;
/// Largest library `prefs.json` read.
const MAX_PREFS_FILE: u64 = 64 << 20;
/// Largest profile file copied.
const MAX_PROFILE_FILE: u64 = 64 << 20;
/// Most backup folders read per library by `store.gatherPresets`.
const MAX_BACKUPS: usize = 200;
const SHARED_VERSION: u32 = 1;

/// The user's way of working beyond develop presets and profiles: the same in every library. In a
/// library's `prefs.json` under the same keys, where older builds and libraries that keep their own
/// have them. What belongs to a library's photos and disk (XMP writing, cache size, smart-preview
/// folder, recent keywords, auto import, folder watching…) stays with the library.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Habits {
    pub export_presets: Vec<crate::export::ExportPreset>,
    /// Parameters of the last export (Export with Previous).
    #[serde(alias = "last_export")]
    pub last_export: Option<Value>,
    /// Options of the last Photo Merge, per command.
    pub last_merge: BTreeMap<String, Value>,
    pub print_templates: Vec<crate::print::template::PrintTemplate>,
    pub metadata_presets: Vec<crate::cmd::metadata::MetadataPreset>,
    pub filter_presets: Vec<crate::cmd::filters::FilterPreset>,
    pub curve_presets: Vec<crate::cmd::curves::CurvePreset>,
    pub label_sets: Vec<crate::cmd::manage::LabelSet>,
    pub keyword_sets: Vec<crate::cmd::keywords::KeywordSet>,
    pub keyword_set: Option<String>,
    pub slideshow: crate::slideshow::PlaybackSettings,
    pub slide_layout: crate::slide::SlideSettings,
    pub slide_overlays: crate::slide_overlay::OverlaySettings,
    pub slide_titles: crate::slide_title::TitleSettings,
    pub hold_view: bool,
    /// The grid's sort (while the store is in use; a library keeping its own has it in view.json).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grid_sort: Option<lcc_catalog::Sort>,
    /// What Copy Settings, Sync and New Preset tick, New Preset's group, and the other dialogs'
    /// last answers (LR-PRE-CREATE).
    #[serde(skip_serializing_if = "DialogMemory::is_default", deserialize_with = "lenient_dialogs")]
    pub dialogs: DialogMemory,
}

/// The group a new preset goes in unless another is chosen.
pub const DEFAULT_PRESET_GROUP: &str = "User Presets";
/// Most dialogs remembered in [`DialogMemory::other`] (a hand-edited file can't grow it without end).
pub const MAX_REMEMBERED_DIALOGS: usize = 64;
/// Largest remembered answer of one dialog (bytes of JSON).
pub const MAX_REMEMBERED_BYTES: usize = 64 << 10;

/// What the dialogs with choices remember, across uses, restarts and libraries.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DialogMemory {
    /// Copy Settings (Paste Selected offers what was copied; tethering's Same as Previous).
    pub copy: lcc_develop::SettingsPick,
    /// Sync Settings.
    pub sync: lcc_develop::SettingsPick,
    /// New Preset's ticks (White Balance left out until it is ticked).
    pub new_preset: lcc_develop::SettingsPick,
    /// The group New Preset put the last preset in.
    pub preset_group: String,
    /// The other dialogs' last choices by dialog id (`dialog.remember`).
    pub other: BTreeMap<String, Value>,
}

impl Default for DialogMemory {
    fn default() -> Self {
        use lcc_develop::{SettingsGroup, SettingsPick};
        DialogMemory {
            copy: SettingsPick::of_groups(SettingsGroup::default_copy()),
            sync: SettingsPick::of_groups(SettingsGroup::default_copy()),
            new_preset: SettingsPick::of_groups(SettingsGroup::default_preset()),
            preset_group: DEFAULT_PRESET_GROUP.into(),
            other: BTreeMap::new(),
        }
    }
}

/// Remembered choices that can't be read (a hand-edited file) are the defaults, never a damaged store.
pub(crate) fn lenient_dialogs<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<DialogMemory, D::Error> {
    let v = Value::deserialize(d)?;
    Ok(serde_json::from_value::<DialogMemory>(v)
        .map_err(|e| log::warn!("the remembered dialog choices can't be read ({e}); the defaults are used"))
        .unwrap_or_default()
        .sanitized())
}

impl DialogMemory {
    pub fn is_default(&self) -> bool {
        *self == DialogMemory::default()
    }

    /// Picks normalized, the map and its answers capped.
    pub(crate) fn sanitized(mut self) -> DialogMemory {
        self.copy = self.copy.normalized();
        self.sync = self.sync.normalized();
        self.new_preset = self.new_preset.normalized();
        if self.preset_group.trim().is_empty() {
            self.preset_group = DEFAULT_PRESET_GROUP.into();
        }
        self.other.retain(|k, v| k.len() <= 64 && serde_json::to_vec(v).is_ok_and(|b| b.len() <= MAX_REMEMBERED_BYTES));
        while self.other.len() > MAX_REMEMBERED_DIALOGS {
            let Some(k) = self.other.keys().next_back().cloned() else { break };
            self.other.remove(&k);
        }
        self
    }

    /// Take `other`'s choices where this has only the defaults (a library merged into the store).
    fn merge(&mut self, other: DialogMemory) {
        let d = DialogMemory::default();
        if self.copy == d.copy {
            self.copy = other.copy;
        }
        if self.sync == d.sync {
            self.sync = other.sync;
        }
        if self.new_preset == d.new_preset {
            self.new_preset = other.new_preset;
        }
        if self.preset_group == d.preset_group {
            self.preset_group = other.preset_group;
        }
        for (k, v) in other.other {
            if self.other.len() < MAX_REMEMBERED_DIALOGS {
                self.other.entry(k).or_insert(v);
            }
        }
    }
}

/// The `prefs.json` keys the store holds while a library uses it (kept there as read).
pub(crate) const SHARED_PREFS_KEYS: [&str; 21] = [
    "gridSort",
    "dialogs",
    "lutProfiles",
    "maskEffects",
    "maskToolEffects",
    "import",
    "exportPresets",
    "lastExport",
    "lastMerge",
    "printTemplates",
    "metadataPresets",
    "filterPresets",
    "curvePresets",
    "labelSets",
    "keywordSets",
    "keywordSet",
    "slideshow",
    "slideLayout",
    "slideOverlays",
    "slideTitles",
    "holdView",
];

/// Presets marked At Import, and those hidden from the panel and pickers.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PresetMarks {
    pub at_import: Vec<String>,
    pub hidden: Vec<String>,
    /// Built-in presets renamed (id → name) and moved to another group (id → group).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub names: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub groups: BTreeMap<String, String>,
}

impl PresetMarks {
    /// The session's marks.
    pub(crate) fn of(s: &Session) -> PresetMarks {
        PresetMarks {
            at_import: s.preset_at_import.clone(),
            hidden: s.preset_hidden.clone(),
            names: s.preset_builtin_names.clone(),
            groups: s.preset_builtin_groups.clone(),
        }
    }
}

/// A library merged into the store.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Merged {
    /// The library folder, as it was named.
    pub library: String,
    /// When (the session clock).
    pub at: String,
    /// What it added.
    pub presets: usize,
    pub profiles: usize,
    pub effects: usize,
    /// Profile ids of the library the store gave a new id: its photos move to it when it opens.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub remap: BTreeMap<String, String>,
    /// The remap was applied to the library's photos.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub remapped: bool,
    /// An older install's camera profiles installed (`None`: not taken, merged by a build that
    /// didn't take them: the next import takes them, once).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera_profiles: Option<usize>,
}

/// `shared.json`.
#[derive(Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct SharedFile {
    version: u32,
    lut_profiles: Vec<LutProfile>,
    mask_effects: Vec<MaskEffect>,
    mask_tool_effects: BTreeMap<String, String>,
    /// Develop defaults for imports (auto import stays with each library).
    import: ImportDefaults,
    preset_marks: PresetMarks,
    #[serde(flatten)]
    habits: Habits,
    merged: Vec<Merged>,
    /// Library folders whose named presets `store.gatherPresets` took already.
    gathered: Vec<String>,
    /// Preparing photos in the background ([`crate::prep`]): the app-wide settings.
    prep: crate::prep::PrepSettings,
}

/// The store-held parts of a library's `prefs.json` (read only).
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct LibraryPrefs {
    pub lut_profiles: Vec<LutProfile>,
    pub mask_effects: Vec<MaskEffect>,
    pub mask_tool_effects: BTreeMap<String, String>,
    pub import: ImportDefaults,
    #[serde(flatten)]
    pub habits: Habits,
    pub store_with_library: bool,
}

/// Everything the store holds, as plain data (a library's, the session's or the store's).
#[derive(Default)]
pub(crate) struct Content {
    pub lut_profiles: Vec<LutProfile>,
    pub mask_effects: Vec<MaskEffect>,
    pub mask_tool_effects: BTreeMap<String, String>,
    pub import: ImportDefaults,
    pub habits: Habits,
    pub marks: PresetMarks,
    /// User presets.
    pub presets: Vec<Preset>,
    pub folders: Vec<String>,
    /// Built-in presets marked favourite.
    pub favorites: Vec<String>,
    pub profile_favorites: Vec<String>,
    pub profile_recent: Vec<String>,
}

impl Content {
    fn from_files(prefs: LibraryPrefs, presets: Option<PresetsFile>) -> Content {
        let p = presets.unwrap_or_default();
        Content {
            lut_profiles: prefs.lut_profiles,
            mask_effects: prefs.mask_effects,
            mask_tool_effects: prefs.mask_tool_effects,
            import: prefs.import,
            habits: prefs.habits,
            marks: PresetMarks { at_import: p.at_import, hidden: p.hidden, names: p.builtin_names, groups: p.builtin_groups },
            presets: p.user.into_iter().filter(|q| !q.builtin).collect(),
            folders: p.folders,
            favorites: p.favorites,
            profile_favorites: p.profile_favorites,
            profile_recent: p.profile_recent,
        }
    }
}

/// What one merge added.
#[derive(Default, Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeReport {
    pub library: String,
    pub presets: usize,
    pub profiles: usize,
    pub effects: usize,
    /// Camera profiles (`.dcp`) installed (an older install's).
    #[serde(skip_serializing_if = "is_zero")]
    pub camera_profiles: usize,
    /// The file names of camera profiles that were there already.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub camera_already: Vec<String>,
    /// Camera bodies that start with one of those camera profiles now.
    #[serde(skip_serializing_if = "is_zero")]
    pub camera_bodies: usize,
    /// Problems (a profile file that couldn't be read…); nothing is lost: the library keeps its copy.
    pub problems: Vec<String>,
    /// Merged before: nothing done.
    pub skipped: bool,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// The store on disk.
pub struct AppStore {
    /// Its folder.
    pub dir: PathBuf,
    files: Box<dyn Store>,
    /// `shared.json` as last written or read (written again only when it changes).
    shared_written: Vec<u8>,
    /// What `presets.json` was last written from.
    presets_saved: PresetsSaved,
    pub merged: Vec<Merged>,
    gathered: Vec<String>,
    /// Files that couldn't be read: never overwritten this session.
    blocked: Vec<&'static str>,
    /// Problems to show once.
    warnings: Vec<String>,
    warnings_reported: usize,
}

impl AppStore {
    /// The store in `dir` (created when missing).
    pub fn open(dir: &Path) -> std::io::Result<AppStore> {
        Ok(AppStore {
            dir: dir.to_path_buf(),
            files: Box::new(FsStore::open(dir)?),
            shared_written: Vec::new(),
            presets_saved: PresetsSaved::default(),
            merged: Vec::new(),
            gathered: Vec::new(),
            blocked: Vec::new(),
            warnings: Vec::new(),
            warnings_reported: 0,
        })
    }

    pub fn profiles_dir(&self) -> PathBuf {
        self.dir.join(PROFILES_DIR)
    }

    /// `library` (a folder) was merged already.
    pub fn has_merged(&self, library: &Path) -> bool {
        self.merged.iter().any(|m| same_path(Path::new(&m.library), library))
    }

    /// Older install `store` was merged already, its camera profiles too.
    fn has_merged_cameras(&self, store: &Path) -> bool {
        self.merged.iter().any(|m| m.camera_profiles.is_some() && same_path(Path::new(&m.library), store))
    }

    /// A store file, parsed; `None` when missing or damaged. A damaged file is kept as
    /// `<name>.corrupt-<time>`; one that can't be read is never written this session.
    fn read<T: DeserializeOwned>(&mut self, name: &'static str) -> Option<T> {
        let bytes = match self.files.read(name) {
            Ok(b) => b?,
            Err(e) => {
                log::error!("store: {name}: {e}");
                self.blocked.push(name);
                self.warnings.push(format!(
                    "The shared {name} in {} couldn't be read ({e}); it isn't overwritten this session. Check the file's permissions and restart.",
                    self.dir.display()
                ));
                return None;
            }
        };
        match serde_json::from_slice(&bytes) {
            Ok(v) => Some(v),
            Err(err) => {
                let keep = format!("{name}.corrupt-{}", unix_secs());
                match self.files.write_atomic(&keep, &bytes) {
                    Ok(()) => self.warnings.push(format!(
                        "The shared {name} in {} is damaged ({err}). It was kept as {keep} and the defaults are used.",
                        self.dir.display()
                    )),
                    Err(w) => {
                        self.blocked.push(name);
                        self.warnings.push(format!(
                            "The shared {name} in {} is damaged ({err}) and couldn't be set aside ({w}); it isn't overwritten. Move it away and restart.",
                            self.dir.display()
                        ));
                    }
                }
                None
            }
        }
    }

    fn write(&mut self, name: &'static str, bytes: &[u8]) -> Result<()> {
        if self.blocked.contains(&name) {
            return Err(EngineError::Other(format!(
                "the shared {name} in {} couldn't be read when the app started, so it isn't overwritten",
                self.dir.display()
            )));
        }
        self.files.write_atomic(name, bytes).map_err(|e| EngineError::Other(format!("shared {name} in {}: {e}", self.dir.display())))
    }

    /// Warnings not handed out yet.
    pub fn take_warnings(&mut self) -> Vec<String> {
        let new = self.warnings.get(self.warnings_reported..).map(<[String]>::to_vec).unwrap_or_default();
        self.warnings_reported = self.warnings.len();
        new
    }
}

/// The folders that may hold backups of the library in `dir` (each backup a folder of its own):
/// `Backups/` and `backups/` in it, and the backup folder its catalog settings name, which may be
/// elsewhere (another drive) and may keep each library's backups under the library's name.
fn backup_roots(dir: &Path) -> Vec<PathBuf> {
    #[allow(unused_mut)]
    let mut roots = vec![dir.join("Backups"), dir.join("backups")];
    // (the catalog's backup settings exist natively only; the web build has no folders to read)
    #[cfg(not(target_arch = "wasm32"))]
    {
        let custom = lcc_catalog::library::CatalogSettings::load(dir).backup_root(dir);
        if let Some(name) = dir.file_name() {
            roots.push(custom.join(name));
        }
        roots.push(custom);
    }
    let mut out: Vec<PathBuf> = Vec::new();
    for r in roots {
        if !out.iter().any(|o| same_path(o, &r)) {
            out.push(r);
        }
    }
    out
}

fn unix_secs() -> u64 {
    web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// `a` and `b` name the same folder.
pub fn same_path(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => {
            let norm = |p: &Path| {
                let s = p.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_string();
                if cfg!(windows) { s.to_lowercase() } else { s }
            };
            norm(a) == norm(b)
        }
    }
}

/// A file of at most `max` bytes (`None` when missing).
fn read_capped(path: &Path, max: u64) -> std::result::Result<Option<Vec<u8>>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if !m.is_file() => Err(format!("{}: not a regular file", path.display())),
        Ok(m) if m.len() > max => Err(format!("{}: {} bytes is too large", path.display(), m.len())),
        Ok(_) => std::fs::read(path).map(Some).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn parse_capped<T: DeserializeOwned + Default>(path: &Path, max: u64) -> std::result::Result<Option<T>, String> {
    match read_capped(path, max)? {
        Some(b) => serde_json::from_slice(&b).map(Some).map_err(|e| format!("{}: {e}", path.display())),
        None => Ok(None),
    }
}

/// A library folder's store-held content (read only).
pub(crate) fn read_library(dir: &Path) -> std::result::Result<Content, String> {
    let prefs: LibraryPrefs = parse_capped(&dir.join("prefs.json"), MAX_PREFS_FILE)?.unwrap_or_default();
    let presets: Option<PresetsFile> = parse_capped(&dir.join("presets.json"), MAX_PRESETS_FILE)?;
    Ok(Content::from_files(prefs, presets))
}

/// Whether `dir` holds an app store: an older install's `Library Store`, with its develop presets or
/// its shared file.
pub fn is_store_folder(dir: &Path) -> bool {
    dir.join(PRESETS).is_file() || dir.join(SHARED).is_file()
}

/// The stores of this computer's older installs, under the app's earlier names: each earlier settings
/// folder's `Library Store` that holds one (`[legacy] settings_dirs` in `brand.toml`).
pub fn older_install_stores() -> Vec<PathBuf> {
    stores_in(lcc_brand::legacy_settings_dirs())
}

/// The `Library Store` folders of `settings_dirs` that hold a store.
pub(crate) fn stores_in(settings_dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    settings_dirs.into_iter().map(|d| d.join(DIR_NAME)).filter(|d| is_store_folder(d)).collect()
}

/// `v` as a `T`, the parts that read: a key `T` can't take is left out and named in `problems`, so one
/// list another build wrote differently costs only itself.
fn lenient<T: DeserializeOwned + Default>(v: &Value, what: &str, problems: &mut Vec<String>) -> T {
    if let Ok(t) = serde_json::from_value::<T>(v.clone()) {
        return t;
    }
    let Some(o) = v.as_object() else {
        if !v.is_null() {
            problems.push(format!("{what}: not an object; left out"));
        }
        return T::default();
    };
    let mut kept = serde_json::Map::new();
    for (k, x) in o {
        let one: serde_json::Map<String, Value> = std::iter::once((k.clone(), x.clone())).collect();
        if serde_json::from_value::<T>(Value::Object(one)).is_ok() {
            kept.insert(k.clone(), x.clone());
        } else {
            problems.push(format!("{what}: {k} couldn't be read; left out"));
        }
    }
    serde_json::from_value(Value::Object(kept)).unwrap_or_default()
}

/// A JSON file of at most `max` bytes (`Null` when missing); one that can't be read or parsed is named
/// in `problems`.
fn json_file(path: &Path, max: u64, problems: &mut Vec<String>) -> Value {
    match read_capped(path, max) {
        Ok(Some(b)) => serde_json::from_slice(&b).unwrap_or_else(|e| {
            problems.push(format!("{}: {e}", path.display()));
            Value::Null
        }),
        Ok(None) => Value::Null,
        Err(e) => {
            problems.push(e);
            Value::Null
        }
    }
}

/// The Presets Manager's choices as an older install's store keeps them (`presetState` in its
/// `shared.json`).
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct OlderPresetState {
    hidden: Vec<String>,
    names: BTreeMap<String, String>,
    groups: BTreeMap<String, String>,
    for_import: Vec<String>,
}

/// An older install's store folder `dir`, read only: its develop presets with their favourites and
/// At Import / hidden marks, local adjustment presets, LUT profiles, import defaults and habits. What
/// can't be read is left out and named in `problems` (a preset that doesn't read costs only itself).
pub(crate) fn read_store_folder(dir: &Path, problems: &mut Vec<String>) -> Content {
    let shared = json_file(&dir.join(SHARED), MAX_PREFS_FILE, problems);
    let prefs: LibraryPrefs = lenient(&shared, SHARED, problems);
    let mut presets = match json_file(&dir.join(PRESETS), MAX_PRESETS_FILE, problems) {
        Value::Object(mut o) => {
            let user = o.remove("user");
            let mut file: PresetsFile = lenient(&Value::Object(o), PRESETS, problems);
            let mut unread = 0usize;
            let items = match user {
                Some(Value::Array(a)) => a,
                _ => Vec::new(),
            };
            for item in items {
                match serde_json::from_value::<Preset>(item) {
                    Ok(p) => file.user.push(p),
                    Err(_) => unread += 1,
                }
            }
            if unread > 0 {
                problems.push(format!("{PRESETS}: {unread} preset(s) couldn't be read; left out"));
            }
            file
        }
        Value::Null => PresetsFile::default(),
        _ => {
            problems.push(format!("{PRESETS}: not an object; left out"));
            PresetsFile::default()
        }
    };
    // the marks: in `presetMarks` (this app's store), or in `presetState` (an older install's Presets Manager)
    let marks: PresetMarks = shared.get("presetMarks").map(|v| lenient(v, "presetMarks", problems)).unwrap_or_default();
    let state: OlderPresetState = shared.get("presetState").map(|v| lenient(v, "presetState", problems)).unwrap_or_default();
    for id in marks.at_import.into_iter().chain(state.for_import) {
        if !presets.at_import.contains(&id) {
            presets.at_import.push(id);
        }
    }
    for id in marks.hidden.into_iter().chain(state.hidden) {
        if !presets.hidden.contains(&id) {
            presets.hidden.push(id);
        }
    }
    for (id, n) in marks.names.into_iter().chain(state.names) {
        presets.builtin_names.entry(id).or_insert(n);
    }
    for (id, g) in marks.groups.into_iter().chain(state.groups) {
        presets.builtin_groups.entry(id).or_insert(g);
    }
    Content::from_files(prefs, Some(presets))
}

/// A free id: `base`, else `base-2`, `base-3`…
fn free_id(base: &str, taken: impl Fn(&str) -> bool) -> String {
    let mut id = base.to_string();
    let mut n = 2u32;
    while taken(&id) && n < 100_000 {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

/// A name not taken: `base`, else "base 2", "base 3"…
fn free_name(base: &str, taken: impl Fn(&str) -> bool) -> String {
    let mut name = base.to_string();
    let mut n = 2u32;
    while taken(&name) && n < 10_000 {
        name = format!("{base} {n}");
        n += 1;
    }
    name
}

/// Add `theirs` to `mine` by their `name`: the same item twice is kept once; the same name with other
/// content is kept as "name 2" (nothing lost). Items are compared as JSON, so no type needs
/// `PartialEq`. → how many were added.
fn merge_named<T: Serialize + DeserializeOwned>(mine: &mut Vec<T>, theirs: Vec<T>) -> usize {
    let mut have: Vec<Value> = mine.iter().filter_map(|t| serde_json::to_value(t).ok()).collect();
    let name_of = |v: &Value| v.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
    let base_of = |n: &str| -> String {
        // "name 2" was kept for a clash before: the same preset again is still a duplicate
        match n.rsplit_once(' ') {
            Some((b, d)) if !b.is_empty() && d.chars().all(|c| c.is_ascii_digit()) && !d.is_empty() => b.to_string(),
            _ => n.to_string(),
        }
    };
    let mut added = 0;
    for t in theirs {
        let Ok(mut v) = serde_json::to_value(&t) else { continue };
        let without_name = |v: &Value| {
            let mut v = v.clone();
            if let Some(o) = v.as_object_mut() {
                o.remove("name");
            }
            v
        };
        let base = base_of(&name_of(&v));
        if have.iter().any(|h| *h == v || (base_of(&name_of(h)) == base && without_name(h) == without_name(&v))) {
            continue;
        }
        let name = name_of(&v);
        if have.iter().any(|h| name_of(h).eq_ignore_ascii_case(&name)) {
            let fresh = free_name(&name, |c| have.iter().any(|h| name_of(h).eq_ignore_ascii_case(c)));
            if let Some(o) = v.as_object_mut() {
                o.insert("name".into(), Value::String(fresh));
            }
        }
        let Ok(t) = serde_json::from_value::<T>(v.clone()) else { continue };
        mine.push(t);
        have.push(v);
        added += 1;
    }
    added
}

impl Habits {
    /// Add `other`'s named presets ([`merge_named`]) → how many were added.
    fn merge_named_presets(&mut self, other: Habits) -> usize {
        let mut n = merge_named(&mut self.export_presets, other.export_presets);
        n += merge_named(&mut self.print_templates, other.print_templates);
        n += merge_named(&mut self.metadata_presets, other.metadata_presets);
        n += merge_named(&mut self.filter_presets, other.filter_presets);
        n += merge_named(&mut self.curve_presets, other.curve_presets);
        n += merge_named(&mut self.label_sets, other.label_sets);
        n += merge_named(&mut self.keyword_sets, other.keyword_sets);
        n
    }

    /// Add what a library's `other` has: named presets by name; a single setting (the last export,
    /// the slideshow's…) only where this has none of its own → presets added.
    fn merge(&mut self, other: Habits) -> usize {
        let d = Habits::default();
        let same = |a: &dyn erased::Json, b: &dyn erased::Json| a.json() == b.json();
        if self.last_export.is_none() {
            self.last_export = other.last_export.clone();
        }
        for (k, v) in &other.last_merge {
            self.last_merge.entry(k.clone()).or_insert_with(|| v.clone());
        }
        if same(&self.slideshow, &d.slideshow) {
            self.slideshow = other.slideshow.clone();
        }
        if same(&self.slide_layout, &d.slide_layout) {
            self.slide_layout = other.slide_layout.clone();
        }
        if same(&self.slide_overlays, &d.slide_overlays) {
            self.slide_overlays = other.slide_overlays.clone();
        }
        if same(&self.slide_titles, &d.slide_titles) {
            self.slide_titles = other.slide_titles.clone();
        }
        self.hold_view |= other.hold_view;
        if self.grid_sort.is_none() {
            self.grid_sort = other.grid_sort;
        }
        self.dialogs.merge(other.dialogs.clone());
        let keyword_set = other.keyword_set.clone();
        let n = self.merge_named_presets(other);
        if self.keyword_set.is_none() {
            self.keyword_set = keyword_set.filter(|k| self.keyword_sets.iter().any(|s| &s.name == k));
        }
        n
    }
}

/// Compare settings of different types as JSON.
mod erased {
    pub trait Json {
        fn json(&self) -> serde_json::Value;
    }
    impl<T: serde::Serialize> Json for T {
        fn json(&self) -> serde_json::Value {
            serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
        }
    }
}

/// Merge library content `theirs` into `mine`. `their_profiles` is the library's `Profiles/` folder
/// (its LUT files), `profiles_dir` the store's (copies go there; `None`: the lists only). → report
/// and the library's profile id remap.
pub(crate) fn merge_content(
    mine: &mut Content,
    theirs: Content,
    their_profiles: Option<&Path>,
    profiles_dir: Option<&Path>,
) -> (MergeReport, BTreeMap<String, String>) {
    let mut report = MergeReport::default();
    let mut remap: BTreeMap<String, String> = BTreeMap::new();
    if let Some(d) = profiles_dir
        && let Err(e) = std::fs::create_dir_all(d)
    {
        report.problems.push(format!("{}: {e}", d.display()));
    }
    // LUT profiles: the same file once; an id taken by another file gets a new id
    for mut l in theirs.lut_profiles {
        let src = their_profiles.and_then(|d| Path::new(&l.file).file_name().map(|n| d.join(n)));
        let bytes = match src.as_deref().map(|f| read_capped(f, MAX_PROFILE_FILE)) {
            Some(Ok(Some(b))) => Some(b),
            Some(Ok(None)) | None => {
                report.problems.push(format!("{}: the profile file is missing; kept as listed", l.file));
                None
            }
            Some(Err(e)) => {
                report.problems.push(e);
                None
            }
        };
        let same_file = |p: &LutProfile| {
            let mine_file = profiles_dir.and_then(|d| Path::new(&p.file).file_name().map(|n| d.join(n)));
            match (&bytes, mine_file) {
                (Some(b), Some(f)) => std::fs::read(f).is_ok_and(|have| have == *b),
                _ => p.file == l.file,
            }
        };
        if let Some(p) = mine.lut_profiles.iter().find(|p| p.name == l.name && p.group == l.group && same_file(p)) {
            if p.id != l.id {
                remap.insert(l.id.clone(), p.id.clone());
            }
            continue;
        }
        let id = free_id(&l.id, |id| mine.lut_profiles.iter().any(|p| p.id == id));
        if id != l.id {
            remap.insert(l.id.clone(), id.clone());
        }
        if let (Some(b), Some(d)) = (&bytes, profiles_dir) {
            let stem = crate::presets::slug(id.trim_start_matches("lut:"));
            let stem = if stem.is_empty() { "profile".to_string() } else { stem };
            let name = free_name(&stem, |c| d.join(format!("{c}.cube")).exists());
            let to = d.join(format!("{name}.cube").replace(' ', "-"));
            match lcc_catalog::safe_file::write_atomic(&to, b) {
                Ok(()) => l.file = to.to_string_lossy().to_string(),
                Err(e) => report.problems.push(format!("{}: {e}; the library's copy is used", to.display())),
            }
        }
        l.id = id;
        mine.lut_profiles.push(l);
        report.profiles += 1;
    }
    // local adjustment presets: by name and folder; other content with the same name as "name 2"
    for mut fx in theirs.mask_effects {
        let same = |a: &MaskEffect, b: &MaskEffect| erased::Json::json(a) == erased::Json::json(b);
        if mine.mask_effects.iter().any(|m| same(m, &fx)) {
            continue;
        }
        if mine.mask_effects.iter().any(|m| m.name == fx.name && m.group == fx.group) {
            let group = fx.group.clone();
            fx.name = free_name(&fx.name, |c| mine.mask_effects.iter().any(|m| m.name == c && m.group == group));
        }
        mine.mask_effects.push(fx);
        report.effects += 1;
    }
    for (k, v) in theirs.mask_tool_effects {
        mine.mask_tool_effects.entry(k).or_insert(v);
    }
    // import develop defaults: the library's when the store has none yet (auto import stays)
    let plain = |d: &ImportDefaults| ImportDefaults { auto_folder: None, auto_copy: false, auto_album: None, ..d.clone() };
    if plain(&mine.import) == ImportDefaults::default() {
        mine.import = plain(&theirs.import);
    }
    report.presets += mine.habits.merge(theirs.habits);
    // develop presets: the same (name, group, settings) once; a taken id gets a new one
    let mut preset_remap: BTreeMap<String, String> = BTreeMap::new();
    let builtin: HashSet<String> = crate::presets::builtin().into_iter().map(|p| p.id).collect();
    for mut p in theirs.presets {
        if let Some(q) = mine.presets.iter().find(|q| q.name == p.name && q.group == p.group && q.settings == p.settings) {
            if q.id != p.id {
                preset_remap.insert(p.id.clone(), q.id.clone());
            }
            continue;
        }
        let taken = |id: &str| builtin.contains(id) || mine.presets.iter().any(|q| q.id == id);
        if p.id.trim().is_empty() || taken(&p.id) {
            let base = format!("user.{}", crate::presets::slug(&format!("{}-{}", p.group, p.name)));
            let id = free_id(&base, taken);
            preset_remap.insert(p.id.clone(), id.clone());
            p.id = id;
        }
        mine.presets.push(p);
        report.presets += 1;
    }
    for f in theirs.folders {
        if !mine.folders.contains(&f) {
            mine.folders.push(f);
        }
    }
    let union = |mine: &mut Vec<String>, theirs: Vec<String>, map: &BTreeMap<String, String>| {
        for id in theirs.into_iter().map(|id| map.get(&id).cloned().unwrap_or(id)) {
            if !mine.contains(&id) {
                mine.push(id);
            }
        }
    };
    union(&mut mine.favorites, theirs.favorites, &preset_remap);
    union(&mut mine.marks.at_import, theirs.marks.at_import, &preset_remap);
    union(&mut mine.marks.hidden, theirs.marks.hidden, &preset_remap);
    for (id, n) in theirs.marks.names {
        mine.marks.names.entry(id).or_insert(n);
    }
    for (id, g) in theirs.marks.groups {
        mine.marks.groups.entry(id).or_insert(g);
    }
    union(&mut mine.profile_favorites, theirs.profile_favorites, &remap);
    let mut recent = std::mem::take(&mut mine.profile_recent);
    union(&mut recent, theirs.profile_recent, &remap);
    recent.truncate(crate::presets::RECENT_PROFILES);
    mine.profile_recent = recent;
    (report, remap)
}

impl Session {
    /// Keep presets, profiles and habits in the settings folder (`<settings>/Library Store`),
    /// shared by every library. Not with `<PREFIX>_NO_PREFS` (tests, scripts) or without a
    /// settings folder.
    pub fn with_default_app_store(self) -> Self {
        if lcc_brand::env_is_set("NO_PREFS") {
            return self;
        }
        match crate::config::config_dir() {
            Some(d) => self.with_app_store(&d.join(DIR_NAME)),
            None => self,
        }
    }

    /// [`Session::with_default_app_store`] on a session that exists already.
    pub fn use_default_app_store(&mut self) {
        let s = std::mem::take(self);
        *self = s.with_default_app_store();
    }

    /// Keep the shared settings in `dir` ([`AppStore`]). A folder that can't be made leaves every
    /// library keeping its own (logged and reported).
    pub fn with_app_store(mut self, dir: &Path) -> Self {
        match AppStore::open(dir) {
            Ok(st) => self.app_store = Some(st),
            Err(e) => log::error!("store: {}: {e}; libraries keep their own presets and profiles", dir.display()),
        }
        if self.uses_app_store() {
            let c = self.read_store();
            self.set_content(c);
            self.mark_store_saved();
        }
        self
    }

    /// The shared settings live in the app store now (not in the open library).
    pub fn uses_app_store(&self) -> bool {
        self.app_store.is_some() && !self.library.as_ref().is_some_and(|l| l.own_store)
    }

    /// Where imported profile files go: the store's `Profiles/`, else the open library's.
    pub fn profiles_dir(&self) -> Option<PathBuf> {
        if self.uses_app_store() {
            return self.app_store.as_ref().map(AppStore::profiles_dir);
        }
        self.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.join("Profiles"))
    }

    /// The store's content as on disk (defaults for a missing or damaged file).
    pub(crate) fn read_store(&mut self) -> Content {
        let Some(st) = self.app_store.as_mut() else { return Content::default() };
        let shared: SharedFile = st.read(SHARED).unwrap_or_default();
        let presets: PresetsFile = st.read(PRESETS).unwrap_or_default();
        st.merged = shared.merged;
        st.gathered = shared.gathered;
        // (the app-wide background preparation settings: the session's own, not a library's content)
        self.prep.app = shared.prep.sanitized();
        let mut marks = shared.preset_marks;
        // (read-old: marks a presets.json still carries join the store's)
        for id in presets.at_import.iter() {
            if !marks.at_import.contains(id) {
                marks.at_import.push(id.clone());
            }
        }
        for id in presets.hidden.iter() {
            if !marks.hidden.contains(id) {
                marks.hidden.push(id.clone());
            }
        }
        Content {
            lut_profiles: shared.lut_profiles,
            mask_effects: shared.mask_effects,
            mask_tool_effects: shared.mask_tool_effects,
            import: shared.import,
            habits: shared.habits,
            marks,
            presets: presets.user.into_iter().filter(|p| !p.builtin).collect(),
            folders: presets.folders,
            favorites: presets.favorites,
            profile_favorites: presets.profile_favorites,
            profile_recent: presets.profile_recent,
        }
    }

    /// What the session holds of the store's content.
    pub(crate) fn content(&self) -> Content {
        #[cfg(test)]
        CONTENT_COPIES.with(|c| c.set(c.get() + 1));
        Content {
            lut_profiles: self.lut_profiles.clone(),
            mask_effects: self.mask_effects.clone(),
            mask_tool_effects: self.mask_tool_effects.clone(),
            import: self.store_import_defaults(),
            habits: self.habits(),
            marks: PresetMarks::of(self),
            presets: self.presets.iter().filter(|p| !p.builtin).cloned().collect(),
            folders: self.preset_folders.clone(),
            favorites: self.presets.iter().filter(|p| p.builtin && p.favorite).map(|p| p.id.clone()).collect(),
            profile_favorites: self.profile_favorites.clone(),
            profile_recent: self.profile_recent.clone(),
        }
    }

    /// The import defaults the store keeps (auto import stays with each library).
    fn store_import_defaults(&self) -> ImportDefaults {
        ImportDefaults { auto_folder: None, auto_copy: false, auto_album: None, ..self.import_defaults.clone() }
    }

    pub(crate) fn habits(&self) -> Habits {
        Habits {
            export_presets: self.export_presets.clone(),
            last_export: self.last_export.clone(),
            last_merge: self.last_merge.clone(),
            print_templates: self.print_templates.clone(),
            metadata_presets: self.metadata_presets.clone(),
            filter_presets: self.filter_presets.clone(),
            curve_presets: self.curve_presets.clone(),
            label_sets: self.label_sets.clone(),
            keyword_sets: self.keyword_sets.clone(),
            keyword_set: self.keyword_set.clone(),
            slideshow: self.slideshow.clone(),
            slide_layout: self.slide_layout.clone(),
            slide_overlays: self.slide_overlays.clone(),
            slide_titles: self.slide_titles.clone(),
            hold_view: self.hold_view,
            grid_sort: self.uses_app_store().then_some(self.sort),
            dialogs: self.dialogs.clone(),
        }
    }

    /// Put `c` in the session (checked like a library's own settings), its LUTs registered.
    pub(crate) fn set_content(&mut self, c: Content) {
        let h = c.habits;
        self.export_presets = h.export_presets;
        self.last_export = h.last_export;
        self.last_merge = h.last_merge;
        self.print_templates = h.print_templates;
        self.metadata_presets = h.metadata_presets;
        self.filter_presets = h.filter_presets;
        self.curve_presets = h.curve_presets;
        self.label_sets = h.label_sets;
        self.keyword_sets = h.keyword_sets;
        self.keyword_set = h.keyword_set;
        self.slideshow = h.slideshow.sanitized();
        self.slide_layout = h.slide_layout.sanitized();
        self.slide_overlays = h.slide_overlays.sanitized();
        self.slide_titles = h.slide_titles.sanitized();
        self.hold_view = h.hold_view;
        if let Some(sort) = h.grid_sort {
            self.sort = sort;
        }
        self.dialogs = h.dialogs.sanitized();
        self.lut_profiles = c.lut_profiles;
        let dir = self.profiles_dir();
        crate::cmd::lut_profiles::register_all(self, dir.as_deref());
        self.mask_effects = c.mask_effects;
        self.mask_tool_effects = c.mask_tool_effects;
        let (auto_folder, auto_copy, auto_album) =
            (self.import_defaults.auto_folder.take(), self.import_defaults.auto_copy, self.import_defaults.auto_album.take());
        self.import_defaults = ImportDefaults { auto_folder, auto_copy, auto_album, ..c.import };
        let mut list = crate::presets::builtin();
        for p in &mut list {
            p.favorite = c.favorites.contains(&p.id);
        }
        let mut ids: HashSet<String> = list.iter().map(|p| p.id.clone()).collect();
        for u in c.presets {
            if ids.insert(u.id.clone()) {
                list.push(u);
            }
        }
        self.presets = crate::presets::PresetList::new(list);
        self.preset_folders = c.folders;
        self.preset_at_import = c.marks.at_import;
        self.preset_hidden = c.marks.hidden;
        self.preset_builtin_names = c.marks.names;
        self.preset_builtin_groups = c.marks.groups;
        self.apply_builtin_edits();
        let known = |s: &Session, id: &String| crate::presets::profile(id).is_some() || s.lut_profiles.iter().any(|p| &p.id == id);
        let favs: Vec<String> = c.profile_favorites.into_iter().filter(|id| known(self, id)).collect();
        let recent: Vec<String> = c.profile_recent.into_iter().filter(|id| known(self, id)).take(crate::presets::RECENT_PROFILES).collect();
        self.profile_favorites = favs;
        self.profile_recent = recent;
        self.refresh_view();
    }

    fn shared_json(&self) -> Vec<u8> {
        let (merged, gathered) = self.app_store.as_ref().map(|s| (s.merged.clone(), s.gathered.clone())).unwrap_or_default();
        // only what shared.json holds: never `content()`, which copies every preset (it runs after
        // every command; 250 ms each with a ten-thousand-preset collection)
        let f = SharedFile {
            version: SHARED_VERSION,
            lut_profiles: self.lut_profiles.clone(),
            mask_effects: self.mask_effects.clone(),
            mask_tool_effects: self.mask_tool_effects.clone(),
            import: self.store_import_defaults(),
            preset_marks: PresetMarks::of(self),
            habits: self.habits(),
            merged,
            gathered,
            prep: self.prep.app,
        };
        serde_json::to_vec_pretty(&f).unwrap_or_default()
    }

    /// What the session holds now counts as on disk (just read).
    fn mark_store_saved(&mut self) {
        let shared = self.shared_json();
        let state = PresetsSaved::state_of(self);
        if let Some(st) = self.app_store.as_mut() {
            st.shared_written = shared;
            st.presets_saved = state;
        }
    }

    /// Write what changed of the store (no-op without one, or while the library keeps its own).
    /// Both files are replaced atomically.
    pub(crate) fn save_app_store(&mut self) -> Result<()> {
        if !self.uses_app_store() {
            return Ok(());
        }
        let shared = self.shared_json();
        let presets_due = self.app_store.as_ref().is_some_and(|st| st.presets_saved.differs(self));
        let presets = presets_due.then(|| {
            // the marks live in shared.json
            let mut json = crate::library::presets_json(self);
            if let Ok(Value::Object(mut o)) = serde_json::from_str::<Value>(&json) {
                o.remove("atImport");
                o.remove("hidden");
                o.remove("builtinNames");
                o.remove("builtinGroups");
                json = serde_json::to_string_pretty(&o).unwrap_or(json);
            }
            (json, PresetsSaved::state_of(self))
        });
        let Some(st) = self.app_store.as_mut() else { return Ok(()) };
        if shared != st.shared_written {
            st.write(SHARED, &shared)?;
            st.shared_written = shared;
        }
        if let Some((json, state)) = presets {
            st.write(PRESETS, json.as_bytes())?;
            st.presets_saved = state;
        }
        Ok(())
    }

    /// The library just opened (its own settings read into the session) takes the store's: merged
    /// into it first when it never was. `dir`'s `Profiles/` holds its LUT files.
    pub(crate) fn adopt_app_store(&mut self, dir: &Path, on_disk: bool) {
        let theirs = self.content();
        let mut mine = self.read_store();
        let first = on_disk && !self.app_store.as_ref().is_some_and(|s| s.has_merged(dir));
        if first {
            let store_profiles = self.app_store.as_ref().map(AppStore::profiles_dir);
            let (mut r, remap) = merge_content(&mut mine, theirs, Some(&dir.join("Profiles")), store_profiles.as_deref());
            r.library = dir.display().to_string();
            self.record_merge(dir, &r, remap);
            log::info!("store: {} merged: {} presets, {} profiles, {} effects", dir.display(), r.presets, r.profiles, r.effects);
        }
        self.set_content(mine);
        // what the library holds of the store was read into the session first: keep it as on disk
        // only after the store is saved below
        if let Some(st) = self.app_store.as_mut() {
            st.presets_saved = PresetsSaved::default();
            st.shared_written = Vec::new();
        }
        if first {
            crate::profile_refs::refresh(self);
        }
        self.apply_store_remap();
        if let Err(e) = self.save_app_store() {
            log::error!("store: {e}");
            if let Some(st) = self.app_store.as_mut() {
                st.warnings.push(e.to_string());
            }
        }
    }

    fn record_merge(&mut self, dir: &Path, r: &MergeReport, remap: BTreeMap<String, String>) {
        let at = (self.clock)();
        let Some(st) = self.app_store.as_mut() else { return };
        st.merged.retain(|m| !same_path(Path::new(&m.library), dir));
        st.merged.push(Merged {
            library: dir.display().to_string(),
            at,
            presets: r.presets,
            profiles: r.profiles,
            effects: r.effects,
            remap,
            remapped: false,
            camera_profiles: None,
        });
        for p in &r.problems {
            log::warn!("store: merging {}: {p}", dir.display());
        }
    }

    /// Merge library folders `dirs` into the store, those not merged yet (`force`: again). Reads
    /// them only; saves the store.
    pub fn merge_libraries(&mut self, dirs: &[PathBuf], force: bool) -> Result<Vec<MergeReport>> {
        self.check_store()?;
        let mut mine = self.content();
        let store_profiles = self.app_store.as_ref().map(AppStore::profiles_dir);
        let mut out = Vec::new();
        for dir in dirs {
            let mut r = MergeReport { library: dir.display().to_string(), ..Default::default() };
            if !force && self.app_store.as_ref().is_some_and(|s| s.has_merged(dir)) {
                r.skipped = true;
            } else if !crate::library::is_library(dir) {
                r.problems.push(format!("{} is not a catalog folder", dir.display()));
            } else {
                match read_library(dir) {
                    Ok(theirs) => {
                        let (mut rr, remap) = merge_content(&mut mine, theirs, Some(&dir.join("Profiles")), store_profiles.as_deref());
                        rr.library = r.library.clone();
                        self.record_merge(dir, &rr, remap);
                        r = rr;
                    }
                    Err(e) => r.problems.push(e),
                }
            }
            out.push(r);
        }
        self.set_content(mine);
        crate::profile_refs::refresh(self);
        self.apply_store_remap();
        self.save_app_store()?;
        Ok(out)
    }

    /// `store.importOlderInstalls`: the presets of older installs' stores (`dirs`; none given: every one
    /// on this computer, [`older_install_stores`]) join the shared store, each folder once unless
    /// `force`: develop presets with their favourites and At Import / hidden marks, local adjustment
    /// presets, LUT profiles, import defaults and habits, and the camera profiles (`.dcp`) of their
    /// `Profiles/` with the bodies they were bound to ([`crate::cmd::dcp_profiles::import_older`]).
    /// The same preset or camera profile twice is kept once. Reads the folders only; saves the store.
    pub fn merge_older_installs(&mut self, dirs: &[PathBuf], force: bool) -> Result<Vec<MergeReport>> {
        self.merge_older_installs_in(dirs, force, None)
    }

    /// [`Self::merge_older_installs`] with the camera profiles going to `camera` (the camera-profiles
    /// folder and its library; `None`: this computer's).
    pub(crate) fn merge_older_installs_in(&mut self, dirs: &[PathBuf], force: bool, camera: Option<(&Path, &Path)>) -> Result<Vec<MergeReport>> {
        self.check_store()?;
        let dirs = if dirs.is_empty() { older_install_stores() } else { dirs.to_vec() };
        if dirs.is_empty() {
            let looked: Vec<String> = lcc_brand::legacy_settings_dirs().iter().map(|d| d.join(DIR_NAME).display().to_string()).collect();
            return Err(EngineError::Other(format!("no older install's presets on this computer (looked in {})", looked.join(", "))));
        }
        let own = self.app_store.as_ref().map(|s| s.dir.clone());
        let mut mine = self.content();
        let store_profiles = self.app_store.as_ref().map(AppStore::profiles_dir);
        let mut out = Vec::new();
        let mut bound = 0usize;
        for dir in &dirs {
            let mut r = MergeReport { library: dir.display().to_string(), ..Default::default() };
            if own.as_deref().is_some_and(|o| same_path(o, dir)) {
                r.problems.push(format!("{} is this app's own store", dir.display()));
            } else if !force && self.app_store.as_ref().is_some_and(|s| s.has_merged_cameras(dir)) {
                r.skipped = true;
            } else if !force && self.app_store.as_ref().is_some_and(|s| s.has_merged(dir)) {
                // merged by a build that left the camera profiles: those only, once
                let cams = crate::cmd::dcp_profiles::import_older(self, dir, camera);
                (r.camera_profiles, r.camera_already, r.camera_bodies, r.problems) = (cams.added, cams.already, cams.bound, cams.problems);
                bound += cams.bound;
                if let Some(m) = self.app_store.as_mut().and_then(|s| s.merged.iter_mut().find(|m| same_path(Path::new(&m.library), dir))) {
                    m.camera_profiles = Some(cams.added);
                }
            } else if !is_store_folder(dir) {
                r.problems.push(format!("{} holds no presets ({PRESETS} or {SHARED})", dir.display()));
            } else {
                let mut problems = Vec::new();
                let theirs = read_store_folder(dir, &mut problems);
                let (mut rr, remap) = merge_content(&mut mine, theirs, Some(&dir.join(PROFILES_DIR)), store_profiles.as_deref());
                problems.append(&mut rr.problems);
                // the camera profiles: installed now, with their bodies
                let cams = crate::cmd::dcp_profiles::import_older(self, dir, camera);
                problems.extend(cams.problems);
                rr.problems = problems;
                rr.camera_profiles = cams.added;
                rr.camera_already = cams.already;
                rr.camera_bodies = cams.bound;
                bound += cams.bound;
                rr.library = r.library.clone();
                self.record_merge(dir, &rr, remap);
                if let Some(m) = self.app_store.as_mut().and_then(|s| s.merged.last_mut()) {
                    m.camera_profiles = Some(rr.camera_profiles);
                }
                r = rr;
            }
            out.push(r);
        }
        self.set_content(mine);
        crate::profile_refs::refresh(self);
        self.save_app_store()?;
        if bound > 0
            && let Err(e) = self.save_prefs()
        {
            log::error!("store: the camera bodies' profiles weren't saved: {e}");
        }
        Ok(out)
    }

    fn check_store(&self) -> Result<()> {
        if self.app_store.is_none() {
            return Err(EngineError::Other("there is no shared store (the settings folder isn't available, or NO_PREFS is set)".into()));
        }
        if !self.uses_app_store() {
            return Err(EngineError::Other(
                "the open catalog keeps its own presets and profiles; run store.useLibrary {\"keep\": false} first".into(),
            ));
        }
        Ok(())
    }

    /// `store.gatherPresets`: the named presets (export presets, print templates, filter /
    /// metadata / curve / label / keyword-set presets) of library folders `dirs`, the open one and
    /// every one merged before, read from each library's `prefs.json` and its backups'
    /// (`Backups/*/prefs.json`), join the store, once per library (`force`: again). Nothing in the
    /// libraries is written.
    pub fn gather_presets(&mut self, dirs: &[PathBuf], force: bool) -> Result<Value> {
        self.check_store()?;
        let mut all: Vec<PathBuf> = dirs.to_vec();
        if let Some(l) = self.library.as_ref().filter(|l| l.on_disk) {
            all.push(l.dir.clone());
        }
        if let Some(st) = self.app_store.as_ref() {
            all.extend(st.merged.iter().map(|m| PathBuf::from(&m.library)));
        }
        let mut seen: Vec<PathBuf> = Vec::new();
        let mut habits = self.habits();
        let (mut libraries, mut files, mut count, mut skipped) = (0usize, 0usize, 0usize, 0usize);
        let mut problems: Vec<String> = Vec::new();
        let mut gathered: Vec<String> = Vec::new();
        for dir in all {
            if seen.iter().any(|d| same_path(d, &dir)) {
                continue;
            }
            seen.push(dir.clone());
            let done = self.app_store.as_ref().is_some_and(|s| s.gathered.iter().any(|g| same_path(Path::new(g), &dir)));
            if done && !force {
                skipped += 1;
                continue;
            }
            if !dir.is_dir() {
                // a disconnected drive: asked again next time
                problems.push(format!("{}: not found", dir.display()));
                continue;
            }
            libraries += 1;
            let mut sources = vec![dir.join("prefs.json")];
            for root in backup_roots(&dir) {
                if let Ok(rd) = std::fs::read_dir(&root) {
                    let mut b: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path().join("prefs.json")).collect();
                    b.sort();
                    sources.extend(b.into_iter().rev().take(MAX_BACKUPS));
                }
            }
            sources.dedup();
            for f in sources {
                match parse_capped::<LibraryPrefs>(&f, MAX_PREFS_FILE) {
                    Ok(Some(p)) => {
                        files += 1;
                        count += habits.merge_named_presets(p.habits);
                    }
                    Ok(None) => {}
                    Err(e) => problems.push(e),
                }
            }
            gathered.push(dir.display().to_string());
        }
        let mut c = self.content();
        c.habits = habits;
        self.set_content(c);
        if let Some(st) = self.app_store.as_mut() {
            for g in gathered {
                if !st.gathered.iter().any(|h| same_path(Path::new(h), Path::new(&g))) {
                    st.gathered.push(g);
                }
            }
        }
        self.save_app_store()?;
        Ok(serde_json::json!({"libraries": libraries, "files": files, "count": count, "skipped": skipped, "problems": problems}))
    }

    /// The open library's photos that name a profile id the store changed when it was merged move to
    /// the new id (once; not an undo step, saved with the library).
    pub(crate) fn apply_store_remap(&mut self) {
        let Some(dir) = self.library.as_ref().filter(|_| self.uses_app_store()).map(|l| l.dir.clone()) else { return };
        let Some(m) = self.app_store.as_ref().and_then(|s| s.merged.iter().find(|m| same_path(Path::new(&m.library), &dir)).cloned()) else {
            return;
        };
        if m.remapped || m.remap.is_empty() {
            return;
        }
        let changes: Vec<lcc_catalog::Op> = self
            .catalog
            .photos()
            .filter_map(|p| {
                let to = m.remap.get(&p.develop.profile.id)?;
                let mut d = (*p.develop).clone();
                d.profile.id = to.clone();
                Some(lcc_catalog::Op::SetDevelop {
                    id: p.id,
                    settings: std::sync::Arc::new(d),
                    label: "Shared profile".into(),
                    edited: p.edited.clone(),
                })
            })
            .collect();
        for op in changes {
            if let Err(e) = self.apply_unrecorded(op) {
                log::error!("store: moving a photo to its profile's new id: {e}");
            }
        }
        if let Some(rec) = self.app_store.as_mut().and_then(|s| s.merged.iter_mut().find(|r| same_path(Path::new(&r.library), &dir))) {
            rec.remapped = true;
        }
    }

    /// `store.useLibrary`: `keep` = the open library keeps its own copy of the store's content
    /// (profile files included) from now on; `!keep` = it uses the store again (its content merged
    /// in first).
    pub fn store_use_library(&mut self, keep: bool) -> Result<Value> {
        if self.app_store.is_none() {
            return Err(EngineError::Other("there is no shared store (the settings folder isn't available)".into()));
        }
        let Some(lib) = self.library.as_ref() else { return Err(EngineError::Other("no catalog is open".into())) };
        let (dir, on_disk, own) = (lib.dir.clone(), lib.on_disk, lib.own_store);
        if keep == own {
            return Ok(self.store_info());
        }
        if keep {
            // the library's own copies: profile files into its Profiles/, every list into its files
            let mut problems = Vec::new();
            if on_disk {
                let to = dir.join("Profiles");
                let from = self.profiles_dir();
                if let Err(e) = std::fs::create_dir_all(&to) {
                    problems.push(format!("{}: {e}", to.display()));
                }
                for p in &mut self.lut_profiles {
                    let Some(name) = Path::new(&p.file).file_name().map(|n| n.to_os_string()) else { continue };
                    let Some(src) = from.as_ref().map(|d| d.join(&name)) else { continue };
                    let dst = to.join(&name);
                    match read_capped(&src, MAX_PROFILE_FILE).map_err(|e| e.to_string()).and_then(|b| {
                        b.map_or(Ok(()), |b| lcc_catalog::safe_file::write_atomic(&dst, &b).map_err(|e| format!("{}: {e}", dst.display())))
                    }) {
                        Ok(()) => p.file = dst.to_string_lossy().to_string(),
                        Err(e) => problems.push(e),
                    }
                }
            }
            if let Some(lib) = self.library.as_mut() {
                lib.own_store = true;
                lib.store_with_library = true;
                lib.forget_presets_saved();
            }
            self.save_prefs()?;
            self.persist()?;
            for p in problems {
                log::warn!("store: {p}");
            }
        } else {
            if let Some(lib) = self.library.as_mut() {
                lib.own_store = false;
                lib.store_with_library = false;
            }
            // its content joins the store (again), the session takes the store's
            if let Some(st) = self.app_store.as_mut() {
                st.merged.retain(|m| !same_path(Path::new(&m.library), &dir));
            }
            self.adopt_app_store(&dir, on_disk);
            self.save_prefs()?;
        }
        Ok(self.store_info())
    }

    /// `store.info`: where the shared settings live and what they hold.
    pub fn store_info(&self) -> Value {
        let st = self.app_store.as_ref();
        serde_json::json!({
            "available": st.is_some(),
            "dir": st.map(|s| s.dir.display().to_string()),
            "inUse": self.uses_app_store(),
            "libraryKeepsOwn": self.library.as_ref().is_some_and(|l| l.own_store),
            "presets": self.presets.iter().filter(|p| !p.builtin).count(),
            "exportPresets": self.export_presets.len(),
            "lutProfiles": self.lut_profiles.len(),
            "effects": self.mask_effects.len(),
            "merged": st.map(|s| s.merged.clone()).unwrap_or_default(),
        })
    }

    /// Store warnings not handed out yet.
    pub(crate) fn take_store_warnings(&mut self) -> Vec<String> {
        self.app_store.as_mut().map(AppStore::take_warnings).unwrap_or_default()
    }
}

#[cfg(test)]
thread_local! {
    static CONTENT_COPIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How often this thread copied the store's whole content (every preset) since the last call.
#[cfg(test)]
pub(crate) fn content_copies() -> usize {
    CONTENT_COPIES.with(|c| c.replace(0))
}
