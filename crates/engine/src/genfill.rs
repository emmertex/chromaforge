//! Generative fill, the session's side (`docs/generative-fill.md`).
//!
//! * **Patches on disk.** A generated variation is a `.lcpatch` file in the library's `genfill`
//!   folder (`genfill/<photo id>/<generation id>-<n>.lcpatch`), never in the catalog or XMP; the
//!   spot's [`GenFill`] refers to it. Renders get the files through [`Session::genfill_store`]
//!   ([`GenStore`]), read on the render thread and kept in a small shared cache. A missing or
//!   damaged file renders the spot as a content-aware Fill and the panel offers Generate again.
//! * **Generating** ([`Session::genfill_start`]): only when the user clicked Generate, only to a
//!   provider whose terms they accepted ([`Session::genfill_consent`]), only with their own key.
//!   A worker thread loads the photo's full-size source, cuts the context crop around the spot
//!   ([`plan`]), sends it with the fixed transform ([`Neutral`]), brings the results back to
//!   linear as-shot light, matches them to the photo around the hole and writes the patches. The
//!   job holds an activity row with ✕ the whole time.
//! * **Keys and consent** live in the app's settings folder (see [`lcc_genfill::cloud::keys`]).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use lcc_catalog::{Photo, PhotoId};
use lcc_develop::{DevelopSettings, GenFill, Spot, SpotMode};
use lcc_genfill::PatchLayer;
#[cfg(not(target_arch = "wasm32"))]
use lcc_genfill::patch::PatchInfo;
use lcc_geom::Point;
use lcc_pipeline::genpatch::{GenStore, PatchSource};
use lcc_raster::{Plane, Rgb32f};

/// Name of the folder (in the library) the patches are kept in.
pub const DIR: &str = "genfill";
/// Most variations one Generate makes.
pub const MAX_VARIATIONS: u32 = 4;
/// Most requests one Generate sends (tiles × variations).
pub const MAX_REQUESTS: u32 = 16;
/// Largest patch file read.
const MAX_FILE: u64 = 1 << 30;
/// Bytes of patches kept in memory.
const CACHE_BYTES: usize = 1 << 30;

/// Patch files read so far, by path (`None`: missing or unreadable, re-checked after a while).
#[derive(Default)]
pub struct PatchCache {
    map: Mutex<HashMap<PathBuf, (Option<Arc<PatchLayer>>, web_time::Instant)>>,
}

impl PatchCache {
    fn get(&self, path: &Path) -> Option<Arc<PatchLayer>> {
        let mut map = self.map.lock().unwrap_or_else(PoisonError::into_inner);
        // answers are re-checked every few seconds: a file that appears or goes (deleted, an offline
        // drive) is noticed
        if let Some((hit, at)) = map.get_mut(path) {
            if at.elapsed().as_secs() < 3 {
                return hit.clone();
            }
            if hit.is_some() && path.is_file() {
                *at = web_time::Instant::now();
                return hit.clone();
            }
        }
        let read = read_patch(path).ok().map(Arc::new);
        let used: usize = map.values().filter_map(|(l, _)| l.as_ref()).map(|l| l.bytes()).sum();
        if used + read.as_ref().map_or(0, |l| l.bytes()) > CACHE_BYTES {
            map.clear();
        }
        map.insert(path.to_path_buf(), (read.clone(), web_time::Instant::now()));
        read
    }

    /// Forget `path` (it was just written).
    pub fn forget(&self, path: &Path) {
        self.map.lock().unwrap_or_else(PoisonError::into_inner).remove(path);
    }
}

/// Read a patch file (size-capped; a damaged file is an error).
pub fn read_patch(path: &Path) -> Result<PatchLayer, String> {
    let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if len > MAX_FILE {
        return Err("patch file too large".into());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    PatchLayer::from_bytes(&bytes).map_err(|e| e.to_string())
}

/// The patches of one photo, for renders.
struct PhotoPatches {
    dir: PathBuf,
    cache: Arc<PatchCache>,
}

impl PatchSource for PhotoPatches {
    fn patch(&self, id: &str, variant: u32) -> Option<Arc<PatchLayer>> {
        if !GenFill::valid_id(id) {
            return None;
        }
        self.cache.get(&self.dir.join(format!("{id}-{variant}.lcpatch")))
    }
}

/// What the user agreed to, per provider (`provider id → date`).
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct ConsentFile {
    accepted: BTreeMap<String, String>,
}

/// Where a generation stands.
#[derive(Clone, Debug, PartialEq)]
pub enum JobState {
    Running { done: u32, total: u32 },
    Done,
    Failed(String),
    Cancelled,
}

/// A running or finished generation.
pub struct Job {
    pub photo: PhotoId,
    pub spot: usize,
    pub stroke: u64,
    pub provider: String,
    state: Mutex<JobState>,
    result: Mutex<Option<Generated>>,
    cancel: Arc<AtomicBool>,
}

impl Job {
    pub fn state(&self) -> JobState {
        self.state.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    fn set(&self, s: JobState) {
        *self.state.lock().unwrap_or_else(PoisonError::into_inner) = s;
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// What a finished generation made.
#[derive(Clone, Debug)]
pub struct Generated {
    pub fill: GenFill,
    pub warnings: Vec<String>,
}

/// The session's generative fill state.
#[derive(Default)]
pub struct GenfillState {
    #[cfg(not(target_arch = "wasm32"))]
    pub keys: lcc_genfill::cloud::keys::KeyStore,
    consent_path: Option<PathBuf>,
    /// Consent given this session when there is no settings folder (tests, a disabled folder).
    consent_mem: BTreeMap<String, String>,
    /// Run generations in the background and apply them in [`Session::genfill_poll`] (the app);
    /// otherwise `spot.generate` waits (CLI, MCP, tests).
    pub background: bool,
    /// A local mock server to send to instead of the providers (tests; `LCC_GENFILL_MOCK`
    /// does the same for a whole process). Only loopback addresses are accepted.
    pub mock: Option<String>,
    pub cache: Arc<PatchCache>,
    pub job: Option<Arc<Job>>,
    /// The last finished job's warnings or error, for the panel.
    pub last: Option<Result<Generated, String>>,
}

impl GenfillState {
    fn consent_file(&self) -> ConsentFile {
        let Some(p) = &self.consent_path else { return ConsentFile { accepted: self.consent_mem.clone() } };
        std::fs::read(p).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    /// Whether the user accepted `provider`'s terms (the consent dialog).
    pub fn consented(&self, provider: &str) -> bool {
        self.consent_file().accepted.contains_key(provider)
    }

    pub fn set_consent(&mut self, provider: &str, accept: bool, today: &str) -> Result<(), String> {
        let mut f = self.consent_file();
        if accept {
            f.accepted.insert(provider.to_string(), today.to_string());
        } else {
            f.accepted.remove(provider);
        }
        match &self.consent_path {
            None => {
                self.consent_mem = f.accepted;
                Ok(())
            }
            Some(p) => {
                if let Some(d) = p.parent() {
                    std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
                }
                std::fs::write(p, serde_json::to_vec_pretty(&f).unwrap_or_default()).map_err(|e| format!("could not save the consent: {e}"))
            }
        }
    }

    pub fn running(&self) -> bool {
        self.job.as_ref().is_some_and(|j| matches!(j.state(), JobState::Running { .. }))
    }
}

// ------------------------------------------------------------------------------------------
// Planning the crop

/// The hole of a spot in source pixels and how it is sent.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// Source size (px) the plan is for.
    pub src_w: usize,
    pub src_h: usize,
    /// The stroke's dabs in source px: centre and hole radius (grown like Fill).
    pub dabs: Vec<(f64, f64, f64)>,
    /// Blend margin outside the hole and the ring of original pixels around it (px).
    pub feather: f64,
    pub ring: f64,
    pub opacity: f32,
    /// The patch rectangle (source px, `[x0, y0, x1, y1)`).
    pub rect: [usize; 4],
    /// What is sent: crops in source px with the size they are sent at, and the part of each a
    /// result is kept from.
    pub tiles: Vec<Tile>,
    /// Sent size ÷ native size (1 = native; below 1 the result is softer than the photo).
    pub scale: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    /// `[x0, y0, x1, y1)` in source px.
    pub crop: [usize; 4],
    pub send: (usize, usize),
    /// The part results are kept from (source px).
    pub core: [usize; 4],
}

/// Provider limits for [`plan`].
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_side: usize,
    /// Fixed sizes (the crop is cut to the nearest aspect), or none.
    pub sizes: &'static [(u32, u32)],
}

#[cfg(not(target_arch = "wasm32"))]
fn smooth(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0).max(1e-9)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Plan sending `spot` of a photo developed with `s` (orientation, lens corrections and geometry
/// place the stroke on the source), for a source of `src_w × src_h` px.
pub fn plan(
    spot: &Spot,
    s: &DevelopSettings,
    lens: Option<&lcc_develop::EmbeddedLens>,
    src_w: usize,
    src_h: usize,
    lim: Limits,
) -> Result<Plan, String> {
    if src_w < 16 || src_h < 16 {
        return Err("the photo is too small".into());
    }
    let frame = lcc_pipeline::geometry::Frame::with_lens(src_w, src_h, s, false, lens);
    let long = frame.ow.max(frame.oh);
    let r_t = spot.size * long;
    if !r_t.is_finite() || r_t <= 0.0 {
        return Err("the spot has no size".into());
    }
    let mut dabs = Vec::new();
    for p in spot.points.iter().take(20_000) {
        let c = lcc_pipeline::genpatch::transformed_to_source(&frame, *p);
        let ex = lcc_pipeline::genpatch::transformed_to_source(&frame, Point::new(p.x + r_t / frame.ow, p.y));
        let ey = lcc_pipeline::genpatch::transformed_to_source(&frame, Point::new(p.x, p.y + r_t / frame.oh));
        let r = (c.dist(ex) + c.dist(ey)) / 2.0;
        if c.x.is_finite() && c.y.is_finite() && r.is_finite() && r > 0.0 {
            dabs.push((c.x, c.y, r * f64::from(lcc_pipeline::spots::GROW) + 1.0));
        }
    }
    if dabs.is_empty() {
        return Err("the spot is outside the photo".into());
    }
    let r = dabs.iter().map(|d| d.2).fold(0.0f64, f64::max);
    let feather = (r * (0.08 + 0.25 * (spot.feather / 100.0).clamp(0.0, 1.0))).max(3.0);
    let ring = (r * 0.3).max(16.0);
    let pad = r + feather + ring;
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (x, y, _) in &dabs {
        (x0, y0, x1, y1) = (x0.min(x - pad), y0.min(y - pad), x1.max(x + pad), y1.max(y + pad));
    }
    let clampx = |v: f64| v.clamp(0.0, src_w as f64) as usize;
    let clampy = |v: f64| v.clamp(0.0, src_h as f64) as usize;
    let rect = [clampx(x0.floor()), clampy(y0.floor()), clampx(x1.ceil()), clampy(y1.ceil())];
    if rect[2] <= rect[0] + 2 || rect[3] <= rect[1] + 2 {
        return Err("the spot is outside the photo".into());
    }
    let (rw, rh) = (rect[2] - rect[0], rect[3] - rect[1]);
    // the send size: a fixed size of the nearest aspect, or up to the provider's longest side
    let send_for = |w: usize, h: usize| -> (usize, usize) {
        match lim.sizes.iter().min_by(|a, b| {
            let ar = w as f64 / h.max(1) as f64;
            let da = (f64::from(a.0) / f64::from(a.1) / ar).ln().abs();
            let db = (f64::from(b.0) / f64::from(b.1) / ar).ln().abs();
            da.total_cmp(&db)
        }) {
            Some(&(a, b)) => (a as usize, b as usize),
            None => (w.min(lim.max_side), h.min(lim.max_side)),
        }
    };
    // context: the hole and a generous margin, as much as the provider takes at native size
    let ctx = (rw.max(rh) as f64 * 0.75).max(256.0) as usize;
    let want = (rw + 2 * ctx, rh + 2 * ctx);
    let minimum = (rw + 2 * 48, rh + 2 * 48);
    let (sw, sh) = send_for(want.0, want.1);
    let fits = |w: usize, h: usize| minimum.0 <= w && minimum.1 <= h;
    let crop_around = |cw: usize, ch: usize, cx: f64, cy: f64| -> [usize; 4] {
        let (cw, ch) = (cw.min(src_w), ch.min(src_h));
        let x0 = (cx - cw as f64 / 2.0).clamp(0.0, (src_w - cw) as f64) as usize;
        let y0 = (cy - ch as f64 / 2.0).clamp(0.0, (src_h - ch) as f64) as usize;
        [x0, y0, x0 + cw, y0 + ch]
    };
    let (cx, cy) = ((rect[0] + rect[2]) as f64 / 2.0, (rect[1] + rect[3]) as f64 / 2.0);
    let whole = [0, 0, src_w, src_h];
    if fits(sw, sh) {
        // native: the crop is the send size in source pixels (a small photo is sent whole, upscaled)
        let crop = crop_around(sw, sh, cx, cy);
        let send = if lim.sizes.is_empty() { (crop[2] - crop[0], crop[3] - crop[1]) } else { (sw, sh) };
        let scale = (send.0 as f64 / (crop[2] - crop[0]) as f64).min(1.0);
        return Ok(Plan {
            src_w,
            src_h,
            dabs,
            feather,
            ring,
            opacity: (spot.opacity / 100.0).clamp(0.0, 1.0) as f32,
            rect,
            tiles: vec![Tile { crop, send, core: whole }],
            scale,
        });
    }
    // too big for one native crop: a thin hole (a wire, a fence) goes in native tiles along it
    let tile = send_for(lim.max_side, lim.max_side);
    let (tw, th) = (tile.0.min(src_w), tile.1.min(src_h));
    let (mx, my) = (tw / 5, th / 5);
    let (stx, sty) = ((tw - 2 * mx).max(1), (th - 2 * my).max(1));
    let mut area = 0.0;
    for (_, _, r) in &dabs {
        area += std::f64::consts::PI * r * r;
    }
    let thin = area / ((rw * rh) as f64) < 0.25;
    let mut tiles = Vec::new();
    if thin {
        let near =
            |b: [usize; 4]| dabs.iter().any(|(x, y, r)| *x + r > b[0] as f64 && *x - r < b[2] as f64 && *y + r > b[1] as f64 && *y - r < b[3] as f64);
        let mut ty = rect[1];
        while ty < rect[3] && tiles.len() <= 8 {
            let mut tx = rect[0];
            while tx < rect[2] && tiles.len() <= 8 {
                let core = [tx, ty, (tx + stx).min(rect[2]), (ty + sty).min(rect[3])];
                if near(core) {
                    let crop = crop_around(tw, th, (core[0] + core[2]) as f64 / 2.0, (core[1] + core[3]) as f64 / 2.0);
                    tiles.push(Tile { crop, send: if lim.sizes.is_empty() { (crop[2] - crop[0], crop[3] - crop[1]) } else { tile }, core });
                }
                tx += stx;
            }
            ty += sty;
        }
    }
    if thin && !tiles.is_empty() && tiles.len() <= 8 {
        return Ok(Plan { src_w, src_h, dabs, feather, ring, opacity: (spot.opacity / 100.0).clamp(0.0, 1.0) as f32, rect, tiles, scale: 1.0 });
    }
    // otherwise one downscaled crop: the hole with a little context, sent smaller (a softer patch)
    let (sw, sh) = send_for(minimum.0, minimum.1);
    let k = (sw as f64 / minimum.0 as f64).min(sh as f64 / minimum.1 as f64).min(1.0);
    let (cw, ch) = (((sw as f64 / k).ceil() as usize).min(src_w), ((sh as f64 / k).ceil() as usize).min(src_h));
    let crop = crop_around(cw, ch, cx, cy);
    let scale = (sw as f64 / (crop[2] - crop[0]) as f64).min(sh as f64 / (crop[3] - crop[1]) as f64).min(1.0);
    Ok(Plan {
        src_w,
        src_h,
        dabs,
        feather,
        ring,
        opacity: (spot.opacity / 100.0).clamp(0.0, 1.0) as f32,
        rect,
        tiles: vec![Tile { crop, send: (sw, sh), core: whole }],
        scale,
    })
}

impl Plan {
    /// Distance (px) from source pixel centre `(x, y)` to the nearest dab's hole edge (≤ 0 inside).
    fn field(&self) -> Plane {
        let [x0, y0, x1, y1] = self.rect;
        let (w, h) = (x1 - x0, y1 - y0);
        let mut d = Plane { width: w, height: h, data: vec![f32::MAX; w * h] };
        let reach = self.feather + self.ring + 2.0;
        for (cx, cy, r) in &self.dabs {
            let (ax, bx) = (((cx - r - reach).floor().max(x0 as f64)) as usize, ((cx + r + reach).ceil().min(x1 as f64)).max(0.0) as usize);
            let (ay, by) = (((cy - r - reach).floor().max(y0 as f64)) as usize, ((cy + r + reach).ceil().min(y1 as f64)).max(0.0) as usize);
            for y in ay..by {
                for x in ax..bx {
                    let v = ((x as f64 + 0.5 - cx).hypot(y as f64 + 0.5 - cy) - r) as f32;
                    if let Some(p) = d.data.get_mut((y - y0) * w + (x - x0))
                        && v < *p
                    {
                        *p = v;
                    }
                }
            }
        }
        d
    }

    /// Requests a generation of `variations` takes with `provider` (`remove`: no prompt).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn requests(&self, provider: lcc_genfill::cloud::Provider, variations: u32, remove: bool) -> u32 {
        if self.tiles.len() > 1 {
            // tiles go one after the other, one variation at a time
            self.tiles.len() as u32 * variations
        } else {
            lcc_genfill::cloud::requests_for(provider, variations, remove)
        }
    }
}

/// Crop `[x0,y0,x1,y1)` of `img` resized to `send`.
fn cut(img: &Rgb32f, c: [usize; 4], send: (usize, usize)) -> Rgb32f {
    cut_over(img, None, c, send)
}

/// [`cut`] with the patch area replaced by a working patch (`(plan, field, patch)`): what the
/// next tile of a tiled generation sees.
fn cut_over(img: &Rgb32f, over: Option<(&Plan, &Plane, &Rgb32f)>, c: [usize; 4], send: (usize, usize)) -> Rgb32f {
    let (w, h) = (c[2].saturating_sub(c[0]).max(1), c[3].saturating_sub(c[1]).max(1));
    let crop = Rgb32f::from_fn(w, h, |x, y| {
        let (sx, sy) = (c[0] + x, c[1] + y);
        if let Some((p, field, g)) = over
            && sx >= p.rect[0]
            && sy >= p.rect[1]
            && sx < p.rect[2]
            && sy < p.rect[3]
        {
            let i = (sy - p.rect[1]) * g.width + (sx - p.rect[0]);
            if field.data.get(i).is_some_and(|d| f64::from(*d) < p.feather)
                && let Some(v) = g.data.get(i)
            {
                return *v;
            }
        }
        img.data.get(sy * img.width + sx).copied().unwrap_or_default()
    });
    if (w, h) == send { crop } else { lcc_raster::resample::resize(&crop, send.0, send.1, lcc_raster::resample::Filter::Mitchell) }
}

/// The mask of a tile at its send size (255 = generate): hole pixels inside the tile's core.
fn tile_mask(p: &Plan, field: &Plane, t: &Tile) -> Vec<u8> {
    let (cw, ch) = ((t.crop[2] - t.crop[0]) as f64, (t.crop[3] - t.crop[1]) as f64);
    let (sw, sh) = t.send;
    let mut m = vec![0u8; sw * sh];
    let [rx0, ry0, rx1, ry1] = p.rect;
    // a send pixel is masked when any source pixel under it is in the hole (so nothing is missed)
    let k = (cw / sw as f64).max(ch / sh as f64).max(1.0);
    for y in 0..sh {
        for x in 0..sw {
            let (sx, sy) = (t.crop[0] as f64 + (x as f64 + 0.5) * cw / sw as f64, t.crop[1] as f64 + (y as f64 + 0.5) * ch / sh as f64);
            let (ix, iy) = (sx as usize, sy as usize);
            if ix < rx0 || iy < ry0 || ix >= rx1 || iy >= ry1 || ix < t.core[0] || iy < t.core[1] || ix >= t.core[2] || iy >= t.core[3] {
                continue;
            }
            let d = field.data.get((iy - ry0) * field.width + (ix - rx0)).copied().unwrap_or(f32::MAX);
            if f64::from(d) < k + 1.0
                && let Some(v) = m.get_mut(y * sw + x)
            {
                *v = 255;
            }
        }
    }
    m
}

#[cfg(not(target_arch = "wasm32"))]
/// One request's results for a tile: decoded to linear light at the crop's native size.
fn receive(img: &lcc_genfill::cloud::Image, neutral: lcc_genfill::Neutral, t: &Tile) -> Option<Rgb32f> {
    let lin = neutral.decode(img.width, img.height, &img.rgb)?;
    let (cw, ch) = (t.crop[2] - t.crop[0], t.crop[3] - t.crop[1]);
    Some(if (lin.width, lin.height) == (cw, ch) {
        lin
    } else {
        lcc_raster::resample::resize(&lin, cw, ch, lcc_raster::resample::Filter::Mitchell)
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// Paste a tile's result into the working patch `gen` (rect-sized) where the hole and its margin are.
fn paste(p: &Plan, field: &Plane, t: &Tile, res: &Rgb32f, gen_img: &mut Rgb32f) {
    let [rx0, ry0, rx1, ry1] = p.rect;
    let reach = (p.feather + p.ring * 0.5) as f32;
    for y in t.crop[1].max(ry0).max(t.core[1])..t.crop[3].min(ry1).min(t.core[3]) {
        for x in t.crop[0].max(rx0).max(t.core[0])..t.crop[2].min(rx1).min(t.core[2]) {
            let i = (y - ry0) * gen_img.width + (x - rx0);
            if field.data.get(i).is_some_and(|d| *d < reach)
                && let (Some(g), Some(v)) = (gen_img.data.get_mut(i), res.data.get((y - t.crop[1]) * res.width + (x - t.crop[0])))
            {
                *g = *v;
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// The finished patch of one variation: matched to the photo, with its blend weight.
fn finish(
    src: &Rgb32f,
    p: &Plan,
    field: &Plane,
    mut gen_img: Rgb32f,
    provider: &str,
    neutral: lcc_genfill::Neutral,
) -> Result<PatchLayer, String> {
    let [rx0, ry0, rx1, ry1] = p.rect;
    let orig = Rgb32f::from_fn(rx1 - rx0, ry1 - ry0, |x, y| src.data.get((ry0 + y) * src.width + rx0 + x).copied().unwrap_or_default());
    let area: Vec<bool> = field.data.iter().map(|d| f64::from(*d) < p.feather).collect();
    let ring: Vec<bool> = field.data.iter().map(|d| f64::from(*d) >= p.feather && f64::from(*d) < p.feather + p.ring).collect();
    let _ = lcc_genfill::matching::match_to_surroundings(&mut gen_img, &orig, &area, &ring);
    // outside the blend area the patch is the photo itself (its weight is 0 there)
    for (i, a) in area.iter().enumerate() {
        if !a && let (Some(g), Some(o)) = (gen_img.data.get_mut(i), orig.data.get(i)) {
            *g = *o;
        }
    }
    let alpha = Plane {
        width: field.width,
        height: field.height,
        data: field.data.iter().map(|d| ((1.0 - smooth(0.0, p.feather, f64::from(*d))) as f32) * p.opacity).collect(),
    };
    let (sw, sh) = (p.src_w as f64, p.src_h as f64);
    let info = PatchInfo {
        rect: [rx0 as f64 / sw, ry0 as f64 / sh, rx1 as f64 / sw, ry1 as f64 / sh],
        source_w: p.src_w as u32,
        source_h: p.src_h as u32,
        scale: p.scale,
        provider: provider.to_string(),
        neutral,
        ..Default::default()
    };
    PatchLayer::new(info, gen_img, alpha).map_err(|e| e.to_string())
}

/// Write `layer` to `path` (a temporary name, then renamed).
pub fn write_patch(path: &Path, layer: &PatchLayer) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("could not create the generative fill folder: {e}"))?;
    }
    let tmp = path.with_extension("lcpatch.tmp");
    std::fs::write(&tmp, layer.to_bytes()).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("could not save the generated patch: {e}")
    })?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("could not save the generated patch: {e}")
    })
}

/// A new generation id: 16 hex digits from the time, the process and a counter.
pub fn new_id(salt: u64) -> String {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let t = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    let mut h = 0xcbf2_9ce4_8422_2325u64
        ^ t
        ^ salt.rotate_left(17)
        ^ u64::from(std::process::id()) << 40
        ^ N.fetch_add(1, Ordering::Relaxed).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    format!("{:016x}", h ^ (h >> 31))
}

/// The encoded crop a generation would send first (the consent dialog shows it), its mask and
/// the transform: `(w, h, rgb8, mask)`.
pub fn preview_crop(src: &Rgb32f, p: &Plan) -> Option<(usize, usize, Vec<u8>, Vec<u8>)> {
    let t = p.tiles.first()?;
    let field = p.field();
    let crop = cut(src, t.crop, t.send);
    let neutral = lcc_genfill::Neutral::for_crop(&crop);
    Some((t.send.0, t.send.1, neutral.encode(&crop), tile_mask(p, &field, t)))
}

/// What a generation needs, taken from the session on its thread.
#[cfg(not(target_arch = "wasm32"))]
pub struct Work {
    pub provider: lcc_genfill::cloud::Provider,
    pub key: String,
    pub endpoint: lcc_genfill::cloud::Endpoint,
    pub spot: Spot,
    pub settings: Arc<DevelopSettings>,
    pub source: crate::media::SourceRef,
    pub header: lcc_pipeline::SourceInfo,
    pub prompt: String,
    pub variations: u32,
    pub dir: PathBuf,
    pub id: String,
}

/// Run a generation (on any thread): load the full-size source, send the crop(s), write the
/// patches. `progress(done, total)` after each request.
#[cfg(not(target_arch = "wasm32"))]
pub fn run(w: &Work, cancel: &AtomicBool, cache: &PatchCache, progress: &dyn Fn(u32, u32)) -> Result<Generated, String> {
    use lcc_genfill::cloud::{Control, Request, generate};
    let source = w.source.load_source()?;
    let src = source.image.clone();
    let info = source.info_or(w.header.clone());
    let lim = Limits { max_side: w.provider.info().max_side as usize, sizes: w.provider.info().sizes };
    let p = plan(&w.spot, &w.settings, info.lens.as_ref(), src.width, src.height, lim)?;
    let remove = w.prompt.trim().is_empty();
    let n = w.variations.clamp(1, MAX_VARIATIONS);
    let total = p.requests(w.provider, n, remove);
    if total > MAX_REQUESTS {
        return Err(format!("this would take {total} requests; paint a smaller area or ask for fewer variations"));
    }
    let field = p.field();
    let [rx0, ry0, rx1, ry1] = p.rect;
    let base = Rgb32f::from_fn(rx1 - rx0, ry1 - ry0, |x, y| src.data.get((ry0 + y) * src.width + rx0 + x).copied().unwrap_or_default());
    let ctl = Control { cancel, stall: std::time::Duration::from_secs(180), deadline: std::time::Duration::from_secs(600) };
    let mut gens: Vec<Rgb32f> = vec![base.clone(); n as usize];
    let mut neutral = lcc_genfill::Neutral::default();
    let mut done = 0u32;
    progress(0, total);
    let seed = u64::from_str_radix(&w.id, 16).unwrap_or(1);
    if let [t] = p.tiles.as_slice() {
        let crop = cut(&src, t.crop, t.send);
        neutral = lcc_genfill::Neutral::for_crop(&crop);
        let req = Request {
            width: t.send.0,
            height: t.send.1,
            rgb: neutral.encode(&crop),
            mask: tile_mask(&p, &field, t),
            prompt: w.prompt.clone(),
            variations: n,
            seed,
        };
        let out = generate(w.provider, &w.key, &w.endpoint, &req, &ctl).map_err(|e| e.to_string())?;
        progress(total, total);
        for (k, img) in out.iter().take(n as usize).enumerate() {
            let res = receive(img, neutral, t).ok_or("the provider sent an unreadable image")?;
            if let Some(g) = gens.get_mut(k) {
                paste(&p, &field, t, &res, g);
            }
        }
        gens.truncate(out.len().min(n as usize));
    } else {
        for g in gens.iter_mut() {
            for t in &p.tiles {
                if cancel.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                // each tile sees the tiles before it filled
                let crop = cut_over(&src, Some((&p, &field, g)), t.crop, t.send);
                neutral = lcc_genfill::Neutral::for_crop(&crop);
                let mask = tile_mask(&p, &field, t);
                if !mask.iter().any(|m| *m > 0) {
                    continue;
                }
                let req = Request {
                    width: t.send.0,
                    height: t.send.1,
                    rgb: neutral.encode(&crop),
                    mask,
                    prompt: w.prompt.clone(),
                    variations: 1,
                    seed: seed.wrapping_add(u64::from(done)),
                };
                let out = generate(w.provider, &w.key, &w.endpoint, &req, &ctl).map_err(|e| e.to_string())?;
                let img = out.first().ok_or("the provider sent no image")?;
                let res = receive(img, neutral, t).ok_or("the provider sent an unreadable image")?;
                paste(&p, &field, t, &res, g);
                done += 1;
                progress(done, total);
            }
        }
    }
    if gens.is_empty() {
        return Err("the provider sent no image".into());
    }
    let photo_dir = &w.dir;
    let variants = gens.len() as u32;
    let fill = GenFill {
        id: w.id.clone(),
        variants,
        variant: 0,
        provider: w.provider.info().id.to_string(),
        prompt: w.prompt.trim().to_string(),
        stroke: w.spot.stroke_key(),
        scale: p.scale,
    };
    for (k, g) in gens.into_iter().enumerate() {
        let layer = finish(&src, &p, &field, g, w.provider.info().id, neutral)?;
        let path = photo_dir.join(fill.file_name(k as u32));
        write_patch(&path, &layer)?;
        cache.forget(&path);
    }
    let mut warnings = Vec::new();
    if p.scale < 0.95 {
        warnings.push(format!(
            "generated at {:.0} % of the photo's resolution: the filled area is softer than the rest (paint a smaller area for a sharper fill)",
            p.scale * 100.0
        ));
    }
    if w.endpoint.is_mock() {
        warnings.push("made by the local mock server (LCC_GENFILL_MOCK), not a provider".into());
    }
    Ok(Generated { fill, warnings })
}

impl crate::Session {
    /// Keep API keys and consent in the app's settings folder ([`crate::config::config_dir`]).
    pub fn with_default_genfill(mut self) -> Self {
        let dir = crate::config::config_dir();
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.genfill.keys = lcc_genfill::cloud::keys::KeyStore::new(dir.as_ref().map(|d| d.join("genfill-keys.json")));
        }
        self.genfill.consent_path = dir.map(|d| d.join("genfill-consent.json"));
        self
    }

    /// The folder photo `id`'s patches are kept in: the library's `genfill/<id>`, or (a library
    /// that is not on disk) a folder in the system's temporary folder.
    pub fn genfill_dir(&self, id: PhotoId) -> PathBuf {
        let root =
            self.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.join(DIR)).unwrap_or_else(|| std::env::temp_dir().join("lcc-genfill"));
        root.join(id.0.to_string())
    }

    /// The patches photo `p` shows, for its renders (`None` when no spot shows one).
    pub fn genfill_store(&self, p: &Photo) -> Option<GenStore> {
        if !p.develop.spots.iter().any(|s| s.shown_gen().is_some()) {
            return None;
        }
        Some(GenStore(Arc::new(PhotoPatches { dir: self.genfill_dir(p.id), cache: self.genfill.cache.clone() })))
    }

    /// Whether spot `index` of photo `id` shows a patch that can't be read (missing file).
    pub fn genfill_missing(&self, id: PhotoId, index: usize) -> bool {
        let Some(p) = self.catalog.photo(id) else { return false };
        let Some(g) = p.develop.spots.get(index).and_then(|s| s.shown_gen()) else { return false };
        !self.genfill_dir(id).join(g.file_name(g.variant)).is_file()
    }

    /// The first crop generating spot `index` with `provider` would send, as it is sent (8-bit,
    /// through the fixed transform) and its mask, from the photo's preview-size source (the
    /// framing is the real one; the real crop has the photo's full resolution).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn genfill_preview(
        &mut self,
        id: PhotoId,
        index: usize,
        provider: lcc_genfill::cloud::Provider,
    ) -> Result<(usize, usize, Vec<u8>, Vec<u8>), String> {
        let p = self.catalog.photo(id).ok_or("no such photo")?.clone();
        let spot = p.develop.spots.get(index).cloned().ok_or("no such spot")?;
        let src = self.source_now(id, crate::media::SourceLevel::Preview)?;
        // the native plan, scaled to this source (so the framing is the one that will be sent)
        let (nw, nh) = (p.width.max(1) as usize, p.height.max(1) as usize);
        let lim = Limits { max_side: provider.info().max_side as usize, sizes: provider.info().sizes };
        let native = plan(&spot, &p.develop, p.embedded_lens.as_deref(), nw, nh, lim)?;
        let k = src.width as f64 / nw as f64;
        let sc = |v: usize, max: usize| ((v as f64 * k).round() as usize).min(max);
        let mut small = native.clone();
        small.src_w = src.width;
        small.src_h = src.height;
        small.dabs = native.dabs.iter().map(|(x, y, r)| (x * k, y * k, r * k)).collect();
        small.feather *= k;
        small.ring *= k;
        small.rect = [sc(native.rect[0], src.width), sc(native.rect[1], src.height), sc(native.rect[2], src.width), sc(native.rect[3], src.height)];
        small.tiles = native
            .tiles
            .iter()
            .map(|t| {
                let crop = [sc(t.crop[0], src.width), sc(t.crop[1], src.height), sc(t.crop[2], src.width), sc(t.crop[3], src.height)];
                let (cw, ch) = ((crop[2] - crop[0]).max(1), (crop[3] - crop[1]).max(1));
                // at most the crop's own pixels at this size (a preview is never upscaled)
                let f = (cw as f64 / t.send.0 as f64).min(ch as f64 / t.send.1 as f64).min(1.0);
                Tile {
                    crop,
                    send: (((t.send.0 as f64 * f).round() as usize).max(1), ((t.send.1 as f64 * f).round() as usize).max(1)),
                    core: [sc(t.core[0], src.width), sc(t.core[1], src.height), sc(t.core[2], src.width), sc(t.core[3], src.height)],
                }
            })
            .collect();
        preview_crop(&src, &small).ok_or_else(|| "nothing to send".into())
    }

    /// [`Session::genfill_preview`] by provider id (an error where generative fill isn't available).
    pub fn genfill_preview_by_id(&mut self, id: PhotoId, index: usize, provider: &str) -> Result<(usize, usize, Vec<u8>, Vec<u8>), String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let pr = lcc_genfill::cloud::Provider::from_id(provider).ok_or("unknown provider")?;
            self.genfill_preview(id, index, pr)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (id, index, provider);
            Err("generative fill is not available in the web version".into())
        }
    }

    /// Accept (or withdraw) `provider`'s terms.
    pub fn genfill_consent(&mut self, provider: &str, accept: bool) -> Result<(), String> {
        let today: String = (self.clock)().chars().take(10).collect();
        self.genfill.set_consent(provider, accept, &today)
    }

    /// Start generating spot `index` of photo `id` (see the module docs); waits unless
    /// [`GenfillState::background`]. Returns the finished generation when it waited.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn genfill_start(
        &mut self,
        id: PhotoId,
        index: usize,
        provider: lcc_genfill::cloud::Provider,
        prompt: &str,
        variations: u32,
    ) -> Result<Option<Generated>, String> {
        use crate::activity::Cancel;
        let pid = provider.info().id;
        if self.genfill.running() {
            return Err("a generative fill is already running; wait for it or cancel it".into());
        }
        if !self.genfill.consented(pid) {
            return Err(format!("accept {}'s terms first (the consent dialog, or genfill.consent)", provider.info().name));
        }
        let (key, _) = self.genfill.keys.get(provider).ok_or_else(|| lcc_genfill::cloud::GenError::NoKey.to_string())?;
        let p = self.catalog.photo(id).ok_or("no such photo")?.clone();
        let spot = p.develop.spots.get(index).cloned().ok_or("no such spot")?;
        if spot.points.is_empty() {
            return Err("the spot has no area".into());
        }
        if prompt.chars().count() > 1000 {
            return Err("the prompt is too long (at most 1000 characters)".into());
        }
        let work = Work {
            provider,
            key,
            endpoint: match &self.genfill.mock {
                Some(m) => lcc_genfill::cloud::Endpoint::mock(m)?,
                None => lcc_genfill::cloud::Endpoint::for_provider(provider),
            },
            spot: spot.clone(),
            settings: p.develop.clone(),
            source: self.media.source_ref_uncached(&p, crate::media::SourceLevel::Full),
            header: crate::media::source_info(&p),
            prompt: prompt.trim().to_string(),
            variations: variations.clamp(1, MAX_VARIATIONS),
            dir: self.genfill_dir(id),
            id: new_id(id.0),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let job = Arc::new(Job {
            photo: id,
            spot: index,
            stroke: spot.stroke_key(),
            provider: pid.to_string(),
            state: Mutex::new(JobState::Running { done: 0, total: 0 }),
            result: Mutex::new(None),
            cancel: cancel.clone(),
        });
        let guard = self.activity.start("genfill", "Generating fill", Cancel::Flag(cancel.clone()));
        guard.detail(provider.info().name);
        let handle = guard.handle();
        let cache = self.genfill.cache.clone();
        let j = job.clone();
        let body = move || {
            let progress = |d: u32, t: u32| {
                handle.progress(u64::from(d), u64::from(t));
                j.set(JobState::Running { done: d, total: t });
            };
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&work, &cancel, &cache, &progress)))
                .unwrap_or_else(|_| Err("the generative fill stopped unexpectedly".into()));
            match r {
                Ok(g) => {
                    *j.result.lock().unwrap_or_else(PoisonError::into_inner) = Some(g);
                    j.set(JobState::Done);
                }
                Err(_) if cancel.load(Ordering::Relaxed) => j.set(JobState::Cancelled),
                Err(e) => j.set(JobState::Failed(e)),
            }
        };
        self.genfill.job = Some(job.clone());
        if self.genfill.background {
            std::thread::Builder::new()
                .name("genfill".into())
                .spawn(move || {
                    let _guard = guard;
                    body();
                })
                .map_err(|e| format!("could not start the generative fill: {e}"))?;
            return Ok(None);
        }
        body();
        drop(guard);
        match self.genfill_poll().into_iter().next() {
            Some(Err(e)) => Err(e),
            Some(Ok(g)) => Ok(Some(g)),
            None => Err("the generative fill ended without a result".into()),
        }
    }

    /// Apply a finished generation to its spot (one undo step) and report it. Cheap when nothing
    /// is running: the app calls it every frame.
    pub fn genfill_poll(&mut self) -> Option<Result<Generated, String>> {
        let job = self.genfill.job.clone()?;
        let out = match job.state() {
            JobState::Running { .. } => return None,
            JobState::Cancelled => Err("cancelled".to_string()),
            JobState::Failed(e) => Err(e),
            JobState::Done => {
                let g = job.result.lock().unwrap_or_else(PoisonError::into_inner).take();
                match g {
                    None => Err("the generative fill ended without a result".into()),
                    Some(g) => self.genfill_apply(&job, &g).map(|()| g),
                }
            }
        };
        self.genfill.job = None;
        self.genfill.last = Some(out.clone());
        Some(out)
    }

    fn genfill_apply(&mut self, job: &Job, g: &Generated) -> Result<(), String> {
        let mut d = (*self.develop_of(job.photo).ok_or("the photo is gone")?).clone();
        let sp = d
            .spots
            .get_mut(job.spot)
            .filter(|s| s.stroke_key() == job.stroke)
            .ok_or("the spot changed while it was generating; generate it again")?;
        sp.mode = SpotMode::Generate;
        sp.generated = Some(g.fill.clone());
        self.set_develop(job.photo, d, "Generative Fill").map_err(|e| e.to_string())
    }

    /// Cancel the running generation (if any).
    pub fn genfill_cancel(&mut self) -> bool {
        match &self.genfill.job {
            Some(j) if matches!(j.state(), JobState::Running { .. }) => {
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

    fn spot(x: f64, y: f64, size: f64) -> Spot {
        Spot { mode: SpotMode::Generate, points: vec![Point::new(x, y)], size, ..Default::default() }
    }

    const OPENAI: Limits = Limits { max_side: 1536, sizes: &[(1024, 1024), (1536, 1024), (1024, 1536)] };
    const FREE: Limits = Limits { max_side: 2048, sizes: &[] };

    #[test]
    fn a_small_hole_is_sent_at_native_size_with_context() {
        let s = DevelopSettings::default();
        let p = plan(&spot(0.5, 0.5, 0.02), &s, None, 6000, 4000, OPENAI).unwrap();
        assert_eq!(p.tiles.len(), 1);
        let t = &p.tiles[0];
        assert_eq!(t.send, (t.crop[2] - t.crop[0], t.crop[3] - t.crop[1]), "native: one source pixel per sent pixel");
        assert!(matches!(t.send, (1024, 1024) | (1536, 1024)));
        assert!((p.scale - 1.0).abs() < 1e-9);
        // the crop holds the whole patch rectangle
        assert!(t.crop[0] <= p.rect[0] && t.crop[2] >= p.rect[2] && t.crop[1] <= p.rect[1] && t.crop[3] >= p.rect[3]);
        let free = plan(&spot(0.5, 0.5, 0.02), &s, None, 6000, 4000, FREE).unwrap();
        assert!(free.tiles[0].crop[2] - free.tiles[0].crop[0] <= 2048);
    }

    #[test]
    fn a_big_hole_is_downscaled_and_a_long_thin_one_is_tiled() {
        let s = DevelopSettings::default();
        let big = plan(&spot(0.5, 0.5, 0.2), &s, None, 6000, 4000, OPENAI).unwrap();
        assert_eq!(big.tiles.len(), 1);
        assert!(big.scale < 0.95, "{}", big.scale);
        // a wire across the frame
        let wire = Spot { points: (0..60).map(|i| Point::new(0.1 + i as f64 * 0.013, 0.3)).collect(), ..spot(0.0, 0.0, 0.003) };
        let w = plan(&wire, &s, None, 6000, 4000, FREE).unwrap();
        assert!(w.tiles.len() > 1 && w.tiles.len() <= 8, "{}", w.tiles.len());
        assert!((w.scale - 1.0).abs() < 1e-9);
        // tile cores cover every dab
        for (x, y, _) in &w.dabs {
            assert!(w.tiles.iter().any(|t| (t.core[0] as f64..t.core[2] as f64).contains(x) && (t.core[1] as f64..t.core[3] as f64).contains(y)));
        }
    }

    #[test]
    fn a_rotated_photo_places_the_hole_on_the_source() {
        let s = DevelopSettings { orientation: lcc_geom::Orientation::Rotate90, ..Default::default() };
        // top-left of the rotated view = bottom-left of the stored image
        let p = plan(&spot(0.1, 0.1, 0.01), &s, None, 6000, 4000, FREE).unwrap();
        let (x, y, _) = p.dabs[0];
        assert!(x < 1000.0 && y > 3000.0, "{x} {y}");
    }

    #[test]
    fn hostile_spots_are_errors() {
        let s = DevelopSettings::default();
        for sp in [
            spot(f64::NAN, 0.5, 0.02),
            spot(5.0, 5.0, 0.02),
            spot(0.5, 0.5, 0.0),
            spot(0.5, 0.5, f64::INFINITY),
            Spot { points: vec![], ..spot(0.5, 0.5, 0.02) },
        ] {
            assert!(plan(&sp, &s, None, 6000, 4000, OPENAI).is_err(), "{sp:?}");
        }
        assert!(plan(&spot(0.5, 0.5, 0.02), &s, None, 8, 8, OPENAI).is_err());
        // a photo smaller than the provider's size is sent whole
        let p = plan(&spot(0.5, 0.5, 0.05), &s, None, 640, 480, OPENAI).unwrap();
        assert_eq!(p.tiles[0].crop, [0, 0, 640, 480]);
    }

    #[test]
    fn ids_are_hex_and_differ() {
        let (a, b) = (new_id(1), new_id(1));
        assert!(GenFill::valid_id(&a) && GenFill::valid_id(&b) && a != b);
    }
}
