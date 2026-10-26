//! Migration check: how close the app's render of an *edited* photo gets to the edited export from the editor it came
//! from. Each raw is imported the way a person would bring it over, so the app reads its XMP sidecar. If the export
//! carries its own `crs:` settings (Lightroom writes them into exports that include all metadata), those settings
//! are applied instead, because they are exactly what was exported. The photo is then rendered in the export's own
//! colour space, so both are clipped to one gamut, and compared against the export with [`crate::compare`]. Results
//! are grouped by the controls the edit used, so a group that stands out points at the import mapping to fix.
//!
//! Sources: own design.
use crate::compare::{self, CompareOptions, Stats};
use crate::dataset::{self, ExportSet};
use crate::deltae::quantile;
use crate::{Error, Result};
use lcc_engine::catalog::PhotoId;
use serde::Serialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Most photos checked in one run (each is a full raw render).
pub const MAX_PHOTOS: usize = 2000;

#[derive(Clone, Debug)]
pub struct VerifyOptions {
    pub compare: CompareOptions,
    /// Long edge of the app's render (pixels).
    pub render_edge: u32,
    /// Stop after this many photos (0 = all, at most [`MAX_PHOTOS`]).
    pub max_photos: usize,
}

impl Default for VerifyOptions {
    fn default() -> Self {
        VerifyOptions { compare: CompareOptions::default(), render_edge: 1024, max_photos: 0 }
    }
}

/// Where the edit the app rendered came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingsFrom {
    /// The `crs:` settings embedded in the export.
    Export,
    /// The raw's XMP sidecar (read on import).
    Sidecar,
    /// Neither: the app's defaults (the export is compared as a default render).
    Defaults,
}

#[derive(Clone, Debug, Serialize)]
pub struct PhotoResult {
    pub raw: String,
    pub export: String,
    pub settings_from: SettingsFrom,
    /// The colour space the export (and so the app's render) was written in.
    pub space: &'static str,
    /// Groups of controls the edit used ([`dataset::edited_groups`]).
    pub groups: Vec<&'static str>,
    pub stats: Stats,
    /// The full comparison, for maps.
    #[serde(skip)]
    pub report: Option<compare::CompareReport>,
}

/// Per-photo median ΔE00 summarised over the photos of one group.
#[derive(Clone, Debug, Serialize)]
pub struct GroupResult {
    pub name: String,
    pub photos: usize,
    /// Median over the photos of their median ΔE00.
    pub median: f64,
    /// 90th percentile over the photos of their median ΔE00.
    pub p90: f64,
    /// Mean over the photos of their mean lightness / chroma bias.
    pub lightness_bias: f64,
    pub chroma_bias: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct VerifyReport {
    pub photos: Vec<PhotoResult>,
    /// Every photo checked.
    pub overall: Option<GroupResult>,
    /// One row per control group, worst first.
    pub groups: Vec<GroupResult>,
    /// (file name, why it was not checked).
    pub skipped: Vec<(String, String)>,
}

fn summarise(name: &str, photos: &[&PhotoResult]) -> Option<GroupResult> {
    if photos.is_empty() {
        return None;
    }
    let med: Vec<f64> = photos.iter().map(|p| p.stats.median).collect();
    let n = photos.len() as f64;
    Some(GroupResult {
        name: name.to_string(),
        photos: photos.len(),
        median: quantile(&med, 0.5),
        p90: quantile(&med, 0.9),
        lightness_bias: photos.iter().map(|p| p.stats.lightness_bias).sum::<f64>() / n,
        chroma_bias: photos.iter().map(|p| p.stats.chroma_bias).sum::<f64>() / n,
    })
}

/// The `crs:` properties of an encoded file's embedded XMP (empty when it has none).
fn crs_props(bytes: &[u8]) -> (Option<String>, lcc_engine::crs::Props) {
    let xmp = lcc_meta::embedded(bytes).xmp;
    let props = xmp.as_deref().and_then(|x| lcc_meta::parse_xmp(x).ok()).map(|d| d.properties).unwrap_or_default();
    (xmp, props)
}

/// The raw's sidecar `crs:` properties (`<stem>.xmp` or `<name>.xmp`), if it has one.
fn sidecar_props(raw: &Path) -> Option<lcc_engine::crs::Props> {
    let candidates = [raw.with_extension("xmp"), raw.with_extension("XMP"), PathBuf::from(format!("{}.xmp", raw.display()))];
    candidates.iter().find_map(|p| {
        let len = std::fs::metadata(p).ok()?.len();
        if len > 64 << 20 {
            return None;
        }
        let text = std::fs::read_to_string(p).ok()?;
        lcc_meta::parse_xmp(&text).ok().map(|d| d.properties)
    })
}

/// The export option id of the colour space the export was written in (ProPhoto when unknown), so the app's render
/// is clipped to the same gamut as the export.
fn export_space(bytes: &[u8]) -> &'static str {
    use lcc_codecs::NamedSpace;
    let named = lcc_codecs::decode(bytes, lcc_codecs::DecodeOptions::fit(64, 64)).ok().and_then(|d| d.space.named);
    match named {
        Some(NamedSpace::Srgb) => "srgb",
        Some(NamedSpace::DisplayP3) => "displayP3",
        Some(NamedSpace::AdobeRgb) => "adobeRgb",
        Some(NamedSpace::Rec2020) => "rec2020",
        Some(NamedSpace::ProPhoto) | None => "proPhoto",
    }
}

/// Import `raw`, apply the export's settings if it carries them, render a 16-bit TIFF in `space`.
fn render_ours(raw: &Path, export_xmp: Option<&str>, space: &str, edge: u32) -> std::result::Result<(Vec<u8>, bool), String> {
    use lcc_engine::export::{ExportOptions, export_photo};
    let mut s = lcc_engine::Session::new().with_fs();
    let abs = std::fs::canonicalize(raw).unwrap_or_else(|_| raw.to_path_buf());
    let r = s.execute("library.import", &json!({"paths": [abs.display().to_string()]})).map_err(|e| format!("import failed: {e}"))?;
    let id = r["imported"].get(0).and_then(serde_json::Value::as_u64).ok_or("not a readable raw")?;
    let id = PhotoId(id);
    let mut applied = false;
    if let Some(xmp) = export_xmp {
        let photo = s.catalog.photo(id).ok_or("the imported photo is missing")?;
        let target = lcc_engine::crs::Target::for_photo(photo);
        let sc = lcc_engine::sidecar::parse_sidecar(xmp, target).map_err(|e| format!("the export's settings: {e}"))?;
        let op = s.sidecar_op(id, sc).map_err(|e| e.to_string())?;
        s.commit("Settings from the export", op).map_err(|e| e.to_string())?;
        applied = true;
    }
    let p = json!({"format": "tiff", "bitDepth": 16, "colorSpace": space, "longEdge": edge, "metadata": "none"});
    let o = ExportOptions::from_params(&p).map_err(|e| e.to_string())?;
    let e = export_photo(&mut s, id, &o, 1)?;
    Ok((e.bytes, applied))
}

/// Check every raw/export pair under `raws` and `exports`. `progress` is called before each photo with
/// (done, total, raw file name).
pub fn verify(raws: &[PathBuf], exports: &[ExportSet], opts: &VerifyOptions, progress: &mut dyn FnMut(usize, usize, &str)) -> Result<VerifyReport> {
    let (mut pairs, mut skipped) = dataset::pair_files(raws, exports, false)?;
    let cap = if opts.max_photos == 0 { MAX_PHOTOS } else { opts.max_photos.min(MAX_PHOTOS) };
    if pairs.len() > cap {
        skipped.push((format!("{} more pairs", pairs.len() - cap), format!("only the first {cap} are checked")));
        pairs.truncate(cap);
    }
    let edge = opts.render_edge.clamp(64, 8192);
    let mut photos = Vec::new();
    let total = pairs.len();
    for (i, (raw, export)) in pairs.iter().enumerate() {
        let (raw_name, export_name) = (dataset::file_name(raw), dataset::file_name(export));
        progress(i, total, &raw_name);
        let theirs = match dataset::read_file(export) {
            Ok(b) => b,
            Err(e) => {
                skipped.push((export_name, e));
                continue;
            }
        };
        let (xmp, props) = crs_props(&theirs);
        let space = export_space(&theirs);
        let embedded = lcc_engine::crs::has_adjustments(&props) || props.keys().any(|k| k.starts_with("crs:"));
        let side = if embedded { None } else { sidecar_props(raw) };
        let (ours, applied) = match render_ours(raw, xmp.as_deref().filter(|_| embedded), space, edge) {
            Ok(r) => r,
            Err(e) => {
                skipped.push((raw_name, e));
                continue;
            }
        };
        let settings_from = match (applied, &side) {
            (true, _) => SettingsFrom::Export,
            (false, Some(_)) => SettingsFrom::Sidecar,
            (false, None) => SettingsFrom::Defaults,
        };
        let groups = dataset::edited_groups(if embedded { &props } else { side.as_ref().unwrap_or(&props) });
        match compare::compare_bytes((&export_name, &theirs), ("the app's render", &ours), &opts.compare) {
            Ok(rep) => photos.push(PhotoResult {
                raw: raw_name,
                export: export_name,
                settings_from,
                space,
                groups,
                stats: rep.overall.clone(),
                report: Some(rep),
            }),
            Err(e) => skipped.push((export_name, e.0)),
        }
    }
    progress(total, total, "");
    if photos.is_empty() {
        let why = skipped.first().map(|(f, w)| format!(" (first: {f}: {w})")).unwrap_or_default();
        return Err(Error(format!("no photo could be checked{why}")));
    }
    let all: Vec<&PhotoResult> = photos.iter().collect();
    let overall = summarise("all photos", &all);
    let mut by: BTreeMap<&'static str, Vec<&PhotoResult>> = BTreeMap::new();
    for p in &photos {
        for g in &p.groups {
            by.entry(g).or_default().push(p);
        }
    }
    let mut groups: Vec<GroupResult> = by.iter().filter_map(|(name, ps)| summarise(name, ps)).collect();
    groups.sort_by(|a, b| b.median.total_cmp(&a.median));
    Ok(VerifyReport { photos, overall, groups, skipped })
}

/// Plain-text summary for the terminal.
pub fn text(r: &VerifyReport) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let _ = writeln!(s, "{:<40} {:>8} {:>6} {:>6}   {:>6} {:>6}  edited", "photo", "settings", "ΔE p50", "p95", "ΔL*", "ΔC*");
    for p in &r.photos {
        let from = match p.settings_from {
            SettingsFrom::Export => "export",
            SettingsFrom::Sidecar => "sidecar",
            SettingsFrom::Defaults => "default",
        };
        let name: String = p.raw.chars().take(40).collect();
        let _ = writeln!(
            s,
            "{:<40} {:>8} {:>6.2} {:>6.2}   {:>+6.2} {:>+6.2}  {}",
            name,
            from,
            p.stats.median,
            p.stats.p95,
            p.stats.lightness_bias,
            p.stats.chroma_bias,
            p.groups.join(", ")
        );
    }
    let _ = writeln!(s, "\n{:<20} {:>6} {:>8} {:>8}   {:>6} {:>6}", "edited with", "photos", "ΔE p50", "ΔE p90", "ΔL*", "ΔC*");
    for g in r.overall.iter().chain(&r.groups) {
        let _ = writeln!(s, "{:<20} {:>6} {:>8.2} {:>8.2}   {:>+6.2} {:>+6.2}", g.name, g.photos, g.median, g.p90, g.lightness_bias, g.chroma_bias);
    }
    if !r.skipped.is_empty() {
        let _ = writeln!(s, "\nnot checked ({}):", r.skipped.len());
        for (f, why) in r.skipped.iter().take(20) {
            let _ = writeln!(s, "  {f}: {why}");
        }
    }
    s.push_str("\nΔE: per-photo CIEDE2000 of the app's render against the export · ΔL* > 0: the app's render is lighter · ΔC* > 0: more saturated\n");
    s
}
