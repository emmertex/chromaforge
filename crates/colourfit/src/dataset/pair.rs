//! Finding raws and exports, reading their metadata, checking the export is at the baseline preset, and pairing
//! them by camera model + capture time (file stem as the fallback).
use super::{file_name, read_file};
use crate::dataset::ExportSet;
use lcc_meta::DateTime;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const MAX_DEPTH: usize = 8;
const MAX_FILES: usize = 100_000;

const RAW_EXT: &[&str] = &[
    "3fr", "ari", "arw", "bay", "cr2", "cr3", "crw", "dcr", "dng", "erf", "fff", "iiq", "k25", "kdc", "mef", "mos", "mrw", "nef", "nrw", "orf",
    "ori", "pef", "raf", "raw", "rw2", "rwl", "sr2", "srf", "srw", "x3f",
];
const EXPORT_EXT: &[&str] = &["tif", "tiff", "jpg", "jpeg"];

/// `(y, mo, d, h, mi, s, ms)`; `ms` 0 when the file has none.
pub type TimeKey = (u16, u8, u8, u8, u8, u8, u16);

fn time_key(t: &Option<DateTime>) -> Option<TimeKey> {
    t.as_ref().map(|t| (t.year, t.month, t.day, t.hour, t.minute, t.second, t.millis))
}

pub struct RawFile {
    pub path: PathBuf,
    pub make: String,
    pub model: String,
    pub time_key: Option<TimeKey>,
}

pub struct Export {
    pub path: PathBuf,
    pub look: String,
    pub temp_tint: Option<(f64, f64)>,
    pub model: String,
    pub time_key: Option<TimeKey>,
}

pub struct Pair {
    pub raw: usize,
    pub export: usize,
}

fn ext_in(p: &Path, list: &[&str]) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| list.iter().any(|l| e.eq_ignore_ascii_case(l)))
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>, skipped: &mut Vec<(String, String)>) {
    walk_ext(dir, depth, RAW_EXT, out, skipped);
}

/// Files with one of `exts` under `dir` and its subfolders (depth- and count-capped, links not followed).
fn walk_ext(dir: &Path, depth: usize, exts: &[&str], out: &mut Vec<PathBuf>, skipped: &mut Vec<(String, String)>) {
    if depth > MAX_DEPTH || out.len() >= MAX_FILES {
        return;
    }
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => {
            skipped.push((file_name(dir), format!("cannot list folder: {e}")));
            return;
        }
    };
    let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for p in entries {
        // never follow links out of the tree
        let Ok(md) = std::fs::symlink_metadata(&p) else { continue };
        if md.is_dir() {
            walk_ext(&p, depth + 1, exts, out, skipped);
        } else if md.is_file() && ext_in(&p, exts) && out.len() < MAX_FILES {
            out.push(p);
        }
    }
}

/// Every raw named or found under a named folder, with its make, model and capture time.
pub fn raw_files(paths: &[PathBuf], skipped: &mut Vec<(String, String)>) -> Vec<RawFile> {
    let mut list = Vec::new();
    for p in paths {
        if p.is_dir() {
            walk(p, 0, &mut list, skipped);
        } else {
            list.push(p.clone());
        }
    }
    list.sort();
    list.dedup();
    let mut out = Vec::new();
    for path in list {
        let info = read_file(&path).and_then(|b| lcc_raw::probe_info(&b).map_err(|e| format!("not a supported raw: {e}")));
        match info {
            Ok(info) => {
                let m = &info.metadata;
                out.push(RawFile {
                    make: m.make.clone().unwrap_or_default().trim().to_string(),
                    model: m.model.clone().unwrap_or_default().trim().to_string(),
                    time_key: time_key(&m.capture_time),
                    path,
                });
            }
            Err(e) => skipped.push((file_name(&path), e)),
        }
    }
    out
}

fn first<'a>(props: &'a BTreeMap<String, Vec<String>>, k: &str) -> Option<&'a str> {
    props.get(k).and_then(|v| v.first()).map(|s| s.trim()).filter(|s| !s.is_empty())
}

/// The profile an export was rendered with: its `crs:Look` name (Lightroom's creative profiles sit on top of the
/// camera profile), else its `crs:CameraProfile`.
pub fn look_name(props: &BTreeMap<String, Vec<String>>) -> Option<String> {
    first(props, "crs:Look/crs:Name").or_else(|| first(props, "crs:CameraProfile")).map(str::to_string)
}

fn num(props: &BTreeMap<String, Vec<String>>, k: &str) -> Option<f64> {
    first(props, k).and_then(|s| s.trim_start_matches('+').parse::<f64>().ok()).filter(|v| v.is_finite())
}

/// Sliders that must be zero at the baseline (they change tone or colour).
const ZERO: &[&str] = &[
    "Exposure2012",
    "Contrast2012",
    "Highlights2012",
    "Shadows2012",
    "Whites2012",
    "Blacks2012",
    "Texture",
    "Clarity2012",
    "Dehaze",
    "Vibrance",
    "Saturation",
    "ParametricShadows",
    "ParametricDarks",
    "ParametricLights",
    "ParametricHighlights",
    "VignetteAmount",
    "PostCropVignetteAmount",
    "GrainAmount",
    "LensProfileEnable",
    "LensManualDistortionAmount",
    "SplitToningShadowSaturation",
    "SplitToningHighlightSaturation",
    "ColorGradeMidtoneSat",
    "ColorGradeShadowSat",
    "ColorGradeHighlightSat",
    "ColorGradeGlobalSat",
    "ShadowTint",
    "RedHue",
    "RedSaturation",
    "GreenHue",
    "GreenSaturation",
    "BlueHue",
    "BlueSaturation",
];
const HSL_PREFIX: &[&str] = &["HueAdjustment", "SaturationAdjustment", "LuminanceAdjustment"];

/// Why the export is not at the baseline preset, if it is not.
pub fn baseline_problem(props: &BTreeMap<String, Vec<String>>) -> Option<String> {
    for k in ZERO {
        if let Some(v) = num(props, &format!("crs:{k}"))
            && v.abs() > 1e-6
        {
            return Some(format!("not at the baseline preset ({k} = {v})"));
        }
    }
    for k in props.keys() {
        let Some(name) = k.strip_prefix("crs:") else { continue };
        if HSL_PREFIX.iter().any(|p| name.starts_with(p))
            && let Some(v) = num(props, k)
            && v.abs() > 1e-6
        {
            return Some(format!("not at the baseline preset ({name} = {v})"));
        }
    }
    // a creative profile at another strength than 100 % is not the profile's own look
    if let Some(a) = num(props, "crs:Look/crs:Amount")
        && (a - 1.0).abs() > 1e-3
    {
        return Some(format!("profile amount {:.0} % (export at 100 %)", a * 100.0));
    }
    if first(props, "crs:HasCrop").is_some_and(|s| s.eq_ignore_ascii_case("true")) {
        return Some("cropped (export without a crop)".into());
    }
    if first(props, "crs:ConvertToGrayscale").is_some_and(|s| s.eq_ignore_ascii_case("true")) {
        return Some("black & white export".into());
    }
    if let Some(wb) = first(props, "crs:WhiteBalance")
        && !wb.eq_ignore_ascii_case("As Shot")
    {
        return Some(format!("white balance is \"{wb}\", not As Shot"));
    }
    for k in ["crs:ToneCurvePV2012", "crs:ToneCurvePV2012Red", "crs:ToneCurvePV2012Green", "crs:ToneCurvePV2012Blue"] {
        if let Some(items) = props.get(k) {
            let linear = items.iter().all(|it| {
                it.split_once(',')
                    .is_some_and(|(x, y)| matches!((x.trim().parse::<f64>(), y.trim().parse::<f64>()), (Ok(x), Ok(y)) if (x - y).abs() < 0.5))
            });
            if !linear {
                return Some("tone curve is not linear".into());
            }
        }
    }
    None
}

/// Every export in the sets, with its look name, Lightroom temperature / tint and capture metadata.
pub fn export_files(sets: &[ExportSet], skipped: &mut Vec<(String, String)>) -> Vec<Export> {
    export_files_with(sets, true, skipped)
}

/// [`export_files`]; `baseline` refuses exports that are not at the baseline preset (a fit needs them, a
/// verification of edits does not).
pub fn export_files_with(sets: &[ExportSet], baseline: bool, skipped: &mut Vec<(String, String)>) -> Vec<Export> {
    let mut out = Vec::new();
    // every export under each folder, its subfolders included; a file reached through two folders counts once
    let mut files: Vec<PathBuf> = Vec::new();
    for set in sets {
        if !set.dir.is_dir() {
            skipped.push((file_name(&set.dir), "not a folder".into()));
            continue;
        }
        walk_ext(&set.dir, 0, EXPORT_EXT, &mut files, skipped);
    }
    let mut seen = std::collections::HashSet::new();
    files.retain(|p| seen.insert(std::fs::canonicalize(p).unwrap_or_else(|_| p.clone())));
    {
        for path in files.into_iter().take(MAX_FILES) {
            let bytes = match read_file(&path) {
                Ok(b) => b,
                Err(e) => {
                    skipped.push((file_name(&path), e));
                    continue;
                }
            };
            if lcc_codecs::sniff(&bytes).is_none() {
                skipped.push((file_name(&path), "not an image".into()));
                continue;
            }
            let meta = lcc_meta::extract(&bytes);
            let props = lcc_meta::embedded(&bytes).xmp.and_then(|x| lcc_meta::parse_xmp(&x).ok()).map(|d| d.properties).unwrap_or_default();
            if let Some(why) = baseline_problem(&props).filter(|_| baseline) {
                skipped.push((file_name(&path), why));
                continue;
            }
            // the profile chosen in Lightroom: a creative profile ("Adobe Color", "Adobe Vivid"…) is a Look on top
            // of the camera profile ("Adobe Standard"), else the camera profile itself, else the export's folder
            let folder = path.parent().map(file_name).unwrap_or_default();
            let look = look_name(&props).unwrap_or(folder);
            let temp_tint = num(&props, "crs:Temperature").zip(num(&props, "crs:Tint"));
            out.push(Export {
                path,
                look,
                temp_tint,
                model: meta.model.unwrap_or_default().trim().to_string(),
                time_key: time_key(&meta.capture_time),
            });
        }
    }
    out
}

/// The groups of Lightroom controls an edit uses (`crs:` properties of its XMP), for grouping verification
/// results; `default` when it uses none of them.
pub fn edited_groups(props: &BTreeMap<String, Vec<String>>) -> Vec<&'static str> {
    let nonzero = |k: &str| num(props, &format!("crs:{k}")).is_some_and(|v| v.abs() > 1e-6);
    let any = |ks: &[&str]| ks.iter().any(|k| nonzero(k));
    let yes = |k: &str| first(props, &format!("crs:{k}")).is_some_and(|s| s.eq_ignore_ascii_case("true"));
    let has_prefix = |prefixes: &[&str]| {
        props.keys().any(|k| {
            k.strip_prefix("crs:").is_some_and(|n| prefixes.iter().any(|p| n.starts_with(p))) && num(props, k).is_some_and(|v| v.abs() > 1e-6)
        })
    };
    let mut g = Vec::new();
    if first(props, "crs:WhiteBalance").is_some_and(|wb| !wb.eq_ignore_ascii_case("As Shot")) {
        g.push("white balance");
    }
    if any(&["Exposure2012", "Contrast2012", "Highlights2012", "Shadows2012", "Whites2012", "Blacks2012"]) {
        g.push("light");
    }
    let curve_bent = ["crs:ToneCurvePV2012", "crs:ToneCurvePV2012Red", "crs:ToneCurvePV2012Green", "crs:ToneCurvePV2012Blue"].iter().any(|k| {
        props.get(*k).is_some_and(|items| {
            !items.iter().all(|it| {
                it.split_once(',')
                    .is_some_and(|(x, y)| matches!((x.trim().parse::<f64>(), y.trim().parse::<f64>()), (Ok(x), Ok(y)) if (x - y).abs() < 0.5))
            })
        })
    });
    if curve_bent || any(&["ParametricShadows", "ParametricDarks", "ParametricLights", "ParametricHighlights"]) {
        g.push("tone curve");
    }
    if any(&["Texture", "Clarity2012", "Dehaze", "Vibrance", "Saturation"]) {
        g.push("presence");
    }
    if has_prefix(HSL_PREFIX) {
        g.push("colour mixer");
    }
    if any(&[
        "SplitToningShadowSaturation",
        "SplitToningHighlightSaturation",
        "ColorGradeMidtoneSat",
        "ColorGradeShadowSat",
        "ColorGradeHighlightSat",
        "ColorGradeGlobalSat",
    ]) {
        g.push("colour grading");
    }
    if any(&["ShadowTint", "RedHue", "RedSaturation", "GreenHue", "GreenSaturation", "BlueHue", "BlueSaturation"]) {
        g.push("calibration");
    }
    if look_name(props).is_some_and(|n| !matches!(n.trim(), "Adobe Standard" | "Adobe Color")) {
        g.push("profile");
    }
    if yes("ConvertToGrayscale") {
        g.push("black & white");
    }
    if any(&["VignetteAmount", "PostCropVignetteAmount", "GrainAmount"]) {
        g.push("effects");
    }
    if yes("LensProfileEnable") || nonzero("LensProfileEnable") || nonzero("LensManualDistortionAmount") {
        g.push("lens");
    }
    if yes("HasCrop") {
        g.push("crop");
    }
    const MASKS: &[&str] = &["MaskGroupBasedCorrections", "GradientBasedCorrections", "CircularGradientBasedCorrections", "PaintBasedCorrections"];
    if props.keys().any(|k| MASKS.iter().any(|m| k.starts_with(&format!("crs:{m}")))) {
        g.push("masks");
    }
    if g.is_empty() {
        g.push("default");
    }
    g
}

fn same_model(a: &str, b: &str) -> bool {
    let n = |s: &str| s.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect::<String>();
    let (a, b) = (n(a), n(b));
    !a.is_empty() && !b.is_empty() && (a == b || a.contains(&b) || b.contains(&a))
}

fn same_time(a: TimeKey, b: TimeKey) -> bool {
    let (sa, sb) = ((a.0, a.1, a.2, a.3, a.4, a.5), (b.0, b.1, b.2, b.3, b.4, b.5));
    sa == sb && (a.6 == b.6 || a.6 == 0 || b.6 == 0)
}

fn stem(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// Export stem `e` names raw stem `r`: equal, or `r` followed by a separator (`IMG_1-2`, `IMG_1 copy`).
fn stem_matches(r: &str, e: &str) -> bool {
    !r.is_empty() && (e == r || e.strip_prefix(r).is_some_and(|rest| rest.starts_with(|c: char| !c.is_alphanumeric())))
}

/// Pair each export with one raw: same model + capture time (stem breaks ties), else the stem alone.
pub fn pair(raws: &[RawFile], exports: &[Export], skipped: &mut Vec<(String, String)>) -> Vec<Pair> {
    let mut out = Vec::new();
    for (ei, e) in exports.iter().enumerate() {
        let es = stem(&e.path);
        let by_time: Vec<usize> = match e.time_key {
            Some(et) => raws
                .iter()
                .enumerate()
                .filter(|(_, r)| r.time_key.is_some_and(|rt| same_time(rt, et)) && (e.model.is_empty() || same_model(&r.model, &e.model)))
                .map(|(i, _)| i)
                .collect(),
            None => Vec::new(),
        };
        let by_stem = |cands: &mut dyn Iterator<Item = usize>| {
            cands.filter(|&i| raws.get(i).is_some_and(|r| stem_matches(&stem(&r.path), &es))).collect::<Vec<_>>()
        };
        let pick = match by_time.len() {
            1 => by_time.first().copied(),
            0 => {
                let s = by_stem(&mut (0..raws.len()));
                if s.len() == 1 { s.first().copied() } else { None }
            }
            _ => {
                let s = by_stem(&mut by_time.iter().copied());
                if s.len() == 1 { s.first().copied() } else { None }
            }
        };
        match pick {
            Some(raw) => out.push(Pair { raw, export: ei }),
            None => skipped.push((file_name(&e.path), "no matching raw (camera model and capture time, or file name)".into())),
        }
    }
    out
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_creative_profile_is_named_by_its_look_not_the_camera_profile() {
        let mut p: BTreeMap<String, Vec<String>> = BTreeMap::new();
        p.insert("crs:CameraProfile".into(), vec!["Adobe Standard".into()]);
        assert_eq!(look_name(&p).as_deref(), Some("Adobe Standard"));
        p.insert("crs:Look/crs:Name".into(), vec!["Adobe Color".into()]);
        p.insert("crs:Look/crs:Amount".into(), vec!["1".into()]);
        assert_eq!(look_name(&p).as_deref(), Some("Adobe Color"));
        assert_eq!(baseline_problem(&p), None);
        p.insert("crs:Look/crs:Amount".into(), vec!["0.5".into()]);
        assert!(baseline_problem(&p).is_some_and(|w| w.contains("50 %")));
    }

    #[test]
    fn export_folders_are_searched_with_their_subfolders_once() {
        let root = std::env::temp_dir().join(format!("colourfit-nested-{}", std::process::id()));
        let deep = root.join("Adobe Color").join("day 1");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("IMG_1.tif"), b"not an image").unwrap();
        std::fs::write(root.join("notes.txt"), b"ignored").unwrap();
        // the root and a subfolder of it: the file is found (and skipped as damaged) once
        let sets = [ExportSet { dir: root.clone() }, ExportSet { dir: root.join("Adobe Color") }, ExportSet { dir: root.join("missing") }];
        let mut skipped = Vec::new();
        let found = export_files(&sets, &mut skipped);
        assert!(found.is_empty());
        assert_eq!(skipped.iter().filter(|(n, _)| n == "IMG_1.tif").count(), 1, "{skipped:?}");
        assert!(skipped.iter().any(|(n, why)| n == "missing" && why == "not a folder"));
        let _ = std::fs::remove_dir_all(&root);
    }

    use super::*;

    fn props(kv: &[(&str, &str)]) -> BTreeMap<String, Vec<String>> {
        kv.iter().map(|(k, v)| (format!("crs:{k}"), vec![v.to_string()])).collect()
    }

    #[test]
    fn edits_are_grouped_by_the_controls_they_use() {
        assert_eq!(edited_groups(&props(&[("Exposure2012", "0"), ("WhiteBalance", "As Shot")])), vec!["default"]);
        let g = edited_groups(&props(&[("Exposure2012", "+0.5"), ("HueAdjustmentRed", "-10"), ("WhiteBalance", "Custom"), ("HasCrop", "True")]));
        for want in ["light", "colour mixer", "white balance", "crop"] {
            assert!(g.contains(&want), "{want} in {g:?}");
        }
        assert!(!g.contains(&"default"));
        let mut p = props(&[]);
        p.insert("crs:ToneCurvePV2012".into(), vec!["0, 0".into(), "128, 150".into(), "255, 255".into()]);
        assert_eq!(edited_groups(&p), vec!["tone curve"]);
        // junk values are not edits
        assert_eq!(edited_groups(&props(&[("Contrast2012", "NaN"), ("Texture", "abc")])), vec!["default"]);
    }

    #[test]
    fn baseline_checks() {
        assert!(baseline_problem(&props(&[("Exposure2012", "0.00"), ("WhiteBalance", "As Shot"), ("Sharpness", "40")])).is_none());
        assert!(baseline_problem(&props(&[("Exposure2012", "+0.65")])).is_some());
        assert!(baseline_problem(&props(&[("HasCrop", "True")])).is_some());
        assert!(baseline_problem(&props(&[("WhiteBalance", "Custom")])).is_some());
        assert!(baseline_problem(&props(&[("HueAdjustmentRed", "-5")])).is_some());
        let mut p = BTreeMap::new();
        p.insert("crs:ToneCurvePV2012".to_string(), vec!["0, 0".to_string(), "64, 52".to_string(), "255, 255".to_string()]);
        assert!(baseline_problem(&p).is_some());
        p.insert("crs:ToneCurvePV2012".to_string(), vec!["0, 0".to_string(), "255, 255".to_string()]);
        assert!(baseline_problem(&p).is_none());
        // garbage values are not adjustments
        assert!(baseline_problem(&props(&[("Contrast2012", "NaN"), ("Clarity2012", "abc")])).is_none());
    }

    fn raw(path: &str, model: &str, t: Option<TimeKey>) -> RawFile {
        RawFile { path: path.into(), make: "Make".into(), model: model.into(), time_key: t }
    }
    fn export(path: &str, model: &str, t: Option<TimeKey>) -> Export {
        Export { path: path.into(), look: "L".into(), temp_tint: None, model: model.into(), time_key: t }
    }

    #[test]
    fn pairs_by_time_then_stem() {
        let t1 = Some((2026, 1, 2, 3, 4, 5, 0));
        let t2 = Some((2026, 1, 2, 3, 4, 6, 120));
        let raws = [raw("a/IMG_1.CR3", "EOS R5", t1), raw("a/IMG_2.CR3", "EOS R5", t2), raw("a/IMG_3.CR3", "EOS R5", t1)];
        let exports = [
            export("x/renamed.tif", "Canon EOS R5", Some((2026, 1, 2, 3, 4, 6, 0))), // time → IMG_2
            export("x/IMG_3-2.tif", "Canon EOS R5", t1),                             // time tie → stem
            export("x/IMG_10.tif", "", None),                                        // no match
            export("x/IMG_1.jpg", "", None),                                         // stem
        ];
        let mut sk = Vec::new();
        let p = pair(&raws, &exports, &mut sk);
        let got: Vec<(usize, usize)> = p.iter().map(|p| (p.export, p.raw)).collect();
        assert_eq!(got, vec![(0, 1), (1, 2), (3, 0)]);
        assert_eq!(sk.len(), 1);
    }

    #[test]
    fn walk_bounds_and_missing_dirs() {
        let mut out = Vec::new();
        let mut sk = Vec::new();
        walk(Path::new("/definitely/not/here"), 0, &mut out, &mut sk);
        assert!(out.is_empty() && sk.len() == 1);
    }
}
