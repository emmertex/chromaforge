//! Camera colour profiles of our own, pooled from many photos of one camera model.
//!
//! A raw file without colour matrices (Sony ARW, Nikon NEF, Fujifilm RAF) gets its look fitted to its own embedded camera
//! JPEG (`camera_preview`), but one photo shows too little of some colours: a lime shirt covering a
//! few dozen proxy pixels next to a hillside of foliage at the same hue. the CLI's `calibrate`
//! pools the colour pairs of many photos per model and fits one matrix and hue/saturation/value
//! table; photos of that model then only fit their tone and chroma curves.
//!
//! Profiles are JSON files (`<model>.json`) in [`dir`]: `{ENV_PREFIX}_CAMERA_PROFILES`, else
//! `<config>/camera-profiles`; a local profile replaces the one built in ([`BUNDLED`], from
//! `assets/camera-profiles/`). They are read once per process; a damaged or hostile file is
//! ignored with a warning. They hold aggregate colour statistics only, never image content.
use lcc_color::Mat3;
use lcc_pipeline::tone::CameraTone;
use lcc_raw::profile::HsvTable;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

/// Format version of the profile files.
const VERSION: u32 = 1;
/// Largest profile file read (a 5 × 72 × 5 table is ~60 KB of JSON).
const MAX_FILE: u64 = 4 << 20;

/// Profiles built into the app (`assets/camera-profiles/`, see `assets/ATTRIBUTION.md`):
/// `(model, JSON)`.
pub const BUNDLED: &[(&str, &str)] = &[
    ("ILCA-99M2", include_str!("../../../assets/camera-profiles/ILCA-99M2.json")),
    ("ILCE-7CR", include_str!("../../../assets/camera-profiles/ILCE-7CR.json")),
    ("ILCE-7M4", include_str!("../../../assets/camera-profiles/ILCE-7M4.json")),
    ("NIKON D5300", include_str!("../../../assets/camera-profiles/NIKON_D5300.json")),
    ("NIKON D750", include_str!("../../../assets/camera-profiles/NIKON_D750.json")),
    ("NIKON D780", include_str!("../../../assets/camera-profiles/NIKON_D780.json")),
    ("NIKON D90", include_str!("../../../assets/camera-profiles/NIKON_D90.json")),
    ("NIKON D70", include_str!("../../../assets/camera-profiles/NIKON_D70.json")),
    ("NIKON D700", include_str!("../../../assets/camera-profiles/NIKON_D700.json")),
    ("X-T1", include_str!("../../../assets/camera-profiles/X-T1.json")),
    ("X100T", include_str!("../../../assets/camera-profiles/X100T.json")),
    ("X20", include_str!("../../../assets/camera-profiles/X20.json")),
    ("NIKON Z 6_2", include_str!("../../../assets/camera-profiles/NIKON_Z_6_2.json")),
    ("NIKON Z 8", include_str!("../../../assets/camera-profiles/NIKON_Z_8.json")),
    ("X-H2S", include_str!("../../../assets/camera-profiles/X-H2S.json")),
    ("X-T4", include_str!("../../../assets/camera-profiles/X-T4.json")),
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CameraProfile {
    pub version: u32,
    /// The camera model as the files report it (Exif `Model`), e.g. `ILCE-7M4`.
    pub model: String,
    /// Photos and colour pairs the profile was fitted on.
    pub files: usize,
    pub samples: usize,
    /// White-balanced camera RGB (with the baseline exposure) → linear Rec.2020, rows.
    matrix: [[f64; 3]; 3],
    /// Hue/saturation/value correction after `matrix` (linear ProPhoto RGB).
    pub hue_sat: Option<HsvTable>,
    /// The camera's tone curve pooled over the photos (scene luminance after `matrix` → the camera JPEG's): a photo
    /// follows it past what its own JPEG shows, and takes it whole when its JPEG teaches no curve. Absent in profiles
    /// fitted before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<CameraTone>,
}

impl CameraProfile {
    pub fn new(model: &str, files: usize, samples: usize, matrix: Mat3, hue_sat: Option<HsvTable>) -> CameraProfile {
        CameraProfile { version: VERSION, model: model.to_owned(), files, samples, matrix: matrix.0, hue_sat, tone: None }
    }

    /// The profile with its pooled tone curve.
    pub fn with_tone(self, tone: Option<CameraTone>) -> CameraProfile {
        CameraProfile { tone, ..self }
    }

    pub fn matrix(&self) -> Mat3 {
        Mat3(self.matrix)
    }

    /// Whether the data is usable (bounded matrix, table shape matching its data).
    fn valid(&self) -> bool {
        let matrix = self.matrix.iter().flatten().all(|v| v.is_finite() && v.abs() < 8.0) && Mat3(self.matrix).inverse().is_some();
        let table = self.hue_sat.as_ref().is_none_or(|t| {
            let dims = (1..=4096).contains(&t.hue_divisions) && (2..=4096).contains(&t.sat_divisions) && (1..=4096).contains(&t.val_divisions);
            let len = t.hue_divisions.checked_mul(t.sat_divisions).and_then(|n| n.checked_mul(t.val_divisions));
            dims && len == Some(t.data.len()) && t.data.iter().flatten().all(|v| v.is_finite()) && t.data.iter().all(|e| e[1] >= 0.0 && e[2] >= 0.0)
        });
        self.version == VERSION && !self.model.is_empty() && matrix && table
    }
}

/// The app's configuration folder (settings, GPU marker, camera profiles); one helper for the app.
pub use crate::config::config_dir;

/// Where camera profiles are read from and written to.
pub fn dir() -> Option<PathBuf> {
    lcc_brand::env_os("CAMERA_PROFILES").map(PathBuf::from).or_else(|| config_dir().map(|d| d.join("camera-profiles")))
}

/// File name of `model`'s profile: letters, digits, `-` and `_` kept, anything else `_`.
pub fn file_name(model: &str) -> String {
    let name: String = model.trim().chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    format!("{name}.json")
}

/// Read and validate a profile file.
pub fn load(path: &Path) -> Result<CameraProfile, String> {
    let size = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?.len();
    if size > MAX_FILE {
        return Err(format!("{}: {size} bytes is too large for a camera profile", path.display()));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&bytes, &path.display().to_string())
}

/// Parse and validate a profile's JSON (`origin` names it in errors).
fn parse(json: &[u8], origin: &str) -> Result<CameraProfile, String> {
    let profile: CameraProfile = serde_json::from_slice(json).map_err(|e| format!("{origin}: {e}"))?;
    if !profile.valid() {
        return Err(format!("{origin}: not a usable camera profile (version {}, model {:?})", profile.version, profile.model));
    }
    Ok(profile)
}

/// The built-in profile for `model`, if any.
fn bundled(model: &str) -> Option<CameraProfile> {
    let (_, json) = BUNDLED.iter().find(|(m, _)| *m == model)?;
    parse(json.as_bytes(), &format!("built-in profile {model}")).inspect_err(|e| eprintln!("{}: ignoring camera profile {e}", lcc_brand::BINARY)).ok()
}

/// Write `profile` to `dir` (created if needed) as `<model>.json`; returns the path.
pub fn save(profile: &CameraProfile, dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(file_name(&profile.model));
    let json = serde_json::to_vec_pretty(profile).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

fn cache() -> &'static Mutex<HashMap<String, Option<Arc<CameraProfile>>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Arc<CameraProfile>>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The profile for camera `model`: from [`dir`], else built in; read once per process.
pub fn get(model: &str) -> Option<Arc<CameraProfile>> {
    let model = model.trim();
    if model.is_empty() {
        return None;
    }
    let mut cache = cache().lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(hit) = cache.get(model) {
        return hit.clone();
    }
    let path = dir().map(|d| d.join(file_name(model)));
    let profile = match path {
        Some(path) if path.is_file() => match load(&path) {
            Ok(p) if p.model.trim() == model => Some(Arc::new(p)),
            Ok(p) => {
                eprintln!("{}: ignoring camera profile {}: it is for {:?}, not {model:?}", lcc_brand::BINARY, path.display(), p.model);
                None
            }
            Err(e) => {
                eprintln!("{}: ignoring camera profile {e}", lcc_brand::BINARY);
                None
            }
        },
        _ => None,
    };
    let profile = profile.or_else(|| bundled(model).map(Arc::new));
    cache.insert(model.to_owned(), profile.clone());
    profile
}

/// Changes when the profiles folder's contents change (per process): part of the render cache
/// keys, so thumbnails rendered before a profile existed are not reused after.
pub fn cache_key() -> u64 {
    static KEY: OnceLock<u64> = OnceLock::new();
    *KEY.get_or_init(|| cache_key_for(dir().as_deref()))
}

fn cache_key_for(directory: Option<&Path>) -> u64 {
    let entries = directory.and_then(|d| std::fs::read_dir(d).ok());
    let mut files: Vec<(String, u64, std::time::Duration)> = entries
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json" || x.eq_ignore_ascii_case("dcp")))
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            let modified = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
            Some((e.file_name().to_string_lossy().into_owned(), meta.len(), modified))
        })
        .collect();
    files.sort();
    let mut h = lcc_preview::Hasher128::new();
    for (name, len, modified) in &files {
        h.str(name).u64(*len).u64(modified.as_secs()).u64(u64::from(modified.subsec_nanos()));
    }
    for (model, json) in BUNDLED {
        h.str(model).str(json);
    }
    for (name, json) in lcc_camdb::calibration::embedded_files() {
        h.str(name).str(json);
    }
    for p in dev_calibrations_dir().map(|d| json_files(&d)).unwrap_or_default() {
        let meta = std::fs::metadata(&p).ok();
        let modified = meta.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).unwrap_or_default();
        h.str(&p.to_string_lossy()).u64(meta.map_or(0, |m| m.len())).u64(modified.as_secs()).u64(u64::from(modified.subsec_nanos()));
    }
    h.finish().0 as u64
}

// --- External `.dcp` camera profiles -------------------------------------------
//
// Imported profiles are kept in `<profiles>/dcp/`; the active one for a camera model is a copy at
// `<profiles>/<model>.dcp`. The active ones are read once per process ([`install_dcps`]), like the
// JSON profiles, and are part of [`cache_key`], so a change takes effect on the next start.

/// Where imported `.dcp` files are kept.
pub fn dcp_library_dir() -> Option<PathBuf> {
    dir().map(|d| d.join("dcp"))
}

/// File name of `model`'s active `.dcp`.
pub fn dcp_file_name(model: &str) -> String {
    let json = file_name(model);
    format!("{}.dcp", json.trim_end_matches(".json"))
}

/// Read a `.dcp` file.
pub fn load_dcp(path: &Path) -> Result<lcc_raw::dcp::Dcp, String> {
    let size = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?.len();
    if size > lcc_raw::dcp::MAX_FILE as u64 {
        return Err(format!("{}: {size} bytes is too large for a camera profile", path.display()));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    lcc_raw::dcp::parse(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// The active `.dcp` files in [`dir`] and what they are (damaged ones skipped with a warning).
pub fn active_dcps() -> Vec<(PathBuf, lcc_raw::dcp::Dcp)> {
    let Some(d) = dir() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(&d) else { return Vec::new() };
    let mut paths: Vec<PathBuf> =
        entries.flatten().map(|e| e.path()).filter(|p| p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dcp"))).collect();
    paths.sort();
    paths
        .into_iter()
        .filter_map(|p| match load_dcp(&p) {
            Ok(d) => Some((p, d)),
            Err(e) => {
                log::warn!("ignoring camera profile {e}");
                None
            }
        })
        .collect()
}

/// The imported `.dcp` files in [`dcp_library_dir`]: `(id, profile)`, the id being `dcp:<file stem>`.
pub fn library_dcps() -> Vec<(String, lcc_raw::dcp::Dcp)> {
    let Some(d) = dcp_library_dir() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(&d) else { return Vec::new() };
    let mut paths: Vec<PathBuf> =
        entries.flatten().map(|e| e.path()).filter(|p| p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dcp"))).collect();
    paths.sort();
    paths
        .into_iter()
        .filter_map(|p| {
            let stem = p.file_stem()?.to_string_lossy().to_string();
            match load_dcp(&p) {
                Ok(d) => Some((format!("dcp:{stem}"), d)),
                Err(e) => {
                    log::warn!("ignoring camera profile {e}");
                    None
                }
            }
        })
        .collect()
}

/// Install the active `.dcp` profiles (each camera's default) and the imported library (chosen per
/// photo) for this process (once; later calls do nothing).
pub fn install_dcps() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        let list: Vec<_> = active_dcps().into_iter().map(|(_, d)| d).collect();
        if !list.is_empty() {
            log::info!("camera profiles (.dcp): {}", list.iter().map(|d| format!("{} ({})", d.name, d.model)).collect::<Vec<_>>().join(", "));
        }
        lcc_raw::dcp::install(list);
        lcc_raw::dcp::install_library(library_dcps());
        lcc_raw::dcp::install_calibrated(calibrations().iter().map(lcc_camdb::calibration::Calibration::calibrated).collect());
    });
}

/// Folder of calibration files under development (debug and test builds only): `{ENV_PREFIX}_CAMERA_CALIBRATIONS`.
fn dev_calibrations_dir() -> Option<PathBuf> {
    if cfg!(debug_assertions) { lcc_brand::env_os("CAMERA_CALIBRATIONS").map(PathBuf::from) } else { None }
}

/// The `.json` files in `dir`, sorted.
fn json_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut paths: Vec<PathBuf> =
        entries.flatten().map(|e| e.path()).filter(|p| p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("json"))).collect();
    paths.sort();
    paths
}

/// Calibration files in `dir` (damaged ones skipped with a warning).
fn load_calibrations(dir: &Path) -> Vec<lcc_camdb::calibration::Calibration> {
    json_files(dir)
        .into_iter()
        .filter_map(|p| {
            let read = std::fs::metadata(&p)
                .map_err(|e| e.to_string())
                .and_then(|m| if m.len() > lcc_camdb::calibration::MAX_FILE as u64 { Err("too large".to_string()) } else { Ok(()) })
                .and_then(|()| std::fs::read_to_string(&p).map_err(|e| e.to_string()))
                .and_then(|t| lcc_camdb::calibration::Calibration::parse(&t).map_err(|e| e.to_string()));
            read.inspect_err(|e| log::warn!("ignoring camera calibration {}: {e}", p.display())).ok()
        })
        .collect()
}

/// The bundled calibrations, with a development file for the same camera replacing the bundled one.
fn calibrations() -> Vec<lcc_camdb::calibration::Calibration> {
    merge_calibrations(lcc_camdb::calibration::embedded().to_vec(), dev_calibrations_dir().map(|d| load_calibrations(&d)).unwrap_or_default())
}

fn merge_calibrations(
    mut list: Vec<lcc_camdb::calibration::Calibration>,
    dev: Vec<lcc_camdb::calibration::Calibration>,
) -> Vec<lcc_camdb::calibration::Calibration> {
    // a development calibration replaces the bundled one for the same camera and editor; other editors' stay
    for c in dev {
        list.retain(|b| !(b.editor == c.editor && b.matches(Some(&c.make), &c.model)));
        list.push(c);
    }
    list
}

/// Replace `color` with the colour model a photo's camera-profile choice names (see
/// [`lcc_raw::dcp::resolve`]): an imported `.dcp`, its camera's default `.dcp`, or nothing
/// (the app's own colour). Returns the profile's name when one was applied.
pub fn apply_choice(color: &mut lcc_raw::ColorData, meta: &lcc_meta::Metadata, choice: &str) -> Option<String> {
    let model = meta.model.as_deref()?;
    let dcp = lcc_raw::dcp::resolve(choice, meta.make.as_deref(), model)?;
    dcp.apply(color);
    if lcc_pipeline::profiling() {
        eprintln!("[profile] camera colour from {:?} ({})", dcp.name, dcp.model);
    }
    Some(dcp.name.clone())
}

/// [`apply_choice`] with the camera's default.
pub fn apply_dcp(color: &mut lcc_raw::ColorData, meta: &lcc_meta::Metadata) -> Option<String> {
    apply_choice(color, meta, "")
}

/// Per camera model: the photos read and their pooled colour pairs and tone samples.
#[derive(Default)]
pub struct Pool {
    models: HashMap<String, Model>,
}

#[derive(Default)]
struct Model {
    files: usize,
    pairs: Vec<([f64; 3], [f64; 3])>,
    tone: Vec<([f64; 3], f64)>,
}

/// Most colour pairs (and, separately, tone samples) kept per photo, so a few busy photos can't dominate a profile.
const PAIRS_PER_FILE: usize = 4000;

/// Every `n`-th item, `n` chosen to keep at most [`PAIRS_PER_FILE`].
fn thinned<T>(items: Vec<T>) -> impl Iterator<Item = T> {
    let step = items.len().div_ceil(PAIRS_PER_FILE).max(1);
    items.into_iter().step_by(step)
}

impl Pool {
    /// Add one raw file's colour pairs. `Ok(None)` when the file can't contribute (not an ARW,
    /// NEF or RAF without colour matrices, no usable camera JPEG, too little colour).
    pub fn add(&mut self, bytes: &[u8]) -> Result<Option<String>, String> {
        let mut raw = lcc_raw::decode(bytes).map_err(|e| e.to_string())?;
        // colour only: geometric lens corrections would stop the sensor proxy from binning (the pairs still follow
        // the warp the camera applied to its JPEG)
        let lens = crate::camera_preview::lens_geometry(&raw.opcodes.list3);
        raw.opcodes.list3.retain(|op| !op.is_lens_correction());
        let Some(model) = raw.metadata.model.as_deref().map(str::trim).filter(|m| !m.is_empty()) else { return Ok(None) };
        let Some(samples) = crate::camera_preview::profile_samples(&raw, bytes, lens.as_ref()) else { return Ok(None) };
        let entry = self.models.entry(model.to_owned()).or_default();
        entry.files += 1;
        entry.pairs.extend(thinned(samples.colour));
        entry.tone.extend(thinned(samples.tone));
        Ok(Some(model.to_owned()))
    }

    /// Fit a profile per model with at least `min_files` photos.
    pub fn fit(&self, min_files: usize) -> Vec<Result<CameraProfile, String>> {
        let mut models: Vec<_> = self.models.iter().collect();
        models.sort_by(|a, b| a.0.cmp(b.0));
        models
            .into_iter()
            .filter(|(_, m)| m.files >= min_files)
            .map(|(model, m)| {
                let (matrix, hue_sat) = crate::camera_preview::fit_profile(&m.pairs).ok_or_else(|| format!("{model}: no usable colour fit"))?;
                let tone = crate::camera_preview::fit_profile_tone(&m.tone, &matrix);
                Ok(CameraProfile::new(model, m.files, m.pairs.len(), matrix, hue_sat).with_tone(tone))
            })
            .collect()
    }

    /// Add another pool's photos and pairs (pools filled on separate threads).
    pub fn merge(&mut self, other: Pool) {
        for (model, m) in other.models {
            let entry = self.models.entry(model).or_default();
            entry.files += m.files;
            entry.pairs.extend(m.pairs);
            entry.tone.extend(m.tone);
        }
    }

    /// Photos read per model.
    pub fn files(&self) -> Vec<(String, usize)> {
        let mut v: Vec<_> = self.models.iter().map(|(name, m)| (name.clone(), m.files)).collect();
        v.sort();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calibration(model: &str, look: &str) -> lcc_camdb::calibration::Calibration {
        use lcc_camdb::calibration as c;
        let fm = Mat3([[0.70, 0.18, 0.08], [0.28, 0.80, -0.08], [0.02, -0.15, 0.95]]);
        let cm = Mat3([[0.9, -0.3, -0.05], [-0.5, 1.4, 0.1], [-0.1, 0.3, 0.6]]);
        c::Calibration {
            version: c::VERSION,
            make: "TESTMAKE".into(),
            model: model.into(),
            aliases: vec![],
            editor: c::Editor::Lightroom,
            method: c::METHOD.into(),
            tool_version: "1".into(),
            contributor: None,
            licence: c::LICENCE.into(),
            camera: c::CameraModel { illuminants: [17, 21], color_matrix: [cm, cm], forward_matrix: [fm, fm], hue_sat_map: None },
            looks: vec![c::Look { name: look.into(), baseline_exposure_offset: 0.0, look_table: None, tone_curve: vec![] }],
            evidence: c::Evidence { photos: 10, samples: 100, hue_coverage: 0.5, ..Default::default() },
        }
    }

    #[test]
    fn dev_calibrations_skip_damaged_files_and_replace_bundled_ones() {
        let dir = std::env::temp_dir().join(format!("lc-calibrations-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let good = calibration("CAM ONE", "Dev Look");
        std::fs::write(dir.join("good.json"), good.to_json().unwrap()).unwrap();
        std::fs::write(dir.join("bad.json"), "{\"version\": 1").unwrap();
        let loaded = load_calibrations(&dir);
        assert_eq!(loaded, vec![good.clone()]);
        let merged = merge_calibrations(vec![calibration("CAM ONE", "Bundled"), calibration("CAM TWO", "Other")], loaded);
        assert_eq!(merged.len(), 2);
        assert!(merged.iter().any(|c| c.model == "CAM ONE" && c.looks[0].name == "Dev Look"));
        assert!(merged.iter().all(|c| c.looks[0].name != "Bundled"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn profile() -> CameraProfile {
        let table = HsvTable { hue_divisions: 4, sat_divisions: 2, val_divisions: 1, data: vec![[5.0, 1.2, 1.0]; 8], srgb_value: false };
        CameraProfile::new("ILCE-7M4", 12, 3456, Mat3([[1.6, -0.5, -0.1], [-0.2, 1.3, -0.1], [-0.1, -0.2, 1.3]]), Some(table))
    }

    #[test]
    fn profiles_round_trip_and_reject_bad_files() {
        let dir = std::env::temp_dir().join(format!("lc-camera-profiles-{}", std::process::id()));
        let path = save(&profile(), &dir).unwrap();
        assert_eq!(path.file_name().unwrap(), "ILCE-7M4.json");
        assert_eq!(load(&path).unwrap(), profile());
        let bad = |p: CameraProfile| {
            let path = dir.join("bad.json");
            std::fs::write(&path, serde_json::to_vec(&p).unwrap()).unwrap();
            load(&path).is_err()
        };
        let mut p = profile();
        p.matrix[0][0] = f64::NAN;
        assert!(bad(p), "non-finite matrix");
        let mut p = profile();
        p.matrix = [[0.0; 3]; 3];
        assert!(bad(p), "singular matrix");
        let mut p = profile();
        if let Some(t) = p.hue_sat.as_mut() {
            t.data.truncate(3);
        }
        assert!(bad(p), "table data doesn't match its shape");
        let mut p = profile();
        p.version = 99;
        assert!(bad(p), "unknown version");
        // a pooled tone curve round-trips; a broken one (knots going backwards) is a damaged file
        let toned = profile().with_tone(CameraTone::default_raw());
        let path = save(&toned, &dir).unwrap();
        assert_eq!(load(&path).unwrap(), toned);
        let mut json: serde_json::Value = serde_json::to_value(&toned).unwrap();
        json["tone"]["knots"][3][0] = serde_json::json!(5.0);
        std::fs::write(dir.join("bad.json"), serde_json::to_vec(&json).unwrap()).unwrap();
        assert!(load(&dir.join("bad.json")).is_err(), "invalid tone curve");
        std::fs::write(dir.join("junk.json"), b"{not json").unwrap();
        assert!(load(&dir.join("junk.json")).is_err());
        assert!(load(&dir.join("missing.json")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bundled_profiles_are_valid_and_named_after_their_model() {
        assert!(!BUNDLED.is_empty());
        for (model, json) in BUNDLED {
            let p = parse(json.as_bytes(), model).unwrap();
            assert_eq!(p.model, *model);
            assert!(p.files >= 5 && p.hue_sat.is_some(), "{model}: {} photos", p.files);
            assert_eq!(bundled(model), Some(p));
        }
        assert!(bundled("No Such Camera").is_none());
    }

    #[test]
    fn same_second_profile_replacement_changes_render_cache_key() {
        let dir = std::env::temp_dir().join(format!("lc-camera-profile-cache-{}", std::process::id()));
        let mut first = profile();
        first.model = "ILCE-7CR".into();
        let path = save(&first, &dir).unwrap();
        let set_modified = |millis| {
            let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000) + std::time::Duration::from_millis(millis);
            let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            file.set_times(std::fs::FileTimes::new().set_modified(time)).unwrap();
            file.metadata().unwrap()
        };
        let before = set_modified(100);
        assert_eq!(load(&path).unwrap(), first);
        let before_key = cache_key_for(Some(&dir));

        let mut second = first;
        second.matrix[0][0] = 1.7;
        save(&second, &dir).unwrap();
        let after = set_modified(350);
        assert_eq!(load(&path).unwrap(), second);
        assert_eq!(before.len(), after.len());
        let before_time = before.modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap();
        let after_time = after.modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap();
        assert_eq!(before_time.as_secs(), after_time.as_secs());
        assert_ne!(before_time.subsec_nanos(), after_time.subsec_nanos());
        let after_key = cache_key_for(Some(&dir));
        let _ = std::fs::remove_dir_all(dir);

        assert_ne!(before_key, after_key, "a changed profile must not reuse the previous render cache");
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(file_name("ILCE-7M4"), "ILCE-7M4.json");
        assert_eq!(file_name(" DSC-RX100M3 "), "DSC-RX100M3.json");
        assert_eq!(file_name("../../etc/passwd"), "______etc_passwd.json");
        assert_eq!(file_name("Ω 1/2"), "__1_2.json");
    }

    #[test]
    fn pool_skips_files_that_cannot_contribute() {
        let mut pool = Pool::default();
        assert!(pool.add(b"not a raw file").is_err());
        assert!(pool.fit(1).is_empty());
        assert!(pool.files().is_empty());
    }
}
