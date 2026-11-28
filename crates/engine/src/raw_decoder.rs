//! Which raw decoder reads each camera body's files: **Auto**, the app's own decoder (`lcc-raw`) or **LibRaw**.
//!
//! The person's choice is kept per camera (the library label, [`lcc_catalog::camera_label`] of the file's Exif make
//! and model) in the library preferences (`library.preferences` `rawDecoder`) and installed here
//! ([`install_choices`]), the way the default camera profiles are. Only the reading of the file changes: whichever
//! decoder reads it, the mosaic is demosaiced, coloured and developed by the app's own (pure Rust) pipeline.
//!
//! LibRaw itself is not part of the app (it is C++): a host that can run it hands a [`Backend`] to
//! [`install_libraw`]. Where none is installed (the web build, Android, a desktop without it) a camera set to LibRaw
//! is read by `lcc-raw`, and the Settings panel says LibRaw is not available.
//!
//! - **Auto** (the default, and every camera without a choice): `lcc-raw`, exactly as before; a file it refuses as an
//!   unsupported variant goes to LibRaw when LibRaw is available, else to the embedded preview as before.
//! - **lcc-raw**: `lcc-raw` only, never LibRaw.
//! - **LibRaw**: LibRaw first; when it fails (or is not available) the file is read by `lcc-raw`.
//!
//! A backend returns the file's mosaic as a [`Frame`]; [`Frame::into_raw_image`] checks it as hostile input and fills
//! in what LibRaw doesn't say (metadata, orientation, embedded lens corrections, default crop) from `lcc-raw`'s
//! header probe of the same file when that agrees on the sensor geometry.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, PoisonError, RwLock};

use serde::{Deserialize, Serialize};

/// A camera's raw decoder choice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decoder {
    /// `lcc-raw`, with LibRaw for the files it can't read (when LibRaw is available).
    #[default]
    Auto,
    /// The app's own decoder only.
    Lcc,
    /// LibRaw, with `lcc-raw` when LibRaw fails or is not available.
    Libraw,
}

impl Decoder {
    pub const ALL: [Decoder; 3] = [Decoder::Auto, Decoder::Lcc, Decoder::Libraw];

    /// The id commands and `prefs.json` use.
    pub fn id(self) -> &'static str {
        match self {
            Decoder::Auto => "auto",
            Decoder::Lcc => "lcc",
            Decoder::Libraw => "libraw",
        }
    }

    /// An id as a person may type it (`auto`, `lcc`, `lcc-raw`, `lccraw`, `libraw`), any case.
    pub fn parse(s: &str) -> Option<Decoder> {
        match s.trim().to_ascii_lowercase().replace(['-', '_', ' '], "").as_str() {
            "auto" | "" | "default" => Some(Decoder::Auto),
            "lcc" | "lccraw" | "builtin" => Some(Decoder::Lcc),
            "libraw" => Some(Decoder::Libraw),
            _ => None,
        }
    }
}

/// A raw file's mosaic as an external decoder (LibRaw) unpacked it, before any processing: no black subtraction,
/// scaling, white balance or demosaic.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    /// Full sensor data dimensions, masked borders included (LibRaw `raw_width` × `raw_height`).
    pub width: usize,
    pub height: usize,
    /// `width × height` samples, row-major.
    pub data: Vec<u16>,
    /// The colour filter pattern anchored at sample (0, 0); `None` for a monochrome sensor.
    pub cfa: Option<lcc_raw::Cfa>,
    /// Significant bits of the samples.
    pub bits: u32,
    /// Black level, anchored at the active area's top-left corner (LibRaw `black` + `cblack`).
    pub black: lcc_raw::BlackLevel,
    /// Clip level (LibRaw `maximum`).
    pub white: f32,
    /// The image area within the data (LibRaw `left_margin`, `top_margin`, `width`, `height`).
    pub active_area: lcc_raw::Rect,
    /// As-shot white balance multipliers, green = 1 (LibRaw `cam_mul`), when known.
    pub wb_multipliers: Option<[f32; 3]>,
    /// XYZ (D65) → camera matrix (LibRaw `cam_xyz`), when the decoder knows one; used only when the app has none.
    pub xyz_to_camera: Option<lcc_raw::Mat3>,
}

/// The largest frame accepted from a backend (samples): 600 MP, above any sensor made.
const MAX_SAMPLES: usize = 600_000_000;

impl Frame {
    /// The frame as the app's [`lcc_raw::RawImage`]: checked (sizes, CFA, levels), with the file's metadata,
    /// orientation, opcodes and crop taken from `lcc-raw`'s header probe of `bytes` when it reads the same sensor
    /// area, else from its Exif.
    pub fn into_raw_image(self, bytes: &[u8]) -> Result<lcc_raw::RawImage, String> {
        let Frame { width, height, data, cfa, bits, black, white, active_area, wb_multipliers, xyz_to_camera } = self;
        let samples = width.checked_mul(height).ok_or("LibRaw frame size overflows")?;
        if width == 0 || height == 0 || samples > MAX_SAMPLES {
            return Err(format!("LibRaw frame is {width} x {height}, outside the sizes the app reads"));
        }
        if data.len() != samples {
            return Err(format!("LibRaw frame has {} samples for {width} x {height}", data.len()));
        }
        if let Some(c) = &cfa {
            let cells = c.width.checked_mul(c.height).unwrap_or(0);
            if c.width == 0 || c.height == 0 || c.width > 16 || c.height > 16 || c.pattern.len() != cells || c.pattern.iter().any(|&p| p > 2) {
                return Err("LibRaw frame has an unusable colour filter pattern".into());
            }
        }
        let a = active_area;
        let fits = a.width > 0
            && a.height > 0
            && a.x.checked_add(a.width).is_some_and(|r| r <= width)
            && a.y.checked_add(a.height).is_some_and(|b| b <= height);
        if !fits {
            return Err("LibRaw frame's image area lies outside its data".into());
        }
        let black_ok = black.repeat_rows > 0
            && black.repeat_cols > 0
            && black.repeat_rows <= 16
            && black.repeat_cols <= 16
            && black.values.len() == black.repeat_rows * black.repeat_cols
            && black.values.iter().chain(&black.delta_h).chain(&black.delta_v).all(|v| v.is_finite());
        if !black_ok || !white.is_finite() || white <= black.mean() || white > 65535.0 {
            return Err(format!("LibRaw frame has unusable levels (black {}, white {white})", black.mean()));
        }
        let wb_multipliers = wb_multipliers.filter(|m| m.iter().all(|v| v.is_finite() && *v > 0.0));
        let xyz_to_camera = xyz_to_camera.filter(|m| m.0.iter().flatten().all(|v| v.is_finite()) && m.0.iter().flatten().any(|v| *v != 0.0));

        // what LibRaw doesn't describe comes from the app's own reading of the headers, when it sees the same area
        let header = lcc_raw::probe_info(bytes).ok();
        let same_area = header.as_ref().is_some_and(|h| h.active_area.width == a.width && h.active_area.height == a.height);
        let metadata = header.as_ref().map(|h| h.metadata.clone()).unwrap_or_else(|| lcc_meta::extract(bytes));
        let orientation = header.as_ref().map(|h| h.orientation).or(metadata.orientation).unwrap_or(lcc_raw::Orientation::Normal);
        let format = header.as_ref().map(|h| h.format).or_else(|| lcc_raw::probe(bytes)).unwrap_or(lcc_raw::RawFormat::OtherTiff);
        let mut color = header.as_ref().map(|h| h.color.clone()).unwrap_or_default();
        // LibRaw's matrix is the last resort: the file's own comes first, then the app's camera table (applied later, and
        // only when the file has none), so a body the app knows keeps its measured matrices on this path as on lcc-raw's
        if !lcc_raw::color::has_matrix(&color)
            && !app_table_has_matrix(metadata.make.as_deref(), metadata.model.as_deref())
            && let Some(m) = xyz_to_camera
        {
            color.color_matrix = [Some(m), None];
            color.illuminant = [21, 0]; // D65
        }
        let (crop, opcodes, lens_metadata, pixel_aspect) = match header.filter(|_| same_area) {
            Some(h) => (h.crop, h.opcodes, h.lens_metadata, h.pixel_aspect),
            None => (lcc_raw::Rect::new(0, 0, a.width, a.height), Default::default(), Default::default(), 1.0),
        };
        let wb_multipliers = wb_multipliers.map(|m| {
            let g = if m[1] > 0.0 { m[1] } else { 1.0 };
            [m[0] / g, 1.0, m[2] / g]
        });
        Ok(lcc_raw::RawImage {
            format,
            width,
            height,
            cpp: 1,
            data: lcc_raw::RawData::U16(data),
            cfa,
            bits: bits.clamp(1, 16),
            black,
            white: vec![white],
            active_area: a,
            crop: crop.clipped(a.width, a.height),
            orientation,
            color,
            wb_multipliers,
            linearized: false,
            pixel_aspect: if pixel_aspect.is_finite() && pixel_aspect > 0.0 { pixel_aspect } else { 1.0 },
            opcodes,
            metadata,
            lens_metadata,
        })
    }
}

/// Whether the app's own camera table has colour matrices for this body (`lcc-camdb`'s database and the measured spectral
/// table, as the colour model reads them): then a raw without matrices of its own takes those, not LibRaw's.
fn app_table_has_matrix(make: Option<&str>, model: Option<&str>) -> bool {
    let Some(model) = model else { return false };
    lcc_raw::spectral::find(make, model).is_some() || lcc_camdb::find(make, model).is_some_and(|e| lcc_raw::color::has_matrix(&e.color_data()))
}

/// Something that runs LibRaw on a file's bytes (installed by the host that can, see [`install_libraw`]).
pub trait Backend: Send + Sync {
    /// Shown in Settings and logs, e.g. `LibRaw 0.22`.
    fn name(&self) -> String;
    /// The file's unpacked mosaic, or why LibRaw can't read it.
    fn decode(&self, bytes: &[u8]) -> Result<Frame, String>;
}

fn choices() -> &'static RwLock<BTreeMap<String, Decoder>> {
    static R: OnceLock<RwLock<BTreeMap<String, Decoder>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

fn libraw_slot() -> &'static RwLock<Option<Arc<dyn Backend>>> {
    static R: OnceLock<RwLock<Option<Arc<dyn Backend>>>> = OnceLock::new();
    R.get_or_init(|| RwLock::new(built_in()))
}

/// The LibRaw this build links (the `libraw` feature with LibRaw found at build time), if any.
fn built_in() -> Option<Arc<dyn Backend>> {
    #[cfg(feature = "libraw")]
    if lcc_libraw::available() {
        return Some(Arc::new(Linked));
    }
    None
}

/// LibRaw linked into the app (`lcc-libraw`).
#[cfg(feature = "libraw")]
struct Linked;

#[cfg(feature = "libraw")]
impl Backend for Linked {
    fn name(&self) -> String {
        lcc_libraw::version().unwrap_or_else(|| "LibRaw".into())
    }
    fn decode(&self, bytes: &[u8]) -> Result<Frame, String> {
        Ok(Frame::from_libraw(lcc_libraw::decode(bytes)?))
    }
}

#[cfg(feature = "libraw")]
impl Frame {
    fn from_libraw(u: lcc_libraw::Unpacked) -> Frame {
        let [x, y, w, h] = u.image_area;
        Frame {
            width: u.width,
            height: u.height,
            data: u.data,
            cfa: u.pattern.map(|p| lcc_raw::Cfa { width: p.width, height: p.height, pattern: p.colors }),
            bits: u.bits,
            black: lcc_raw::BlackLevel { repeat_rows: u.black.rows, repeat_cols: u.black.cols, values: u.black.values, ..Default::default() },
            white: u.white,
            active_area: lcc_raw::Rect::new(x, y, w, h),
            wb_multipliers: u.as_shot,
            xyz_to_camera: u.xyz_to_camera.map(lcc_raw::Mat3),
        }
    }
}

/// Whether this build can run LibRaw at all: false on the web and Android, and in desktop builds made without it.
/// The Settings panel greys the LibRaw choice out then.
pub fn libraw_built_in() -> bool {
    libraw().is_some()
}

/// [`refresh_key`] once the built-in backend is known, so the first key already counts it.
fn ensure_key() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(refresh_key);
}

/// [`key`]: a hash of the choices and the backend's name, 0 with neither, stable from one run to the next (it is in
/// the thumbnail cache's keys on disk).
static KEY: AtomicU64 = AtomicU64::new(0);

fn refresh_key() {
    let map = choices().read().unwrap_or_else(PoisonError::into_inner).clone();
    let name = libraw_name();
    let key = if map.is_empty() && name.is_none() {
        0
    } else {
        // FNV-1a over the choices and the backend's name
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let text = map.iter().map(|(c, d)| format!("{c}={};", d.id())).collect::<String>() + "|" + name.as_deref().unwrap_or_default();
        for b in text.bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
        h.max(1)
    };
    KEY.store(key, Ordering::Relaxed);
}

/// A camera label as matched: words upper-cased and single-spaced.
pub fn camera_key(camera: &str) -> String {
    camera.split_whitespace().map(str::to_uppercase).collect::<Vec<_>>().join(" ")
}

/// Install the person's choices, camera label → decoder (replaces the previous ones; `Auto` entries are dropped).
pub fn install_choices<'a>(list: impl IntoIterator<Item = (&'a String, &'a Decoder)>) {
    let map: BTreeMap<String, Decoder> =
        list.into_iter().filter(|(_, d)| **d != Decoder::Auto).map(|(c, d)| (camera_key(c), *d)).filter(|(c, _)| !c.is_empty()).collect();
    *choices().write().unwrap_or_else(PoisonError::into_inner) = map;
    refresh_key();
}

/// Install (or remove, `None`) the LibRaw backend.
pub fn install_libraw(backend: Option<Arc<dyn Backend>>) {
    *libraw_slot().write().unwrap_or_else(PoisonError::into_inner) = backend;
    refresh_key();
}

/// The LibRaw backend's name, when one is installed.
pub fn libraw_name() -> Option<String> {
    libraw().map(|b| b.name())
}

fn libraw() -> Option<Arc<dyn Backend>> {
    libraw_slot().read().unwrap_or_else(PoisonError::into_inner).clone()
}

/// The choice for a camera label (`Auto` when it has none).
pub fn choice_for(camera: &str) -> Decoder {
    choice_in(&choices().read().unwrap_or_else(PoisonError::into_inner), camera)
}

/// The choice for a camera label in a map keyed by [`camera_key`].
fn choice_in(map: &BTreeMap<String, Decoder>, camera: &str) -> Decoder {
    map.get(&camera_key(camera)).copied().unwrap_or_default()
}

/// Changes whenever a choice or the backend changes: part of the decode and render cache keys, so pictures read by
/// the other decoder are not reused. 0 with no choices and no LibRaw (the cache keys are then what they were before
/// decoders could be chosen).
pub fn key() -> u64 {
    ensure_key();
    KEY.load(Ordering::Relaxed)
}

/// The decoder that read a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Used {
    Lcc,
    Libraw,
}

/// Decode a raw file with its camera's decoder (see the module docs). With no choices and no LibRaw this is
/// [`lcc_raw::decode`].
pub fn decode(bytes: &[u8]) -> lcc_raw::Result<lcc_raw::RawImage> {
    decode_reporting(bytes).map(|(r, _)| r)
}

/// [`decode`], saying which decoder read the file.
pub fn decode_reporting(bytes: &[u8]) -> lcc_raw::Result<(lcc_raw::RawImage, Used)> {
    let backend = libraw();
    let any_choice = !choices().read().unwrap_or_else(PoisonError::into_inner).is_empty();
    if backend.is_none() && !any_choice {
        return lcc_raw::decode(bytes).map(|r| (r, Used::Lcc));
    }
    let choice = if any_choice {
        let m = lcc_meta::extract(bytes);
        choice_for(&lcc_catalog::camera_label(m.make.as_deref().unwrap_or_default(), m.model.as_deref().unwrap_or_default()))
    } else {
        Decoder::Auto
    };
    decode_as(bytes, choice, backend.as_deref())
}

/// Decode `bytes` as `choice` says, with `backend` as LibRaw.
pub fn decode_as(bytes: &[u8], choice: Decoder, backend: Option<&dyn Backend>) -> lcc_raw::Result<(lcc_raw::RawImage, Used)> {
    let with_libraw = |b: &dyn Backend| -> Result<lcc_raw::RawImage, String> { b.decode(bytes)?.into_raw_image(bytes) };
    match (choice, backend) {
        (Decoder::Lcc, _) | (Decoder::Auto, None) => lcc_raw::decode(bytes).map(|r| (r, Used::Lcc)),
        (Decoder::Libraw, None) => {
            log::info!("raw decoder: LibRaw is chosen for this camera but not available here; reading the file with lcc-raw");
            lcc_raw::decode(bytes).map(|r| (r, Used::Lcc))
        }
        (Decoder::Libraw, Some(b)) => match with_libraw(b) {
            Ok(r) => Ok((r, Used::Libraw)),
            Err(e) => {
                log::warn!("raw decoder: {} could not read the file ({e}); reading it with lcc-raw", b.name());
                lcc_raw::decode(bytes).map(|r| (r, Used::Lcc))
            }
        },
        (Decoder::Auto, Some(b)) => match lcc_raw::decode(bytes) {
            Ok(r) => Ok((r, Used::Lcc)),
            Err(lcc_raw::RawError::Unsupported(why)) => match with_libraw(b) {
                Ok(r) => Ok((r, Used::Libraw)),
                Err(e) => {
                    log::info!("raw decoder: {} could not read an unsupported variant either ({e})", b.name());
                    Err(lcc_raw::RawError::Unsupported(why))
                }
            },
            Err(e) => Err(e),
        },
    }
}

#[cfg(test)]
#[path = "tests_raw_decoder.rs"]
mod tests;
