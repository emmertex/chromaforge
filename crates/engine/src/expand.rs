//! Crop ▸ Fill Edges (issue #861): filling the part of a crop extended past the photo.
//!
//! A crop with Extend Past the Edge ([`lcc_develop::Crop::extend`]) may reach past the photo, where the render shows
//! blank canvas. Fill Edges fills that canvas once, on demand, and keeps the result as a patch of the photo's own
//! (EXIF-oriented, as-shot, scene-linear) source, placed past its edges: the same `.lcpatch` file a generative fill
//! makes, in the photo's `genfill/<photo>/edge/` folder (apart from the spot generations, so the Remove panel's list of
//! results never offers it to a spot). [`lcc_develop::DevelopSettings::edge_fill`] refers to it, and renders composite
//! it first thing in the scene-linear stage ([`lcc_pipeline::genpatch::apply_edge_fill`]), so every adjustment applies
//! to it as to the photo and crops or rotations done later carry it along.
//!
//! * **Content-Aware** ([`EdgeFillKind::ContentAware`]): PatchMatch completion ([`lcc_inpaint`]) of the area past the
//!   photo from the photo itself, on this computer, at up to [`WORK_PIXELS`] (a larger area is filled at a lower
//!   resolution: the fill is softer than the photo, and the render adds the photo's grain to it).
//! * **Generative** ([`EdgeFillKind::Generative`]): the same area sent in one request, with the photo around it, to a
//!   generative fill service the user set up with their own key and whose terms they accepted, as a generative fill
//!   spot is (`docs/generative-fill.md`). Never sent without the user asking.
//!
//! Either runs off the UI thread with an activity row and ✕ ([`Session::edge_fill_start`]); the result lands on the
//! photo in one undo step ([`Session::edge_fill_poll`]).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use lcc_catalog::PhotoId;
use lcc_develop::{DevelopSettings, EdgeFill, EdgeFillKind, EmbeddedLens, GenFill};
use lcc_genfill::PatchLayer;
use lcc_genfill::patch::PatchInfo;
use lcc_geom::Point;
use lcc_pipeline::geometry::Frame;
use lcc_raster::Rgb32f;

/// The folder (in the photo's generative fill folder) edge fills are kept in.
pub const EDGE_DIR: &str = "edge";
/// The most pixels a content-aware edge fill works at (the area past the photo and the photo around it).
pub const WORK_PIXELS: f64 = 4.0e6;
/// What Fill Edges says when the crop doesn't reach past the photo.
pub const NOTHING_TO_FILL: &str = "the crop doesn't reach past the photo: turn on Extend Past the Edge and drag it out first";

/// The area (source px of an EXIF-oriented `src_w × src_h` source, `[x0, y0, x1, y1]`, past `0..w × 0..h` where it
/// reaches beyond the photo) an edge fill of a photo developed with `s` covers: what its crop shows, with a margin.
/// `None` when the crop doesn't extend or stays on the photo.
pub fn fill_area(s: &DevelopSettings, lens: Option<&EmbeddedLens>, src_w: usize, src_h: usize) -> Option<[f64; 4]> {
    if !s.crop.extend || src_w < 2 || src_h < 2 {
        return None;
    }
    let frame = Frame::with_lens(src_w, src_h, s, true, lens);
    let q = frame.crop_quad_norm();
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    // along each side (lens corrections bend them in the source)
    const STEPS: usize = 32;
    for i in 0..4 {
        let (a, b) = (q[i], q[(i + 1) % 4]);
        for k in 0..=STEPS {
            let t = k as f64 / STEPS as f64;
            let p = lcc_pipeline::genpatch::transformed_to_source(&frame, Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t));
            if p.x.is_finite() && p.y.is_finite() {
                (x0, y0, x1, y1) = (x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y));
            }
        }
    }
    let (sw, sh) = (src_w as f64, src_h as f64);
    if !(x0 < -0.5 || y0 < -0.5 || x1 > sw + 0.5 || y1 > sh + 0.5) || !(x0 < x1 && y0 < y1) {
        return None;
    }
    // a margin, so a small nudge of the crop stays filled; a patch's rectangle stays in −1..2 of the source
    let pad = 0.02 * sw.max(sh) + 8.0;
    Some([(x0 - pad).max(-sw), (y0 - pad).max(-sh), (x1 + pad).min(2.0 * sw), (y1 + pad).min(2.0 * sh)])
}

/// How far (canvas px) into the photo a fill is blended, so no seam shows along the photo's edge.
const FEATHER: usize = 6;

/// The area past the photo and the photo around it, at `k` × the source's resolution.
struct Canvas {
    img: Rgb32f,
    /// Pixels past the photo.
    hole: Vec<bool>,
    /// The fill's weight: 1 past the photo, falling to 0 over [`FEATHER`] px into it.
    alpha: Vec<f32>,
    /// What is filled (made anew): where the weight is above 0.
    fill: Vec<bool>,
}

/// [`Canvas`] of `rect` (source px) of `src` at scale `k`.
fn canvas(src: &Rgb32f, rect: [f64; 4], k: f64) -> Result<Canvas, String> {
    let [x0, y0, x1, y1] = rect;
    let cw = ((x1 - x0) * k).ceil().max(1.0);
    let ch = ((y1 - y0) * k).ceil().max(1.0);
    if !(cw * ch <= lcc_genfill::patch::MAX_PIXELS as f64 && cw.max(ch) <= f64::from(lcc_genfill::patch::MAX_SIDE)) {
        return Err("the area to fill is too large".into());
    }
    let (cw, ch) = (cw as usize, ch as usize);
    let (sw, sh) = (src.width as f64, src.height as f64);
    // the photo at the working resolution (area-downscaled: no aliasing)
    let scaled;
    let base = if k < 0.999 {
        let (bw, bh) = (((sw * k).round() as usize).max(1), ((sh * k).round() as usize).max(1));
        scaled = lcc_raster::resample::try_resize(src, bw, bh, lcc_raster::resample::Filter::Mitchell).map_err(|e| e.to_string())?;
        &scaled
    } else {
        src
    };
    let (bx, by) = (base.width as f64 / sw.max(1.0), base.height as f64 / sh.max(1.0));
    let at = |x: usize, y: usize| (x0 + (x as f64 + 0.5) / k, y0 + (y as f64 + 0.5) / k);
    let on_photo = |(sx, sy): (f64, f64)| sx >= 0.0 && sy >= 0.0 && sx < sw && sy < sh;
    let hole: Vec<bool> = (0..cw * ch).map(|i| !on_photo(at(i % cw, i / cw))).collect();
    let img = Rgb32f::from_fn(cw, ch, |x, y| {
        let (sx, sy) = at(x, y);
        if on_photo((sx, sy)) { base.sample_bilinear((sx * bx) as f32, (sy * by) as f32) } else { [0.0; 3] }
    });
    if !hole.iter().any(|h| *h) {
        return Err(NOTHING_TO_FILL.into());
    }
    if hole.iter().all(|h| *h) {
        return Err("the crop shows none of the photo to fill from".into());
    }
    let mut alpha: Vec<f32> = hole.iter().map(|h| if *h { 1.0 } else { 0.0 }).collect();
    for d in 1..=FEATHER {
        let reach = lcc_inpaint::mask::dilate(cw, ch, &hole, d);
        let a = 1.0 - d as f32 / (FEATHER + 1) as f32;
        for (v, r) in alpha.iter_mut().zip(reach) {
            if r && *v == 0.0 {
                *v = a * a * (3.0 - 2.0 * a);
            }
        }
    }
    let fill = alpha.iter().map(|a| *a > 0.0).collect();
    Ok(Canvas { img, hole, alpha, fill })
}

/// The patch of a filled canvas: the fill past the photo (weight 1, blended into the photo's edge) and the photo
/// itself under it (weight 0).
fn patch(
    c: &Canvas,
    filled: Rgb32f,
    rect: [f64; 4],
    src_w: usize,
    src_h: usize,
    k: f64,
    provider: &str,
    neutral: lcc_genfill::Neutral,
) -> Result<PatchLayer, String> {
    let mut rgb = filled;
    for ((g, o), f) in rgb.data.iter_mut().zip(&c.img.data).zip(&c.fill) {
        if !f {
            *g = *o;
        }
    }
    let alpha = lcc_raster::Plane { width: c.img.width, height: c.img.height, data: c.alpha.clone() };
    let (sw, sh) = (src_w.max(1) as f64, src_h.max(1) as f64);
    let info = PatchInfo {
        rect: [rect[0] / sw, rect[1] / sh, rect[2] / sw, rect[3] / sh],
        source_w: u32::try_from(src_w).unwrap_or(u32::MAX),
        source_h: u32::try_from(src_h).unwrap_or(u32::MAX),
        scale: k.min(1.0),
        provider: provider.to_string(),
        neutral,
        ..Default::default()
    };
    PatchLayer::new(info, rgb, alpha).map_err(|e| e.to_string())
}

/// Content-aware fill of `c`'s hole (PatchMatch completion from the photo around it).
fn complete(c: &Canvas, seed: u64, cancel: &AtomicBool) -> Result<Rgb32f, String> {
    let (w, h) = (c.img.width, c.img.height);
    let flat: Vec<f32> = c.img.data.iter().flatten().copied().collect();
    let params = lcc_inpaint::patchmatch::CompleteParams { seed, ..Default::default() };
    let out = lcc_inpaint::patchmatch::complete_with(w, h, 3, &flat, &c.fill, &params, &|| cancel.load(Ordering::Relaxed))
        .map_err(|_| "cancelled".to_string())?
        .ok_or("there is too little of the photo to fill from")?;
    let data: Vec<[f32; 3]> = out.chunks_exact(3).map(|p| [p[0], p[1], p[2]]).collect();
    if data.len() != w * h {
        return Err("the fill came out the wrong size".into());
    }
    Ok(Rgb32f { width: w, height: h, data })
}

/// What a Fill Edges job needs, taken from the session to its thread.
pub struct Work {
    pub kind: EdgeFillKind,
    pub settings: Arc<DevelopSettings>,
    pub source: crate::media::SourceRef,
    pub header: lcc_pipeline::SourceInfo,
    pub dir: std::path::PathBuf,
    pub id: String,
    #[cfg(not(target_arch = "wasm32"))]
    pub cloud: Option<Cloud>,
}

/// A generative edge fill's service and prompt.
#[cfg(not(target_arch = "wasm32"))]
pub struct Cloud {
    pub provider: lcc_genfill::cloud::Provider,
    pub key: String,
    pub endpoint: lcc_genfill::cloud::Endpoint,
    pub prompt: String,
}

/// Run a Fill Edges job (on any thread): load the full-size source, fill the area past the photo, write the patch.
pub fn run(w: &Work, cancel: &AtomicBool, cache: &crate::genfill::PatchCache) -> Result<EdgeFill, String> {
    let source = w.source.load_source()?;
    let src = &source.image;
    let info = source.info_or(w.header.clone());
    let rect = fill_area(&w.settings, info.lens.as_ref(), src.width, src.height).ok_or(NOTHING_TO_FILL)?;
    let seed = u64::from_str_radix(&w.id, 16).unwrap_or(1);
    let area = (rect[2] - rect[0]) * (rect[3] - rect[1]);
    let (layer, provider, prompt) = match w.kind {
        EdgeFillKind::ContentAware => {
            let k = (WORK_PIXELS / area.max(1.0)).sqrt().min(1.0);
            let c = canvas(src, rect, k)?;
            let filled = complete(&c, seed, cancel)?;
            (patch(&c, filled, rect, src.width, src.height, k, "contentAware", Default::default())?, "contentAware".to_string(), String::new())
        }
        EdgeFillKind::Generative => {
            #[cfg(not(target_arch = "wasm32"))]
            {
                let cl = w.cloud.as_ref().ok_or("no generative fill service chosen")?;
                (generate(cl, src, rect, seed, cancel)?, cl.provider.info().id.to_string(), cl.prompt.clone())
            }
            #[cfg(target_arch = "wasm32")]
            return Err("generative fill is not available in the web version".into());
        }
    };
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    let fill = GenFill { id: w.id.clone(), variants: 1, variant: 0, provider, prompt, scale: layer.info.scale, ..Default::default() };
    let path = w.dir.join(fill.file_name(0));
    crate::genfill::write_patch(&path, &layer)?;
    cache.forget(&path);
    let (sw, sh) = (src.width.max(1) as f64, src.height.max(1) as f64);
    Ok(EdgeFill { kind: w.kind, fill, rect: [rect[0] / sw, rect[1] / sh, rect[2] / sw, rect[3] / sh] })
}

/// A generative fill of the area past the photo: one request to `cl`'s service with the photo around it.
#[cfg(not(target_arch = "wasm32"))]
fn generate(cl: &Cloud, src: &Rgb32f, rect: [f64; 4], seed: u64, cancel: &AtomicBool) -> Result<PatchLayer, String> {
    use lcc_genfill::cloud::{Control, Request, generate};
    use lcc_raster::resample::{Filter, resize};
    let info = cl.provider.info();
    let (aw, ah) = (rect[2] - rect[0], rect[3] - rect[1]);
    // what is sent: one of the service's sizes nearest the area's shape, or the area up to its longest side
    let send = match info.sizes.iter().min_by(|a, b| {
        let ar = aw / ah.max(1e-9);
        let da = (f64::from(a.0) / f64::from(a.1) / ar).ln().abs();
        let db = (f64::from(b.0) / f64::from(b.1) / ar).ln().abs();
        da.total_cmp(&db)
    }) {
        Some(&(a, b)) => (a as usize, b as usize),
        None => {
            let k = (f64::from(info.max_side) / aw.max(ah).max(1.0)).min(1.0);
            (((aw * k).round() as usize).max(16), ((ah * k).round() as usize).max(16))
        }
    };
    // the canvas at the resolution sent (a fixed size's slightly different shape is undone on the way back)
    let k = (send.0 as f64 / aw.max(1.0)).max(send.1 as f64 / ah.max(1.0)).min(1.0);
    let c = canvas(src, rect, k)?;
    // the hole sent as the photo's average colour (the encoding is measured on the photo)
    let known: Vec<[f32; 3]> = c.img.data.iter().zip(&c.hole).filter(|(_, h)| !**h).map(|(p, _)| *p).collect();
    let n = known.len().max(1) as f32;
    let mean = known.iter().fold([0.0f32; 3], |a, p| [a[0] + p[0] / n, a[1] + p[1] / n, a[2] + p[2] / n]);
    let mut flat = c.img.clone();
    for (p, h) in flat.data.iter_mut().zip(&c.hole) {
        if *h {
            *p = mean;
        }
    }
    let crop = if (flat.width, flat.height) == send { flat } else { resize(&flat, send.0, send.1, Filter::Mitchell) };
    let neutral = lcc_genfill::Neutral::for_crop(&crop);
    // a send pixel is generated when any canvas pixel under it is filled (grown a little: no seam left)
    let grown = lcc_inpaint::mask::dilate(c.img.width, c.img.height, &c.fill, 2);
    let mut mask = vec![0u8; send.0 * send.1];
    let (fx, fy) = (c.img.width as f64 / send.0 as f64, c.img.height as f64 / send.1 as f64);
    for (i, m) in mask.iter_mut().enumerate() {
        let (x, y) = (i % send.0, i / send.0);
        let (cx, cy) = (((x as f64 + 0.5) * fx) as usize, ((y as f64 + 0.5) * fy) as usize);
        if grown.get(cy.min(c.img.height - 1) * c.img.width + cx.min(c.img.width - 1)).copied().unwrap_or(false) {
            *m = 255;
        }
    }
    let req = Request { width: send.0, height: send.1, rgb: neutral.encode(&crop), mask, prompt: cl.prompt.clone(), variations: 1, seed };
    let ctl = Control { cancel, stall: std::time::Duration::from_secs(180), deadline: std::time::Duration::from_secs(600), accepted: None };
    let out = generate(cl.provider, &cl.key, &cl.endpoint, &req, &ctl).map_err(|e| e.to_string())?;
    let img = out.first().ok_or("the provider sent no image")?;
    let lin = neutral.decode(img.width, img.height, &img.rgb).ok_or("the provider sent an unreadable image")?;
    let mut lin =
        if (lin.width, lin.height) == (c.img.width, c.img.height) { lin } else { resize(&lin, c.img.width, c.img.height, Filter::Mitchell) };
    // matched to the photo along the edge
    let near = lcc_inpaint::mask::dilate(c.img.width, c.img.height, &c.fill, 16);
    let ring: Vec<bool> = near.iter().zip(&c.fill).map(|(n, f)| *n && !*f).collect();
    let _ = lcc_genfill::matching::match_to_surroundings(&mut lin, &c.img, &c.fill, &ring);
    patch(&c, lin, rect, src.width, src.height, k, info.id, neutral)
}

/// A running or finished Fill Edges job.
pub struct Job {
    pub photo: PhotoId,
    pub kind: EdgeFillKind,
    outcome: Mutex<Option<Result<EdgeFill, String>>>,
    cancel: Arc<AtomicBool>,
}

impl Job {
    pub fn running(&self) -> bool {
        self.outcome.lock().unwrap_or_else(PoisonError::into_inner).is_none()
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The session's Fill Edges state.
#[derive(Default)]
pub struct EdgeFillState {
    pub job: Option<Arc<Job>>,
}

impl EdgeFillState {
    pub fn running(&self) -> bool {
        self.job.as_ref().is_some_and(|j| j.running())
    }
}

impl crate::Session {
    /// Where photo `id`'s edge fills are kept.
    pub fn edge_fill_dir(&self, id: PhotoId) -> std::path::PathBuf {
        self.genfill_dir(id).join(EDGE_DIR)
    }

    /// Fill the area of photo `id`'s crop past the photo ([`EdgeFillKind`]; a generative fill with `generative`'s
    /// service (its id, empty for the default) and prompt, whose terms the user accepted, with their own key). In the app it runs in the background
    /// ([`crate::genfill::GenfillState::background`]) and returns `None`; [`Session::edge_fill_poll`] puts the result on
    /// the photo. Elsewhere it waits and returns the fill, on the photo already.
    pub fn edge_fill_start(&mut self, id: PhotoId, kind: EdgeFillKind, generative: Option<(&str, &str)>) -> Result<Option<EdgeFill>, String> {
        use crate::activity::Cancel;
        if self.edge_fill.running() {
            return Err("an edge fill is already running; wait for it or cancel it".into());
        }
        let p = self.catalog.photo(id).ok_or("no such photo")?.clone();
        if !p.develop.crop.extend {
            return Err(NOTHING_TO_FILL.into());
        }
        if p.width >= 2 && p.height >= 2 && fill_area(&p.develop, p.embedded_lens.as_deref(), p.width as usize, p.height as usize).is_none() {
            return Err(NOTHING_TO_FILL.into());
        }
        #[cfg(not(target_arch = "wasm32"))]
        let cloud = match (kind, generative) {
            (EdgeFillKind::ContentAware, _) => None,
            (EdgeFillKind::Generative, g) => {
                let (pid, prompt) = g.unwrap_or_default();
                let pid = if pid.is_empty() { lcc_genfill::cloud::DEFAULT.info().id } else { pid };
                let provider = lcc_genfill::cloud::Provider::from_id(pid).ok_or_else(|| format!("unknown provider `{pid}`"))?;
                if !self.genfill.consented(provider.info().id) {
                    return Err(format!("accept {}'s terms first (Settings ▸ Generative Fill)", provider.info().name));
                }
                if prompt.chars().count() > 1000 {
                    return Err("the prompt is too long (at most 1000 characters)".into());
                }
                let (key, _) = self.genfill.keys.get(provider).ok_or_else(|| lcc_genfill::cloud::GenError::NoKey.to_string())?;
                let endpoint = match &self.genfill.mock {
                    Some(m) => lcc_genfill::cloud::Endpoint::mock(m)?,
                    None => lcc_genfill::cloud::Endpoint::for_provider(provider),
                };
                Some(Cloud { provider, key, endpoint, prompt: prompt.trim().to_string() })
            }
        };
        #[cfg(target_arch = "wasm32")]
        if kind == EdgeFillKind::Generative {
            let _ = generative;
            return Err("generative fill is not available in the web version".into());
        }
        let work = Work {
            kind,
            settings: p.develop.clone(),
            source: self.media.source_ref_uncached(&p, crate::media::SourceLevel::Full),
            header: crate::media::source_info(&p),
            dir: self.edge_fill_dir(id),
            id: crate::genfill::new_id(id.0),
            #[cfg(not(target_arch = "wasm32"))]
            cloud,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let job = Arc::new(Job { photo: id, kind, outcome: Mutex::new(None), cancel: cancel.clone() });
        let guard = self.activity.start("edgeFill", "Filling edges", Cancel::Flag(cancel.clone()));
        let cache = self.genfill.cache.clone();
        let j = job.clone();
        let body = move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&work, &cancel, &cache)))
                .unwrap_or_else(|_| Err("the edge fill stopped unexpectedly".into()));
            let r = if cancel.load(Ordering::Relaxed) { Err("cancelled".to_string()) } else { r };
            *j.outcome.lock().unwrap_or_else(PoisonError::into_inner) = Some(r);
        };
        self.edge_fill.job = Some(job);
        #[cfg(not(target_arch = "wasm32"))]
        if self.genfill.background {
            std::thread::Builder::new()
                .name("edge fill".into())
                .spawn(move || {
                    let _guard = guard;
                    body();
                })
                .map_err(|e| {
                    self.edge_fill.job = None;
                    format!("could not start the edge fill: {e}")
                })?;
            return Ok(None);
        }
        body();
        drop(guard);
        match self.edge_fill_poll() {
            Some(r) => r.map(Some),
            None => Err("the edge fill ended without a result".into()),
        }
    }

    /// Put a finished Fill Edges job's fill on its photo (one undo step) and report it. Cheap when nothing is
    /// running: the app calls it every frame.
    pub fn edge_fill_poll(&mut self) -> Option<Result<EdgeFill, String>> {
        let job = self.edge_fill.job.clone()?;
        let outcome = job.outcome.lock().unwrap_or_else(PoisonError::into_inner).take()?;
        self.edge_fill.job = None;
        Some(outcome.and_then(|fill| {
            let mut d = (*self.develop_of(job.photo).ok_or("the photo is gone")?).clone();
            // (it lands whatever happened meanwhile: the fill is kept with the crop's reach)
            d.crop.extend = true;
            d.edge_fill = Some(fill.clone());
            self.set_develop(job.photo, d, "Fill Edges").map_err(|e| e.to_string())?;
            Ok(fill)
        }))
    }

    /// Cancel a running Fill Edges job; `false` when none is running.
    pub fn edge_fill_cancel(&mut self) -> bool {
        match &self.edge_fill.job {
            Some(j) if j.running() => {
                j.cancel();
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extended(rect: [f64; 4]) -> DevelopSettings {
        let mut s = DevelopSettings::default();
        s.crop.extend = true;
        s.crop.geometry.rect = lcc_geom::Rect::new(rect[0], rect[1], rect[2], rect[3]);
        s
    }

    #[test]
    fn the_fill_area_covers_the_crop_past_the_photo() {
        let a = fill_area(&extended([-0.25, 0.0, 1.0, 1.0]), None, 400, 300).unwrap();
        assert!(a[0] <= -100.0 && a[0] > -140.0, "{a:?}");
        assert!(a[2] >= 400.0 && a[3] >= 300.0);
        // a crop on the photo has nothing to fill
        assert_eq!(fill_area(&extended([0.1, 0.1, 0.9, 0.9]), None, 400, 300), None);
        let mut off = extended([-0.25, 0.0, 1.0, 1.0]);
        off.crop.extend = false;
        assert_eq!(fill_area(&off, None, 400, 300), None);
    }

    #[test]
    fn hostile_crops_give_no_area_or_one_in_bounds() {
        for r in [[f64::NAN, 0.0, 1.0, 1.0], [-1e300, -1e300, 1e300, 1e300], [f64::INFINITY, 0.0, 2.0, 1.0], [0.5, 0.5, 0.5, 0.5]] {
            let mut s = extended(r);
            s.crop.geometry.angle = 44.0;
            if let Some(a) = fill_area(&s, None, 400, 300) {
                assert!(a.iter().all(|v| v.is_finite()) && a[0] >= -400.0 && a[2] <= 800.0 && a[1] >= -300.0 && a[3] <= 600.0, "{a:?}");
            }
        }
        assert_eq!(fill_area(&extended([-0.5, 0.0, 1.0, 1.0]), None, 0, 0), None);
    }

    #[test]
    fn a_content_aware_fill_covers_the_canvas_and_keeps_the_photo() {
        let src = Rgb32f::from_fn(64, 48, |x, y| [0.2 + 0.3 * ((x / 4 + y / 4) % 2) as f32, 0.4, 0.1]);
        let rect = [-16.0, 0.0, 64.0, 48.0];
        let c = canvas(&src, rect, 1.0).unwrap();
        assert_eq!(c.hole.iter().filter(|h| **h).count(), 16 * 48);
        let filled = complete(&c, 1, &AtomicBool::new(false)).unwrap();
        let layer = patch(&c, filled, rect, 64, 48, 1.0, "contentAware", Default::default()).unwrap();
        // past the photo: filled with the photo's own colours, fully weighted
        for y in 0..48 {
            for x in 0..16 {
                let i = y * 80 + x;
                assert_eq!(layer.alpha.data[i], 1.0);
                assert!(layer.rgb.data[i].iter().all(|v| (0.05..0.65).contains(v)), "{:?}", layer.rgb.data[i]);
            }
        }
        // on the photo, past the blend at its edge: the photo itself, unweighted
        assert_eq!(layer.alpha.data[40], 0.0);
        assert_eq!(layer.rgb.data[40], src.data[24]);
        assert!((0.01..0.99).contains(&layer.alpha.data[18]), "blended at the edge: {}", layer.alpha.data[18]);
        assert!((layer.info.rect[0] + 0.25).abs() < 1e-9);
    }

    #[test]
    fn a_canvas_with_nothing_past_the_photo_is_refused() {
        let src = Rgb32f::from_fn(32, 32, |_, _| [0.5; 3]);
        assert!(canvas(&src, [0.0, 0.0, 32.0, 32.0], 1.0).is_err());
        assert!(canvas(&src, [-64.0, -64.0, -32.0, -32.0], 1.0).is_err());
        assert!(canvas(&src, [-1e9, -1e9, 1e9, 1e9], 1.0).is_err());
    }
}
