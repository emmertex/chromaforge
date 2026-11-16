//! Photo ▸ Enhance: Raw Details and Super Resolution, each written as a new linear DNG next to the source photo
//! (`<name>-Enhanced-RD.dng`, `<name>-Enhanced-SR.dng`), imported, given the source's develop settings and stacked
//! with it, as Lightroom's Enhance does.
//!
//! - **Raw Details** demosaics a raw again with the slower edge-aware [`lcc_raw::Method::Detail`].
//! - **Super Resolution** does the same and then doubles each side ([`lcc_merge::super_resolve`]); it also takes
//!   non-raw photos (JPEG, TIFF, …), whose linear pixels it enlarges.
//!
//! - **AI Super Resolution** and **AI Sharpen** (adapted from upstream #3493) render the photo as an export
//!   would (its edits, crop and masks) to 16-bit sRGB at full size, run it through the installed upscaler
//!   (`Domain::RgbToRgb4x`, the `upscaleModel` slot of the denoise models folder) and write a 16-bit TIFF
//!   (`<name>-Enhanced-SR.tif`): 2× or 4× across, or (AI Sharpen) the model's answer averaged back down to the
//!   photo's own size. `enhance.superResolution {method: "ai"}` picks it; `method: "auto"` does when an upscaler is
//!   chosen.
//!
//! The pixels are made on a worker thread ([`plan`] → [`EnhanceWork`], `Send`), with a row in the activity stack for
//! the whole run; `enhance.pump` (called every frame by the app) writes and imports the finished ones. `wait: true`
//! (scripts, CLI, MCP; always on the web) runs inline instead.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use lcc_catalog::{PhotoId, Source};
use lcc_denoise::manifest::{DenoiserManifest, Domain};
use lcc_denoise::run::{Control, MAX_PIXELS, Params, Scales, upscale_rgb};
use lcc_pipeline::{DeepImage, DeepSamples, OutputDepth, OutputSpace};
use lcc_raster::Rgb32f;
use serde_json::{Value, json};

use crate::activity::{Cancel, TaskGuard};
use crate::denoise::{Loader, RunOn, STACK, installed_models, read_settings};
use crate::export::{ExportFormat, ExportOptions};
use crate::{EngineError, Result, Session};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnhanceKind {
    RawDetails,
    SuperResolution,
    /// The installed upscaler, 2× or 4× across.
    AiSuperResolution(u8),
    /// The installed upscaler's answer averaged back down to the photo's size.
    AiSharpen,
}

impl EnhanceKind {
    pub fn label(self) -> &'static str {
        match self {
            EnhanceKind::RawDetails => "Raw Details",
            EnhanceKind::SuperResolution => "Super Resolution",
            EnhanceKind::AiSuperResolution(_) => "AI Super Resolution",
            EnhanceKind::AiSharpen => "AI Sharpen",
        }
    }
    fn suffix(self) -> &'static str {
        match self {
            EnhanceKind::RawDetails => "Enhanced-RD",
            EnhanceKind::SuperResolution | EnhanceKind::AiSuperResolution(_) | EnhanceKind::AiSharpen => "Enhanced-SR",
        }
    }
    /// The factor of an AI result over the photo (`None`: not an AI kind).
    pub fn ai_factor(self) -> Option<usize> {
        match self {
            EnhanceKind::AiSuperResolution(f) => Some(usize::from(f)),
            EnhanceKind::AiSharpen => Some(1),
            _ => None,
        }
    }
    fn extension(self) -> &'static str {
        if self.ai_factor().is_some() { "tif" } else { "dng" }
    }
}

/// The largest 16-bit sample.
const U16_MAX: f32 = 65535.0;

/// The colour spaces an AI result may be made in, as `space` names them: the upscaler learned on sRGB pictures, and
/// these keep close enough to it that it behaves the same; ProPhoto RGB and Rec. 2020 are too far.
pub const AI_SPACES: [(&str, OutputSpace); 3] =
    [("srgb", OutputSpace::Srgb), ("displayP3", OutputSpace::DisplayP3), ("adobeRgb", OutputSpace::AdobeRgb)];

/// The colour space `name` stands for, if an AI result may be made in it.
pub fn ai_space(name: &str) -> std::result::Result<OutputSpace, String> {
    match OutputSpace::parse(name) {
        Some(s) if AI_SPACES.iter().any(|(_, a)| *a == s) => Ok(s),
        Some(s) => Err(format!("{} is too far from sRGB for the upscaler: use srgb, displayP3 or adobeRgb", s.label())),
        None => Err(format!("unknown colour space `{name}`: use srgb, displayP3 or adobeRgb")),
    }
}

/// The installed upscaler an AI job runs, loaded on the job's thread, and what goes into it.
#[derive(Clone)]
struct AiPlan {
    loader: Loader,
    onnx: std::path::PathBuf,
    manifest: DenoiserManifest,
    run_on: RunOn,
    /// The colour space the photo is rendered in, the model works on and the TIFF is tagged with.
    space: OutputSpace,
    /// The photo as an export renders it (its edits, crop and all), full size, 16-bit sRGB: what goes into the model.
    job: crate::media::RenderJob,
    metadata: Option<lcc_meta::Metadata>,
}

fn tiff_options() -> ExportOptions {
    ExportOptions { format: ExportFormat::Tiff, ..Default::default() }
}

/// Refuse a result over [`MAX_PIXELS`] before the full-size render of a `w` × `h` photo rather than after it. The
/// render may come out a pixel smaller, so only a result too large even then is refused here; the tiling checks the
/// rest.
fn check_result_size(w: f64, h: f64, factor: usize) -> std::result::Result<(), String> {
    if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
        return Err(format!("the photo's size ({w} × {h}) is not one a result can be made from"));
    }
    let least = |v: f64| (v.floor() as usize).saturating_sub(1);
    let pixels = least(w).saturating_mul(least(h)).saturating_mul(factor.saturating_mul(factor));
    if pixels > MAX_PIXELS {
        return Err(format!("the result would have {} megapixels, more than the {} a result may have", pixels / 1_000_000, MAX_PIXELS / 1_000_000));
    }
    Ok(())
}

impl AiPlan {
    /// The photo with its edits (as an export renders it), full size, as gamma-encoded sRGB in 0 to 1.
    fn render(&self, factor: usize) -> std::result::Result<Rgb32f, String> {
        let r = &self.job.request;
        check_result_size(r.max_w as f64, r.max_h as f64, factor)?;
        let deep = self.job.clone().run().rendered?.deep.ok_or("the photo could not be rendered at 16 bits")?;
        let DeepSamples::U16(samples) = deep.samples else { return Err("the photo was not rendered at 16 bits".into()) };
        let data: Vec<[f32; 3]> = samples.as_chunks::<3>().0.iter().map(|p| p.map(|v| f32::from(v) / U16_MAX)).collect();
        if data.len() != deep.width.saturating_mul(deep.height) {
            return Err("the rendered photo does not hold width × height pixels".into());
        }
        Ok(Rgb32f { width: deep.width, height: deep.height, data })
    }

    /// Run the upscaler over `image` for a result `factor` times its size; `cancelled()` stops it at the next tile.
    fn upscale(&self, image: &Rgb32f, factor: usize, cancelled: &(dyn Fn() -> bool + Sync)) -> std::result::Result<Rgb32f, String> {
        let model = (self.loader)(&self.onnx, &self.manifest)?;
        let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
        let (runner, parallel) = model.runner(self.run_on, cores);
        let params = Params {
            tile: self.manifest.tile as usize,
            overlap: self.manifest.overlap as usize,
            gain: self.manifest.gain,
            clip: None,
            parallel: parallel.max(1),
        };
        let cancel = AtomicBool::new(false);
        let report = |_: usize, _: usize| {
            if cancelled() {
                cancel.store(true, Ordering::Relaxed);
            }
        };
        let ctl = Control { cancel: Some(&cancel), progress: Some(&report) };
        let scales = Scales { model: Domain::RgbToRgb4x.scale(), out: factor };
        // the tiles run on threads of their own, so the window's renders keep theirs
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(params.parallel)
            .stack_size(STACK)
            .thread_name(|i| format!("enhance-{i}"))
            .build()
            .map_err(|e| format!("could not start the worker threads: {e}"))?;
        pool.install(|| upscale_rgb(image, &*runner, &params, &ctl, scales)).map_err(|e| e.to_string())
    }

    fn encode(&self, image: &Rgb32f) -> std::result::Result<Vec<u8>, String> {
        let samples = image.data.iter().flatten().map(|v| (v.clamp(0.0, 1.0) * U16_MAX).round() as u16).collect();
        let deep = DeepImage { width: image.width, height: image.height, space: self.space, samples: DeepSamples::U16(samples) };
        crate::export::encode_deep(&deep, &tiff_options(), self.metadata.as_ref())
    }
}

/// An sRGB picture (0 to 1) as 8-bit pixels for the dialog.
fn to_rgba8(img: &Rgb32f) -> lcc_raster::Rgba8 {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    lcc_raster::Rgba8 { width: img.width, height: img.height, data: img.data.iter().map(|p| [q(p[0]), q(p[1]), q(p[2]), 255]).collect() }
}

/// Finished and failed photos `enhance.pump` reports (the most recent).
const KEEP: usize = 50;

type ByteReader = Arc<dyn Fn(&str) -> std::result::Result<Vec<u8>, String> + Send + Sync>;

/// One photo to enhance, self-contained and `Send`.
pub struct EnhanceWork {
    pub kind: EnhanceKind,
    pub id: PhotoId,
    pub path: String,
    read: ByteReader,
    ai: Option<AiPlan>,
}

/// The finished pixels of one photo.
pub struct Enhanced {
    pub id: PhotoId,
    pub path: String,
    /// The new file's bytes: a linear DNG, or (AI kinds) a 16-bit TIFF.
    pub dng: Vec<u8>,
    pub size: (usize, usize),
}

impl EnhanceWork {
    /// Decode, enhance and encode. `cancelled()` is checked between stages and Super Resolution's passes.
    pub fn run(&self, cancelled: &(dyn Fn() -> bool + Sync)) -> std::result::Result<Enhanced, String> {
        if let (Some(ai), Some(factor)) = (&self.ai, self.kind.ai_factor()) {
            let image = ai.render(factor)?;
            if cancelled() {
                return Err("cancelled".into());
            }
            let big = ai.upscale(&image, factor, cancelled)?;
            drop(image);
            if cancelled() {
                return Err("cancelled".into());
            }
            let tiff = ai.encode(&big)?;
            return Ok(Enhanced { id: self.id, path: self.path.clone(), dng: tiff, size: (big.width, big.height) });
        }
        let bytes = (self.read)(&self.path)?;
        let is_raw = lcc_raw::probe(&bytes).is_some();
        if self.kind == EnhanceKind::RawDetails && !is_raw {
            return Err(format!("{}: Raw Details needs a raw photo", self.path));
        }
        let frame = lcc_merge::load_frame_full(&bytes, &crate::camera_preview::starting_colour_tags, lcc_raw::Method::Detail)
            .map_err(|e| format!("{}: {e}", self.path))?;
        drop(bytes);
        if cancelled() {
            return Err("cancelled".into());
        }
        let mut color = frame.color.clone();
        if let Some(m) = frame.wb_multipliers {
            color.set_as_shot_multipliers(m);
        }
        let image = match self.kind {
            EnhanceKind::RawDetails | EnhanceKind::AiSuperResolution(_) | EnhanceKind::AiSharpen => frame.image,
            EnhanceKind::SuperResolution => lcc_merge::super_resolve(&frame.image, &|_| !cancelled()).map_err(|e| e.to_string())?,
        };
        let size = (image.width, image.height);
        let dng = lcc_merge::write_linear_dng(
            &image,
            &color,
            frame.orientation,
            &frame.metadata,
            frame.baseline_exposure,
            lcc_merge::DngSamples::Half,
            &frame.lens,
        )
        .map_err(|e| e.to_string())?;
        Ok(Enhanced { id: self.id, path: self.path.clone(), dng, size })
    }
}

/// Side, in source pixels, of the square the dialog's preview enhances.
pub const PREVIEW_CROP: usize = 256;

/// What the Enhance dialog shows: the middle of the photo before and after, and an estimate of the full run.
pub struct EnhancePreview {
    /// The crop as the library shows the photo now (standard demosaic, source size).
    pub before: lcc_raster::Rgba8,
    /// The same crop enhanced (twice the size for Super Resolution).
    pub after: lcc_raster::Rgba8,
    /// The source's size and the enhanced photo's.
    pub source_size: (usize, usize),
    pub out_size: (usize, usize),
    /// Estimated seconds to enhance the whole photo (measured on this photo and the crop, scaled up).
    pub estimate_secs: f64,
}

fn render_linear(
    image: &lcc_raster::Rgb32f,
    frame: &lcc_merge::Frame,
    color: &lcc_merge::FrameColor,
) -> std::result::Result<lcc_raster::Rgba8, String> {
    let edge = image.width.max(image.height).max(1);
    let dng = lcc_merge::write_linear_dng(
        image,
        color,
        frame.orientation,
        &frame.metadata,
        frame.baseline_exposure,
        lcc_merge::DngSamples::Half,
        &frame.lens,
    )
    .map_err(|e| e.to_string())?;
    let (src, info) = crate::files::load_bytes(&dng, edge)?;
    let s = lcc_develop::DevelopSettings::for_raw(info.as_shot_temp, info.as_shot_tint);
    let req = lcc_pipeline::RenderRequest { quality: lcc_pipeline::Quality::Full, apply_crop: false, ..lcc_pipeline::RenderRequest::fit(edge, edge) };
    Ok(lcc_pipeline::render(&src, &info, &s, &req).image)
}

/// The middle `side` × `side` pixels (or less of a smaller image).
fn middle(img: &lcc_raster::Rgb32f, side: usize) -> lcc_raster::Rgb32f {
    let (w, h) = (side.min(img.width), side.min(img.height));
    img.crop(img.width.saturating_sub(w) / 2, img.height.saturating_sub(h) / 2, w, h)
}

impl EnhanceWork {
    /// The dialog's preview: the middle [`PREVIEW_CROP`] pixels before and after, and an estimate of the full run.
    pub fn preview(&self, cancelled: &(dyn Fn() -> bool + Sync)) -> std::result::Result<EnhancePreview, String> {
        if let (Some(ai), Some(factor)) = (&self.ai, self.kind.ai_factor()) {
            let t0 = std::time::Instant::now();
            let image = ai.render(factor)?;
            let render = t0.elapsed().as_secs_f64();
            if cancelled() {
                return Err("cancelled".into());
            }
            let source_size = (image.width, image.height);
            let crop = middle(&image, PREVIEW_CROP);
            drop(image);
            let t1 = std::time::Instant::now();
            let up = ai.upscale(&crop, factor, cancelled)?;
            let area = |s: (usize, usize)| (s.0 as f64 * s.1 as f64).max(1.0);
            let estimate_secs = render + t1.elapsed().as_secs_f64() * area(source_size) / area((crop.width, crop.height));
            let out_size = (source_size.0.saturating_mul(factor), source_size.1.saturating_mul(factor));
            return Ok(EnhancePreview { before: to_rgba8(&crop), after: to_rgba8(&up), source_size, out_size, estimate_secs });
        }
        let bytes = (self.read)(&self.path)?;
        let is_raw = lcc_raw::probe(&bytes).is_some();
        if self.kind == EnhanceKind::RawDetails && !is_raw {
            return Err(format!("{}: Raw Details needs a raw photo", self.path));
        }
        let colour = &crate::camera_preview::starting_colour_tags;
        let load = |m| lcc_merge::load_frame_full(&bytes, colour, m).map_err(|e| format!("{}: {e}", self.path));
        let base = load(lcc_raw::Method::Ahd)?;
        if cancelled() {
            return Err("cancelled".into());
        }
        let t0 = std::time::Instant::now();
        let detail = if is_raw { Some(load(lcc_raw::Method::Detail)?) } else { None };
        let decode = t0.elapsed().as_secs_f64();
        drop(bytes);
        if cancelled() {
            return Err("cancelled".into());
        }
        let frame = detail.as_ref().unwrap_or(&base);
        let mut color = frame.color.clone();
        if let Some(m) = frame.wb_multipliers {
            color.set_as_shot_multipliers(m);
        }
        let source_size = (frame.image.width, frame.image.height);
        let crop = middle(&frame.image, PREVIEW_CROP);
        let t1 = std::time::Instant::now();
        let (after, out_size, estimate_secs) = match self.kind {
            EnhanceKind::RawDetails | EnhanceKind::AiSuperResolution(_) | EnhanceKind::AiSharpen => (crop, source_size, decode),
            EnhanceKind::SuperResolution => {
                let up = lcc_merge::super_resolve(&crop, &|_| !cancelled()).map_err(|e| e.to_string())?;
                let area = |s: (usize, usize)| (s.0 as f64 * s.1 as f64).max(1.0);
                let scale = area(source_size) / area((crop.width, crop.height));
                (up, (source_size.0.saturating_mul(2), source_size.1.saturating_mul(2)), decode + t1.elapsed().as_secs_f64() * scale)
            }
        };
        let before = render_linear(&middle(&base.image, PREVIEW_CROP), &base, &color)?;
        let after = render_linear(&after, frame, &color)?;
        Ok(EnhancePreview { before, after, source_size, out_size, estimate_secs })
    }
}

struct Job {
    kind: EnhanceKind,
    rx: mpsc::Receiver<std::result::Result<Enhanced, String>>,
    left: usize,
    total: usize,
    stack: bool,
    guard: TaskGuard,
}

/// The running Enhance jobs and the outcome of the finished ones.
#[derive(Default)]
pub struct EnhanceJobs {
    jobs: Vec<Job>,
    made: Vec<Value>,
    failed: Vec<Value>,
}

fn output_names(first: &str, suffix: &str, ext: &str) -> impl Iterator<Item = std::path::PathBuf> {
    let p = std::path::Path::new(first);
    let dir = p.parent().map(|d| d.to_path_buf()).unwrap_or_default();
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "photo".into());
    let ext = ext.to_string();
    (0..10_000usize).map(move |i| if i == 0 { dir.join(format!("{stem}-{suffix}.{ext}")) } else { dir.join(format!("{stem}-{suffix}-{i}.{ext}")) })
}

impl Session {
    /// The work for each of `ids` (a photo named twice is enhanced once).
    /// Whether this build keeps AI models (the desktop app does), so an upscaler could be installed.
    pub fn keeps_ai_models(&self) -> bool {
        self.denoise.models_dir.is_some()
    }

    /// Is an upscaler chosen and installed (what AI Super Resolution and AI Sharpen need)?
    pub fn enhance_upscaler_ready(&self) -> bool {
        self.upscaler().is_ok()
    }

    /// The installed upscaler in use, or why there is none.
    fn upscaler(&self) -> Result<(crate::denoise::Installed, RunOn)> {
        let fail = |m: String| EngineError::Other(m);
        let dir = self.denoise.models_dir.as_deref().ok_or_else(|| fail("this build has nowhere to keep AI models (the desktop app does)".into()))?;
        let settings = read_settings(dir);
        let chosen = settings.upscale_model.as_deref().ok_or_else(|| {
            fail("no upscaler is installed: add one in Settings ▸ AI Models (`denoise.models.list` shows the ones the app can fetch)".into())
        })?;
        let installed = installed_models(dir)
            .into_iter()
            .find(|i| i.manifest.id == chosen && i.manifest.domain == Domain::RgbToRgb4x)
            .ok_or_else(|| fail(format!("the upscaler `{chosen}` is not installed any more: choose another in Settings ▸ AI Models")))?;
        Ok((installed, settings.run_on()))
    }

    /// `space` is the colour space of an AI result ([`ai_space`]); other kinds ignore it.
    pub fn plan_enhance(&mut self, kind: EnhanceKind, ids: &[PhotoId], space: OutputSpace) -> Result<Vec<EnhanceWork>> {
        if let EnhanceKind::AiSuperResolution(f) = kind
            && f != 2
            && f != 4
        {
            return Err(EngineError::Other(format!("AI Super Resolution gives 2 or 4 times the photo's size, not {f}")));
        }
        let upscaler = match kind.ai_factor() {
            Some(_) => Some(self.upscaler()?),
            None => None,
        };
        let read: ByteReader = match &self.media.file_bytes {
            Some(r) => r.clone(),
            None => Arc::new(|path: &str| std::fs::read(path).map_err(|e| format!("{path}: {e}"))),
        };
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for id in ids.iter().filter(|id| seen.insert(**id)) {
            let p = self.catalog.photo(*id).ok_or(lcc_catalog::CatalogError::NoPhoto(*id))?;
            match &p.source {
                Source::File { path } => {
                    let path = path.clone();
                    // an AI result is the photo as an export renders it: its edits, crop and masks go into the model
                    let ai = match (&upscaler, kind.ai_factor()) {
                        (Some((i, run_on)), Some(factor)) => {
                            let (w, h) = crate::export::output_size(p, &tiff_options());
                            check_result_size(w as f64, h as f64, factor).map_err(EngineError::Other)?;
                            let metadata = crate::export::export_metadata(p, &self.catalog, &tiff_options());
                            let job = self.export_job(*id, w, h, space, OutputDepth::U16).map_err(EngineError::Other)?;
                            Some(AiPlan {
                                loader: self.denoise.loader.clone(),
                                onnx: i.onnx.clone(),
                                manifest: i.manifest.clone(),
                                run_on: *run_on,
                                space,
                                job,
                                metadata,
                            })
                        }
                        _ => None,
                    };
                    out.push(EnhanceWork { kind, id: *id, path, read: read.clone(), ai });
                }
                Source::Demo { .. } => {
                    return Err(EngineError::Other(format!("{} is a demo photo; {} needs the photo's file", p.file_name, kind.label())));
                }
            }
        }
        if out.is_empty() {
            return Err(EngineError::Other(format!("select a photo for {}", kind.label())));
        }
        Ok(out)
    }

    /// Write a finished photo next to its source, import it, give it the source's develop settings and (with
    /// `stack`) stack it with the source. Returns `{id, source, path, size}`.
    pub fn finish_enhance(&mut self, kind: EnhanceKind, out: Enhanced, stack: bool) -> Result<Value> {
        // the same photo enhanced again gives the same bytes: import would refuse the copy, so the one there is kept
        let hash = lcc_preview::hash_bytes(&out.dng).to_string();
        let existing = self.catalog.photos().find_map(|p| match &p.source {
            Source::File { path } if !p.deleted && p.content_hash.as_deref() == Some(hash.as_str()) && std::path::Path::new(path).exists() => {
                Some((p.id, path.clone()))
            }
            _ => None,
        });
        if let Some((id, path)) = existing {
            return Ok(
                json!({"id": id.0, "source": out.id.0, "path": path, "size": [out.size.0, out.size.1], "bytes": out.dng.len(), "reused": true}),
            );
        }
        let path = lcc_catalog::safe_file::write_new_unique(&mut output_names(&out.path, kind.suffix(), kind.extension()), &out.dng)
            .map_err(|e| EngineError::Other(format!("could not write the enhanced file next to {}: {e}", out.path)))?
            .to_string_lossy()
            .to_string();
        let report = match crate::import::import(self, std::slice::from_ref(&path), crate::import::ImportMode::Add) {
            Ok(r) => r,
            Err(e) => {
                let _ = std::fs::remove_file(&path);
                return Err(e);
            }
        };
        let Some(id) = report.imported.first().copied().map(PhotoId) else {
            let _ = std::fs::remove_file(&path);
            return Err(EngineError::Other(format!("the enhanced file {path} could not be imported: {:?}", report.failed)));
        };
        // an AI result has the source's edits baked in: giving it them again would apply them twice
        if kind.ai_factor().is_none()
            && let Some(d) = self.develop_of(out.id)
        {
            let d = (*d).clone();
            self.set_develop(id, d, kind.label())?;
        }
        if stack && let Some(op) = self.catalog.stack_with_ops(id, &[out.id]) {
            self.commit("Stack with Source", op)?;
        }
        Ok(json!({"id": id.0, "source": out.id.0, "path": path, "size": [out.size.0, out.size.1], "bytes": out.dng.len()}))
    }

    /// `enhance.rawDetails` / `enhance.superResolution` / `enhance.aiSharpen` `{ids?, stack?: true, wait?: false}`.
    pub(crate) fn enhance_command(&mut self, kind: EnhanceKind, p: &Value) -> Result<Value> {
        let ids = self.targets(p)?;
        let space = match p.get("space") {
            None | Some(Value::Null) => OutputSpace::Srgb,
            Some(_) if kind.ai_factor().is_none() => {
                return Err(EngineError::Other(format!("`space` is for AI Super Resolution and AI Sharpen; {} makes a DNG", kind.label())));
            }
            Some(v) => ai_space(v.as_str().unwrap_or("")).map_err(EngineError::Other)?,
        };
        let work = self.plan_enhance(kind, &ids, space)?;
        let stack = p.get("stack").and_then(Value::as_bool).unwrap_or(true);
        let wait = p.get("wait").and_then(Value::as_bool).unwrap_or(false) || cfg!(target_arch = "wasm32");
        let cancel = Arc::new(AtomicBool::new(false));
        let guard = self.activity.start("enhance", kind.label(), Cancel::Flag(cancel.clone()));
        let total = work.len();
        guard.progress(0, total as u64);
        if wait {
            let (mut made, mut failed) = (Vec::new(), Vec::new());
            for (i, w) in work.iter().enumerate() {
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| w.run(&|| cancel.load(Ordering::Relaxed))))
                    .unwrap_or_else(|_| Err(format!("{}: {} failed unexpectedly", w.path, kind.label())));
                match r.map_err(EngineError::Other).and_then(|out| self.finish_enhance(kind, out, stack)) {
                    Ok(v) => made.push(v),
                    Err(e) => failed.push(json!({"id": w.id.0, "error": e.to_string()})),
                }
                guard.progress(i as u64 + 1, total as u64);
            }
            drop(guard);
            if made.is_empty()
                && let Some(e) = failed.first()
            {
                return Err(EngineError::Other(e["error"].as_str().unwrap_or("failed").to_string()));
            }
            return Ok(json!({"made": made, "failed": failed}));
        }
        let (tx, rx) = mpsc::channel();
        let flag = cancel.clone();
        std::thread::Builder::new()
            .name("enhance".into())
            .spawn(move || {
                for w in work {
                    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| w.run(&|| flag.load(Ordering::Relaxed))))
                        .unwrap_or_else(|_| Err(format!("{}: failed unexpectedly", w.path)));
                    if tx.send(r).is_err() || flag.load(Ordering::Relaxed) {
                        return;
                    }
                }
            })
            .map_err(|e| EngineError::Other(format!("could not start {}: {e}", kind.label())))?;
        self.enhance.jobs.push(Job { kind, rx, left: total, total, stack, guard });
        Ok(json!({"queued": total}))
    }

    /// `enhance.pump`: take in finished photos. Cheap when idle.
    pub(crate) fn enhance_pump(&mut self) -> Value {
        let mut done = Vec::new();
        let mut i = 0;
        while i < self.enhance.jobs.len() {
            let mut finished = false;
            while let Some(job) = self.enhance.jobs.get_mut(i) {
                match job.rx.try_recv() {
                    Ok(r) => {
                        job.left = job.left.saturating_sub(1);
                        done.push((job.kind, job.stack, r));
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        finished = true;
                        break;
                    }
                }
            }
            if let Some(job) = self.enhance.jobs.get(i) {
                finished |= job.left == 0;
            }
            if finished {
                self.enhance.jobs.remove(i);
            } else {
                i += 1;
            }
        }
        for (kind, stack, r) in done {
            let id = r.as_ref().map(|o| o.id.0).ok();
            match r.map_err(EngineError::Other).and_then(|out| self.finish_enhance(kind, out, stack)) {
                Ok(v) => self.enhance.made.push(v),
                Err(e) => self.enhance.failed.push(json!({"id": id, "error": e.to_string()})),
            }
        }
        for job in &self.enhance.jobs {
            job.guard.progress(job.total.saturating_sub(job.left) as u64, job.total as u64);
        }
        for list in [&mut self.enhance.made, &mut self.enhance.failed] {
            let extra = list.len().saturating_sub(KEEP);
            list.drain(..extra);
        }
        let running: usize = self.enhance.jobs.iter().map(|j| j.left).sum();
        json!({"active": running > 0, "running": running, "made": self.enhance.made, "failed": self.enhance.failed})
    }
}

#[cfg(test)]
mod tests {
    use super::check_result_size;

    #[test]
    fn a_result_too_large_is_refused_before_the_render() {
        // 45 MP: too large at 2×, fine as AI Sharpen
        assert!(check_result_size(8256.0, 5504.0, 2).unwrap_err().contains("more than the 110"));
        assert!(check_result_size(8256.0, 5504.0, 1).is_ok());
        // 27.5 MP at 2× (110 MP) may come out a pixel smaller after the render: the tiling decides
        assert!(check_result_size(6400.0, 4297.5, 2).is_ok());
        // a size that is not a number never gets through to the render
        assert!(check_result_size(f64::NAN, 4000.0, 2).is_err());
        assert!(check_result_size(6000.0, f64::INFINITY, 2).is_err());
        assert!(check_result_size(0.0, 4000.0, 2).is_err());
    }

    #[test]
    fn an_ai_result_is_made_only_in_a_space_near_srgb() {
        use super::ai_space;
        use lcc_pipeline::OutputSpace;
        assert_eq!(ai_space("displayP3"), Ok(OutputSpace::DisplayP3));
        assert_eq!(ai_space("adobeRgb"), Ok(OutputSpace::AdobeRgb));
        assert!(ai_space("prophoto").is_err());
        assert!(ai_space("").is_err());
    }
}
