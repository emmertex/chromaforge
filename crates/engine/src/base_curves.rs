//! The base curve folder: curve files a user puts in `base-curves` (see `docs/base-curve-format.md`).
//!
//! The folder is `$<PREFIX>_BASE_CURVES`, else `<config>/base-curves` (portable mode and `<PREFIX>_CONFIG_DIR`
//! apply, as for the camera profiles). Each `<stem>.json` is one curve with the id `base:<stem>`. The files are read
//! with a size bound, a damaged or hostile one is skipped with a warning, and the result is installed into
//! `lcc_develop::base_curve`'s library, which the pipeline reads. Nothing is bundled with the app.

use lcc_develop::base_curve;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

/// Where base curve files are read from.
pub fn dir() -> Option<PathBuf> {
    lcc_brand::env_os("BASE_CURVES").map(PathBuf::from).or_else(|| crate::config::config_dir().map(|d| d.join("base-curves")))
}

/// What a scan of a folder found.
#[derive(Default)]
pub struct Scan {
    /// `(id, curve, content hash)` in file-name order.
    pub curves: Vec<(String, base_curve::BaseCurve, u64)>,
    /// One line per file that was skipped.
    pub warnings: Vec<String>,
}

/// Read every `*.json` file of folder `d`. A file is skipped (with a warning) when it is not a regular file, is
/// larger than [`base_curve::MAX_FILE`], is not a valid curve file, or has a name that cannot be an id.
pub fn scan_in(d: &Path) -> Scan {
    let mut out = Scan::default();
    let Ok(entries) = std::fs::read_dir(d) else { return out };
    let mut paths: Vec<PathBuf> =
        entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("json"))).collect();
    paths.sort();
    for p in paths {
        let Some(stem) = p.file_stem().map(|s| s.to_string_lossy().into_owned()) else { continue };
        let shown = format!("{stem}.json");
        let id = base_curve::id_for(&stem);
        if !base_curve::is_valid_id(&id) {
            out.warnings.push(format!("{shown}: the name cannot be used as a base curve id"));
            continue;
        }
        if out.curves.iter().any(|(i, _, _)| *i == id) {
            out.warnings.push(format!("{shown}: another file has the same name"));
            continue;
        }
        let bytes = match crate::bounded_read::read_bounded(&p, base_curve::MAX_FILE) {
            Ok(b) => b,
            Err(e) => {
                out.warnings.push(format!("{shown}: {e}"));
                continue;
            }
        };
        match base_curve::parse(&bytes) {
            Ok(c) => out.curves.push((id, c, base_curve::hash_bytes(&bytes))),
            Err(e) => out.warnings.push(format!("{shown}: {e}")),
        }
    }
    out
}

fn last_warnings() -> &'static Mutex<Vec<String>> {
    static W: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    W.get_or_init(|| Mutex::new(Vec::new()))
}

/// Scan [`dir`] again and install what it holds (replacing the installed curves); returns the warnings.
pub fn refresh() -> Vec<String> {
    let scan = dir().map(|d| scan_in(&d)).unwrap_or_default();
    for w in &scan.warnings {
        log::warn!("ignoring base curve {w}");
    }
    if !scan.curves.is_empty() {
        log::info!("base curves: {} in the folder", scan.curves.len());
    }
    let warnings = scan.warnings.clone();
    base_curve::install_library(scan.curves);
    *last_warnings().lock().unwrap_or_else(PoisonError::into_inner) = scan.warnings;
    warnings
}

/// Install the folder's curves for this process (once; later calls do nothing, [`refresh`] rescans).
pub fn install_once() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        refresh();
    });
}

/// The warnings of the last scan.
pub fn warnings() -> Vec<String> {
    last_warnings().lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// One installed curve, for a selector.
#[derive(Clone, Debug, PartialEq)]
pub struct Listed {
    pub id: String,
    /// The file stem (the id without its prefix).
    pub stem: String,
    /// The file's own `name`, or the stem.
    pub name: String,
    /// Why this build cannot render it, if it cannot.
    pub unsupported: Option<&'static str>,
    /// The camera model the file says it is for (empty: any).
    pub camera: String,
}

/// The installed curves in id order.
pub fn list() -> Vec<Listed> {
    base_curve::library()
        .into_iter()
        .map(|e| {
            let stem = e.id.strip_prefix(base_curve::ID_PREFIX).unwrap_or(&e.id).to_string();
            let name = if e.curve.name.is_empty() { stem.clone() } else { e.curve.name.clone() };
            Listed { id: e.id.clone(), stem, name, unsupported: e.curve.unsupported(), camera: e.curve.camera.clone() }
        })
        .collect()
}

/// Letters and digits of `s`, lower-cased: what camera names are compared by.
fn squash(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Whether a curve made for `curve_camera` is for `photo_camera`: the same name ignoring case, spaces and punctuation,
/// or the photo's name is that with a maker in front (`Nikon Corporation Nikon Z 8` for `Nikon Z 8`). A curve with no
/// camera is for none in particular.
pub fn matches_camera(curve_camera: &str, photo_camera: &str) -> bool {
    let (c, p) = (squash(curve_camera), squash(photo_camera));
    !c.is_empty() && (p == c || p.ends_with(&c))
}

/// [`list`] with the curves for `camera` (the photo's camera model) first, each group in id order.
pub fn list_for(camera: &str) -> Vec<Listed> {
    let (mut first, rest): (Vec<Listed>, Vec<Listed>) = list().into_iter().partition(|l| matches_camera(&l.camera, camera));
    first.extend(rest);
    first
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("base-curves-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("temp dir");
        d
    }

    fn good(mid: f64) -> String {
        json!({
            "format": "base-curve", "version": 1, "name": "synthetic",
            "curves": { "a": { "interpolation": "linear", "points": [[0, 0], [0.5, mid], [1, 1]] } },
            "apply": [ { "op": "curve", "curve": "a", "on": "luminance" } ]
        })
        .to_string()
    }

    #[test]
    fn a_missing_folder_is_empty_and_files_become_ids_in_name_order() {
        assert!(scan_in(Path::new("/definitely/not/here")).curves.is_empty());
        let d = tmp("scan");
        std::fs::write(d.join("b.json"), good(0.7)).expect("write");
        std::fs::write(d.join("A one.JSON"), good(0.6)).expect("write");
        std::fs::write(d.join("notes.txt"), "x").expect("write");
        let s = scan_in(&d);
        let ids: Vec<&str> = s.curves.iter().map(|c| c.0.as_str()).collect();
        assert_eq!(ids, ["base:A one", "base:b"]);
        assert!(s.warnings.is_empty(), "{:?}", s.warnings);
        assert_ne!(s.curves[0].2, s.curves[1].2, "content hashes differ");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn bad_files_are_skipped_with_a_warning_and_never_stop_the_scan() {
        let d = tmp("bad");
        std::fs::write(d.join("ok.json"), good(0.7)).expect("write");
        std::fs::write(d.join("garbage.json"), "{{{{").expect("write");
        std::fs::write(d.join("newer.json"), good(0.7).replace("\"version\":1", "\"version\":9")).expect("write");
        std::fs::write(d.join("big.json"), vec![b' '; base_curve::MAX_FILE as usize + 10]).expect("write");
        std::fs::create_dir_all(d.join("folder.json")).expect("dir named like a file");
        std::fs::write(d.join("a:b.json"), good(0.7)).ok(); // not creatable on Windows; the scan must cope either way
        let s = scan_in(&d);
        assert_eq!(s.curves.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(), ["base:ok"]);
        assert!(s.warnings.len() >= 4, "{:?}", s.warnings);
        assert!(s.warnings.iter().any(|w| w.starts_with("garbage.json")));
        assert!(s.warnings.iter().any(|w| w.starts_with("newer.json") && w.contains("version")));
        assert!(s.warnings.iter().any(|w| w.starts_with("big.json")));
        assert!(s.warnings.iter().any(|w| w.starts_with("folder.json")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_selected_curve_that_is_not_installed_keeps_its_id() {
        // the setting is only an id: nothing about it depends on the folder
        let mut s = lcc_develop::DevelopSettings::default();
        s.profile.base_curve = "base:gone".into();
        let back = lcc_develop::DevelopSettings::from_json(&s.to_json()).expect("round trip");
        assert_eq!(back.profile.base_curve, "base:gone");
        assert!(base_curve::get("base:gone").is_none());
    }

    /// A Lightroom import never writes the field: `crs` maps Lightroom's own settings to ours and has no base curve.
    #[test]
    fn imports_from_lightroom_settings_never_name_a_base_curve() {
        let mut props = crate::crs::Props::new();
        for k in ["crs:Exposure2012", "crs:Contrast2012", "crs:Saturation", "crs:CameraProfile", "crs:Highlights2012"] {
            props.insert(k.into(), vec!["+0.5".into()]);
        }
        let partial = crate::crs::to_partial(&props, crate::crs::Target::Any);
        let text = partial.to_string();
        assert!(!text.contains("base_curve") && !text.contains("base:"), "{text}");
    }

    #[test]
    fn a_curve_matches_the_camera_it_names_and_the_selector_lists_those_first() {
        assert!(matches_camera("Nikon Z 8", "NIKON Z 8"));
        assert!(matches_camera("NikonZ8", "Nikon Z 8"));
        assert!(matches_camera("Nikon Z 8", "NIKON CORPORATION NIKON Z 8"));
        assert!(!matches_camera("Nikon Z 8", "Nikon Z 7"));
        assert!(!matches_camera("Nikon D3", "Nikon D300"), "a name that only starts the other is not it");
        assert!(!matches_camera("Nikon Z 7", "Nikon Z 7II"));
        assert!(!matches_camera("", "Nikon Z 8"), "no camera: for none in particular");
        let doc = |camera: &str| {
            json!({
                "format": "base-curve", "version": 1, "camera": camera,
                "curves": { "a": { "interpolation": "linear", "points": [[0, 0], [1, 1]] } },
                "apply": [ { "op": "curve", "curve": "a", "on": "luminance" } ]
            })
            .to_string()
            .into_bytes()
        };
        let item = |id: &str, camera: &str| {
            let b = doc(camera);
            (id.to_string(), base_curve::parse(&b).expect("valid"), base_curve::hash_bytes(&b))
        };
        base_curve::install_library(vec![
            item("base:A-any", ""),
            item("base:B-other", "Other Maker X"),
            item("base:C-mine", "Test Maker Q 1"),
            item("base:D-mine", "TestMaker Q1"),
        ]);
        let ids: Vec<String> = list_for("TEST MAKER CORPORATION Test Maker Q 1").into_iter().map(|l| l.id).collect();
        assert_eq!(ids, ["base:C-mine", "base:D-mine", "base:A-any", "base:B-other"]);
        let all: Vec<String> = list_for("").into_iter().map(|l| l.id).collect();
        assert_eq!(all, ["base:A-any", "base:B-other", "base:C-mine", "base:D-mine"], "no camera: id order");
        base_curve::install_library(vec![]);
    }
}
