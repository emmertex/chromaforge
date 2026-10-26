//! Pairing raws with Lightroom exports, decoding, registration and sampling.
//!
//! Sources: own design (pairing by camera model and capture time; registration in [`register`]; sampling in
//! [`sample`]).
pub(crate) mod grid;
mod pair;
pub(crate) mod register;
mod sample;

use crate::{Error, Result};
use grid::{Grid, fit_long, reduce_rgb};
use pair::{Export, Pair, RawFile};
pub use pair::{baseline_problem, edited_groups};
use rayon::prelude::*;
use std::path::{Path, PathBuf};

/// A folder of Lightroom exports, searched with its subfolders. Each export's look is its `crs:CameraProfile`, else
/// the name of the folder it sits in (so one root with a subfolder per profile works).
#[derive(Clone, Debug)]
pub struct ExportSet {
    pub dir: PathBuf,
}

/// What `fit` needs about one photo's white balance.
#[derive(Clone, Debug)]
pub struct White {
    /// Camera neutral (AsShotNeutral, or 1 / multipliers), green-normalised.
    pub neutral: [f64; 3],
    /// Lightroom's temperature and tint for this export (crs:Temperature / crs:Tint), when the export carries it.
    pub lightroom_temp_tint: Option<(f64, f64)>,
    /// File's baseline exposure (EV).
    pub baseline_exposure: f64,
}

/// One colour pair: our camera RGB (black-subtracted, white = 1, not white-balanced, no matrix) and Lightroom's
/// colour at the same place as linear XYZ D50 (decoded from the export's ICC profile).
#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub camera: [f32; 3],
    pub lightroom_xyz: [f32; 3],
    /// Weight (bin balancing).
    pub weight: f32,
    /// Where the sample sits: normalised `[x, y]` (0..1, cell centre) in our developed frame (default crop, EXIF
    /// orientation), the frame the app's decode renders, so the fit can re-render the raw and read the same place.
    pub pos: [f32; 2],
}

/// The raw file's own colour tags (DNG `ColorMatrix1/2` / `ForwardMatrix1/2`, or a decoder's equivalent), the
/// fit's first choice of prior.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DngMatrices {
    /// `CalibrationIlluminant1/2` (Exif LightSource codes).
    pub illuminant: [u16; 2],
    pub color_matrix: [Option<lcc_color::Mat3>; 2],
    pub forward_matrix: [Option<lcc_color::Mat3>; 2],
}

/// One raw paired with one export of one look.
#[derive(Clone, Debug)]
pub struct PhotoSamples {
    /// Stable anonymous id within this run (index), never a file name.
    pub photo: usize,
    /// `crs:CameraProfile` name of the export, e.g. "Adobe Standard".
    pub look: String,
    pub make: String,
    pub model: String,
    pub white: White,
    pub samples: Vec<Sample>,
    /// Held out for validation (not fitted on).
    pub held_out: bool,
    /// Path of the raw (local only: report and closed loop; never written to the calibration).
    pub raw_path: PathBuf,
    /// Path of the export (local only).
    pub export_path: PathBuf,
    /// The raw's own colour matrices, when it has any.
    pub dng: Option<DngMatrices>,
}

/// Options for collecting.
#[derive(Clone, Debug)]
pub struct CollectOptions {
    /// Long edge of the sampling proxies (default 256).
    pub proxy: usize,
    /// Share of photos held out (default 0.2).
    pub hold_out: f64,
    /// Cap of samples per photo (default 3000).
    pub per_photo: usize,
}

impl Default for CollectOptions {
    fn default() -> Self {
        CollectOptions { proxy: 256, hold_out: 0.2, per_photo: 3000 }
    }
}

/// What collection did, for the console (counts and reasons, never file contents).
#[derive(Clone, Debug, Default)]
pub struct CollectReport {
    pub paired: usize,
    /// (file name, why skipped): printed locally only.
    pub skipped: Vec<(String, String)>,
}

/// Largest file read (bytes): a raw or a 16-bit full-size TIFF stays well below this.
const MAX_FILE: u64 = 2 << 30;
/// Weakest registration accepted (normalised cross-correlation of high-pass log luminance).
const MIN_SCORE: f32 = 0.35;
/// Largest aspect-ratio mismatch between our develop and the export.
const MAX_ASPECT: f64 = 0.03;

pub(crate) fn read_file(p: &Path) -> std::result::Result<Vec<u8>, String> {
    let len = std::fs::metadata(p).map_err(|e| format!("cannot read: {e}"))?.len();
    if len > MAX_FILE {
        return Err("file too large".into());
    }
    std::fs::read(p).map_err(|e| format!("cannot read: {e}"))
}

pub(crate) fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

/// Our side of one raw at the registration grid: camera RGB, per-cell max channel, white.
struct Ours {
    cam: Grid<[f32; 3]>,
    cam_max: Grid<f32>,
    white: White,
    make: String,
    model: String,
    dng: Option<DngMatrices>,
}

/// Decode, develop (camera RGB, no white balance, no matrix), orient and reduce a raw to long edge `reg`.
fn develop_ours(path: &Path, reg: usize) -> std::result::Result<Ours, String> {
    let bytes = read_file(path)?;
    let raw = lcc_raw::decode(&bytes).map_err(|e| format!("raw decode failed: {e}"))?;
    drop(bytes);
    let long = raw.width.max(raw.height);
    // bin to about twice the registration grid (fast), else demosaic
    let k = ((long / reg.saturating_mul(2).max(1)) & !1).max(2);
    let img = match raw.develop_binned(k, 1.0) {
        Ok(Some(img)) => img,
        _ => raw.develop(lcc_raw::Method::Bilinear).map_err(|e| format!("raw develop failed: {e}"))?,
    };
    let img = img.into_oriented(raw.orientation);
    if img.width < 16 || img.height < 16 {
        return Err("raw too small".into());
    }
    let src = Grid { w: img.width, h: img.height, data: img.data };
    let (w, h) = fit_long(src.w, src.h, reg);
    let (cam, cam_max) = reduce_rgb(&src, w, h, |p| p[0].max(p[1]).max(p[2]));
    let white = white_of(&raw.color, raw.wb_multipliers);
    let m = &raw.metadata;
    let c = &raw.color;
    let dng = c.color_matrix.iter().any(Option::is_some).then_some(DngMatrices {
        illuminant: c.illuminant,
        color_matrix: c.color_matrix,
        forward_matrix: c.forward_matrix,
    });
    Ok(Ours {
        dng,
        cam,
        cam_max,
        white,
        make: m.make.clone().unwrap_or_default().trim().to_string(),
        model: m.model.clone().unwrap_or_default().trim().to_string(),
    })
}

/// The camera neutral, green-normalised: `AsShotNeutral`, else 1 / vendor multipliers, else the neutral of the
/// as-shot white.
fn white_of(c: &lcc_raw::ColorData, wb: Option<[f32; 3]>) -> White {
    let norm =
        |n: [f64; 3]| -> Option<[f64; 3]> { (n.iter().all(|v| v.is_finite() && *v > 0.0) && n[1] > 0.0).then(|| [n[0] / n[1], 1.0, n[2] / n[1]]) };
    let neutral = c
        .as_shot_neutral
        .and_then(norm)
        .or_else(|| wb.and_then(|m| norm([1.0 / m[0] as f64, 1.0 / m[1] as f64, 1.0 / m[2] as f64])))
        .or_else(|| norm(lcc_raw::color::camera_neutral(c, lcc_raw::color::as_shot_white_of(c, wb))))
        .unwrap_or([1.0; 3]);
    let be = if c.baseline_exposure.is_finite() { c.baseline_exposure } else { 0.0 };
    White { neutral, lightroom_temp_tint: None, baseline_exposure: be }
}

/// Decode an export to linear XYZ D50, oriented, reduced to width `w`.
fn decode_theirs(path: &Path, w: usize) -> std::result::Result<Grid<[f32; 3]>, String> {
    let bytes = read_file(path)?;
    let box_edge = u32::try_from(w.saturating_mul(4)).unwrap_or(u32::MAX);
    let d = lcc_codecs::decode(&bytes, lcc_codecs::DecodeOptions::fit(box_edge, box_edge)).map_err(|e| format!("export decode failed: {e}"))?;
    drop(bytes);
    let m = d.space.to_xyz_d50;
    let img = d.image.into_oriented(lcc_raw::Orientation::from_exif(d.orientation));
    if img.width < 16 || img.height < 16 {
        return Err("export too small".into());
    }
    let src = Grid { w: img.width, h: img.height, data: img.data.iter().map(|p| m.apply_f32(*p)).collect() };
    let h = ((src.h as f64 * w as f64 / src.w as f64).round() as usize).max(1);
    Ok(reduce_rgb(&src, w, h, |p| p[1]).0)
}

/// Register and sample one export against our develop.
fn pair_samples(ours: &Ours, export: &Path, per_photo: usize) -> std::result::Result<Vec<Sample>, String> {
    let theirs = decode_theirs(export, ours.cam.w)?;
    let (ao, ae) = (ours.cam.h as f64 / ours.cam.w as f64, theirs.h as f64 / theirs.w as f64);
    if (ao / ae - 1.0).abs() > MAX_ASPECT {
        return Err(format!("aspect ratio differs from the raw ({:.3} vs {:.3}): cropped or rotated export?", 1.0 / ae, 1.0 / ao));
    }
    let la = register::feature(&ours.cam.map(|p| 0.25 * p[0] + 0.5 * p[1] + 0.25 * p[2]));
    let lb = register::feature(&theirs.map(|p| p[1]));
    let t = register::register(&la, &lb, 0.04, 0.04);
    if t.score < MIN_SCORE {
        return Err(format!("could not register the export with the raw (correlation {:.2})", t.score));
    }
    let s = sample::sample(&ours.cam, &ours.cam_max, &theirs, &t, per_photo);
    if s.is_empty() {
        return Err("no usable colour samples (all clipped, black or textured)".into());
    }
    Ok(s)
}

/// Exactly one camera model among `models` (make + model), else an error naming them.
fn single_model(models: &[(String, String)]) -> Result<()> {
    let mut seen: Vec<String> = models.iter().map(|(mk, md)| format!("{mk} {md}").trim().to_string()).collect();
    seen.sort_by_key(|a| a.to_lowercase());
    seen.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    if seen.len() > 1 {
        return Err(Error(format!("one camera model per run, found {}: {}", seen.len(), seen.join(", "))));
    }
    Ok(())
}

/// Hold out every `round(1 / share)`-th raw (by capture time), never all of them.
fn held_out(order: usize, n: usize, share: f64) -> bool {
    if !(share.is_finite() && share > 0.0) || n < 2 {
        return false;
    }
    let every = ((1.0 / share.min(0.5)).round() as usize).max(2);
    order % every == every - 1
}

/// Raws paired with exports of them (same camera model and capture time, else file name), without decoding:
/// `(raw, export)` paths. `baseline` refuses exports that are not at the baseline preset. Errors only when
/// nothing pairs; the second value lists what was skipped and why.
pub fn pair_files(raws: &[PathBuf], exports: &[ExportSet], baseline: bool) -> Result<(Vec<(PathBuf, PathBuf)>, Vec<(String, String)>)> {
    let mut skipped = Vec::new();
    let raw_files: Vec<RawFile> = pair::raw_files(raws, &mut skipped);
    if raw_files.is_empty() {
        return Err(Error("no raw files found".into()));
    }
    let export_files: Vec<Export> = pair::export_files_with(exports, baseline, &mut skipped);
    if export_files.is_empty() {
        return Err(Error("no usable exports found (TIFF or JPEG)".into()));
    }
    let pairs = pair::pair(&raw_files, &export_files, &mut skipped);
    let out: Vec<(PathBuf, PathBuf)> =
        pairs.iter().filter_map(|p| Some((raw_files.get(p.raw)?.path.clone(), export_files.get(p.export)?.path.clone()))).collect();
    if out.is_empty() {
        return Err(Error("no export could be matched to a raw (same camera model and capture time, or file name)".into()));
    }
    Ok((out, skipped))
}

/// Pair, decode, register and sample. Errors only when nothing usable is found.
pub fn collect(raws: &[PathBuf], exports: &[ExportSet], opts: &CollectOptions) -> Result<(Vec<PhotoSamples>, CollectReport)> {
    let proxy = opts.proxy.clamp(32, 2048);
    let reg = proxy * 2;
    let per_photo = opts.per_photo.clamp(1, 1_000_000);
    let mut report = CollectReport::default();

    let raw_files: Vec<RawFile> = pair::raw_files(raws, &mut report.skipped);
    if raw_files.is_empty() {
        return Err(Error("no raw files found".into()));
    }
    let export_files: Vec<Export> = pair::export_files(exports, &mut report.skipped);
    if export_files.is_empty() {
        return Err(Error("no usable exports found (16-bit TIFF or JPEG, at the baseline preset)".into()));
    }
    let pairs: Vec<Pair> = pair::pair(&raw_files, &export_files, &mut report.skipped);
    if pairs.is_empty() {
        return Err(Error("no export could be matched to a raw (same camera model and capture time, or file name)".into()));
    }
    // one model per run, over the raws that were paired
    let mut used: Vec<usize> = pairs.iter().map(|p| p.raw).collect();
    used.sort_unstable();
    used.dedup();
    single_model(&used.iter().filter_map(|&i| raw_files.get(i)).map(|r| (r.make.clone(), r.model.clone())).collect::<Vec<_>>())?;

    // hold-out order: capture time, then path
    let mut order = used.clone();
    order.sort_by(|a, b| {
        let (ra, rb) = (raw_files.get(*a), raw_files.get(*b));
        ra.map(|r| (r.time_key, &r.path)).cmp(&rb.map(|r| (r.time_key, &r.path)))
    });
    let n_raws = order.len();

    let results: Vec<(Vec<PhotoSamples>, Vec<(String, String)>)> = order
        .par_iter()
        .enumerate()
        .map(|(rank, &ri)| {
            let mut out = Vec::new();
            let mut skipped = Vec::new();
            let Some(rf) = raw_files.get(ri) else { return (out, skipped) };
            let mine: Vec<&Pair> = pairs.iter().filter(|p| p.raw == ri).collect();
            let ours = match develop_ours(&rf.path, reg) {
                Ok(o) => o,
                Err(e) => {
                    skipped.push((file_name(&rf.path), e));
                    return (out, skipped);
                }
            };
            for p in mine {
                let Some(ex) = export_files.get(p.export) else { continue };
                match pair_samples(&ours, &ex.path, per_photo) {
                    Ok(samples) => out.push(PhotoSamples {
                        photo: ri,
                        look: ex.look.clone(),
                        make: if ours.make.is_empty() { rf.make.clone() } else { ours.make.clone() },
                        model: if ours.model.is_empty() { rf.model.clone() } else { ours.model.clone() },
                        white: White { lightroom_temp_tint: ex.temp_tint, ..ours.white.clone() },
                        samples,
                        held_out: held_out(rank, n_raws, opts.hold_out),
                        raw_path: rf.path.clone(),
                        export_path: ex.path.clone(),
                        dng: ours.dng,
                    }),
                    Err(e) => skipped.push((file_name(&ex.path), e)),
                }
            }
            (out, skipped)
        })
        .collect();
    let mut photos = Vec::new();
    for (p, s) in results {
        photos.extend(p);
        report.skipped.extend(s);
    }
    report.paired = photos.len();
    if photos.is_empty() {
        let why = report.skipped.first().map(|(f, w)| format!(" (first: {f}: {w})")).unwrap_or_default();
        return Err(Error(format!("no usable raw/export pairs{why}")));
    }
    // a run with only held-out photos fits nothing: train on them all instead
    if photos.iter().all(|p| p.held_out) {
        for p in &mut photos {
            p.held_out = false;
        }
    }
    Ok((photos, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("colourfit-dataset-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn empty_dirs_are_errors() {
        let d = scratch("empty");
        let r = collect(std::slice::from_ref(&d), &[ExportSet { dir: d.clone() }], &CollectOptions::default());
        assert!(r.is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn corrupt_files_are_skipped_not_panics() {
        let d = scratch("corrupt");
        let raws = d.join("raws");
        let ex = d.join("Adobe Color");
        std::fs::create_dir_all(&raws).unwrap();
        std::fs::create_dir_all(&ex).unwrap();
        std::fs::write(raws.join("IMG_0001.CR2"), b"II*\0garbage").unwrap();
        std::fs::write(raws.join("IMG_0002.NEF"), []).unwrap();
        std::fs::write(ex.join("IMG_0001.tif"), b"II*\0\x08\0\0\0\xff\xff").unwrap();
        std::fs::write(ex.join("IMG_0002.jpg"), b"\xff\xd8\xff\xe0 not a jpeg").unwrap();
        let r = collect(&[raws], &[ExportSet { dir: ex }], &CollectOptions::default());
        let Err(e) = r else { panic!("expected an error") };
        assert!(!e.0.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn mixed_models_are_an_error_naming_them() {
        let e = single_model(&[("Canon".into(), "EOS R5".into()), ("Nikon".into(), "Z 6".into()), ("canon".into(), "eos r5".into())]).unwrap_err();
        assert!(e.0.contains("EOS R5") && e.0.contains("Z 6") && e.0.contains("found 2"), "{e}");
        assert!(single_model(&[("A".into(), "B".into()), ("A".into(), "B".into())]).is_ok());
    }

    #[test]
    fn hold_out_share_is_deterministic_and_partial() {
        let n = 20;
        let held: Vec<bool> = (0..n).map(|i| held_out(i, n, 0.2)).collect();
        assert_eq!(held.iter().filter(|h| **h).count(), 4);
        assert!(!(0..1).any(|i| held_out(i, 1, 0.2)));
        assert!(!(0..10).any(|i| held_out(i, 10, 0.0)));
        assert!(!(0..10).any(|i| held_out(i, 10, f64::NAN)));
    }

    #[test]
    fn white_is_green_normalised() {
        let c = lcc_raw::ColorData { as_shot_neutral: Some([0.5, 0.8, 0.6]), ..Default::default() };
        let w = white_of(&c, None);
        assert!((w.neutral[1] - 1.0).abs() < 1e-12 && (w.neutral[0] - 0.625).abs() < 1e-12);
        let c = lcc_raw::ColorData::default();
        let w = white_of(&c, Some([2.0, 1.0, 1.5]));
        assert!((w.neutral[0] - 0.5).abs() < 1e-9);
        let w = white_of(&c, Some([0.0, f32::NAN, 1.0]));
        assert!(w.neutral.iter().all(|v| v.is_finite()));
    }
}
