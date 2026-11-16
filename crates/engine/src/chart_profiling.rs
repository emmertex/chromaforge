//! Camera profiling from a ColorChecker photograph (LR-PROF-CREATE): the chart is found on the raw
//! file, a DNG Camera Profile is fitted to it ([`lcc_camprof`]) and lands in the user's camera
//! profiles ([`crate::cmd::dcp_profiles`]), so it is chosen like an imported `.dcp` right away.
//!
//! The work is split so the UI can run it off its thread: [`prepare`] decodes the file and looks for
//! the chart, [`build`] fits the profile (both pure, `Send`), [`install`] files the result in the
//! session. The `profile.findChart` and `profile.createFromChart` commands do all of it in one call.
//!
//! **Same data as the render.** The chart is measured on the camera RGB the app's own raw decoder
//! develops ([`frame_of`]): what the profile is later applied to, so the fit sees the signal it
//! will render. RADcamProfiles' own raw readers are not used.
//!
//! **Coordinates.** Chart corners are in the *frame*: the raw file's developed picture (default
//! crop) as the sensor recorded it (not rotated by its orientation tag), normalised to 0–1 across and down, in screen
//! order (top left, top right, bottom right, bottom left). `turns` (0–3) says which corner patch
//! A01 (dark skin) is in: top left, top right, bottom right, bottom left. [`Prepared::display`] is
//! the photo as the app develops it, oriented; [`Prepared::to_display`] / [`Prepared::from_display`]
//! map between the two.

use std::sync::Arc;

pub use lcc_camprof as camprof;

use lcc_camprof::{BuildOptions, Built, ChartCorners, Located, RawFrame};
use lcc_raster::Rgba8;
use serde_json::{Value, json};

/// The longest edge of the developed picture [`prepare`] makes for looking at.
pub const PREVIEW_EDGE: usize = 3000;

/// The light a chart photo's greys measure through the file's colour matrix: information only (the
/// light a profile is made for is always chosen; a camera's white balance doesn't say it).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightEstimate {
    /// Colour temperature, K.
    pub cct: f64,
    /// The nearest calibration illuminant ([`lcc_camprof::ILLUMINANTS`]).
    pub illuminant: &'static str,
}

/// The light a chart photo is profiled for when none is chosen.
pub const DEFAULT_LIGHT: &str = "D55";

/// A decoded chart photograph.
#[derive(Clone, Debug)]
pub struct Prepared {
    /// Camera-native RGB at half the sensor's resolution: what the chart is measured on.
    pub frame: Arc<RawFrame>,
    /// `UniqueCameraModel` for the profile.
    pub camera_model: String,
    /// The photo developed as the app does by default (as-shot white balance, the file's colour,
    /// default tone; no lens correction, so it lines up with the frame), oriented, display sRGB, at
    /// most [`PREVIEW_EDGE`] long.
    pub display: Rgba8,
    /// The file's orientation: `display` is the frame turned by it.
    pub orientation: lcc_geom::Orientation,
    /// The camera body: its serial number (EXIF `BodySerialNumber`), else `file:XX` from the first
    /// two characters of the file name ([`body_key`]), else `None`.
    pub body: Option<String>,
    /// The body's serial number alone (no file-name key), to give the photo's catalog entry.
    pub serial: Option<String>,
    /// The capture date, `YYYY-MM-DD`, when the file has it.
    pub captured: Option<String>,
    /// Where the chart was found, if it was.
    pub located: Option<Located>,
    /// The file's own colour model, to tell which light the chart was under.
    pub color: lcc_raw::ColorData,
}

/// Camera RGB of `raw` at half resolution, taken exactly where the app develops a raw file and
/// where a camera profile is applied ([`crate::files`]): black and white levels, linearisation,
/// `OpcodeList1/2`, the default crop, `OpcodeList3` without the lens corrections (the pipeline
/// applies those itself), before white balance and colour matrix. Bayer mosaics take the render
/// path's 2 × 2 binning (block means of each colour); other mosaics are demosaiced as for a full-size
/// render and halved.
fn frame_of(raw: &lcc_raw::RawImage) -> Result<RawFrame, String> {
    let mut raw = raw.clone();
    raw.opcodes.list3.retain(|op| !op.is_lens_correction());
    let rgb = match raw.develop_binned_masked(2, f32::INFINITY).map_err(|e| e.to_string())? {
        Some((img, _)) => img,
        None => {
            if raw.cpp == 1 && raw.cfa.is_none() {
                return Err("a monochrome sensor has no colour to profile".into());
            }
            let full = raw.develop(lcc_raw::Method::Ahd).map_err(|e| e.to_string())?;
            let (w, h) = (full.width / 2, full.height / 2);
            let px = |x: usize, y: usize| full.data.get(y * full.width + x).copied().unwrap_or([0.0; 3]);
            let mut data = Vec::with_capacity(w.saturating_mul(h));
            for y in 0..h {
                for x in 0..w {
                    let q = [px(2 * x, 2 * y), px(2 * x + 1, 2 * y), px(2 * x, 2 * y + 1), px(2 * x + 1, 2 * y + 1)];
                    data.push(std::array::from_fn(|c| q.iter().map(|p| p[c]).sum::<f32>() / 4.0));
                }
            }
            lcc_raster::Rgb32f { width: w, height: h, data }
        }
    };
    if rgb.width < 8 || rgb.height < 8 {
        return Err("the photo is too small to profile from".into());
    }
    RawFrame::from_rgb(rgb.width, rgb.height, rgb.data).map_err(|e| e.to_string())
}

/// The camera name a profile for `raw` carries: the file's own `UniqueCameraModel` when it is a
/// DNG that states one, else Adobe-style `Maker Model`.
fn camera_model_of(raw: &lcc_raw::RawImage, bytes: &[u8]) -> String {
    if raw.format == lcc_raw::RawFormat::Dng
        && let Some(name) = unique_camera_model(bytes)
    {
        return name;
    }
    let make = raw.metadata.make.clone().unwrap_or_default();
    let model = raw.metadata.model.clone().unwrap_or_default();
    let name = lcc_camprof::camera_name(&make, &model);
    if name.trim().is_empty() { "Unknown Camera".into() } else { name }
}

fn unique_camera_model(bytes: &[u8]) -> Option<String> {
    let opts = lcc_tiff::ParseOptions { follow_children: false, max_ifds: 2, ..Default::default() };
    let tiff = lcc_tiff::Tiff::parse_with(bytes, &opts).ok()?;
    let name = tiff.ifds.first()?.string(lcc_tiff::tags::UNIQUE_CAMERA_MODEL)?;
    let name = name.trim_matches(char::from(0)).trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// The frame white balanced (by the file's as-shot neutral, else its grey world) and exposed for
/// looking at, oriented: the stand-in when the photo can't be developed for display.
fn preview_of(frame: &RawFrame, neutral: Option<[f64; 3]>, orientation: lcc_geom::Orientation) -> Rgba8 {
    let scale = (frame.width.max(frame.height) as f64 / PREVIEW_EDGE as f64).max(1.0);
    let (w, h) = (((frame.width as f64 / scale) as usize).max(1), ((frame.height as f64 / scale) as usize).max(1));
    let small = frame.preview(w, h);
    let neutral = neutral.filter(|n| n.iter().all(|v| v.is_finite() && *v > 0.0)).unwrap_or_else(|| {
        let mut sum = [0.0f64; 3];
        for p in &small {
            for c in 0..3 {
                sum[c] += p.v[c];
            }
        }
        let g = sum[1].max(1e-9);
        [sum[0].max(1e-9) / g, 1.0, sum[2].max(1e-9) / g]
    });
    let balanced: Vec<[f64; 3]> = small.iter().map(|p| std::array::from_fn(|c| p.v[c] / neutral[c].max(1e-6))).collect();
    let mut lum: Vec<f64> = balanced.iter().map(|p| p[1]).filter(|v| v.is_finite()).collect();
    lum.sort_by(f64::total_cmp);
    let bright = lum.get(lum.len().saturating_sub(1) * 99 / 100).copied().unwrap_or(1.0).max(1e-6);
    let gain = 0.9 / bright;
    let data = balanced
        .iter()
        .map(|p| {
            let e = |v: f64| lcc_color::transfer::encode_srgb8((v * gain).clamp(0.0, 1.0) as f32);
            [e(p[0]), e(p[1]), e(p[2]), 255]
        })
        .collect();
    Rgba8 { width: w, height: h, data }.into_oriented(orientation)
}

/// The photo as the app develops it by default, for looking at (see [`Prepared::display`]).
fn developed(bytes: &[u8]) -> Result<Rgba8, String> {
    let (img, info) = crate::files::load_bytes(bytes, PREVIEW_EDGE)?;
    let mut settings = lcc_develop::DevelopSettings::for_raw(info.as_shot_temp, info.as_shot_tint);
    settings.optics.lens_profile = false;
    // (never larger than the photo itself)
    let req = lcc_pipeline::RenderRequest::fit(PREVIEW_EDGE.min(img.width.max(1)), PREVIEW_EDGE.min(img.height.max(1)));
    let out = lcc_pipeline::render(&img, &info, &settings, &req);
    Ok(out.image)
}

/// Decode a raw chart photograph and look for the chart.
pub fn prepare(bytes: &[u8]) -> Result<Prepared, String> {
    prepare_named(bytes, None)
}

/// [`prepare`] for a file named `file_name` (its body key when the file has no serial number).
pub fn prepare_named(bytes: &[u8], file_name: Option<&str>) -> Result<Prepared, String> {
    if lcc_raw::probe(bytes).is_none() {
        return Err("not a raw file: a camera profile is made from a raw photo of the chart".into());
    }
    let raw = lcc_raw::decode(bytes).map_err(|e| e.to_string())?;
    let frame = frame_of(&raw)?;
    let camera_model = camera_model_of(&raw, bytes);
    let orientation = raw.orientation;
    let display = match developed(bytes) {
        Ok(img) => img,
        Err(e) => {
            log::warn!("camera profile: the chart photo could not be developed for display ({e}); showing it as shot");
            let neutral = raw.color.as_shot_neutral.or_else(|| raw.wb_multipliers.map(|m| std::array::from_fn(|c| 1.0 / f64::from(m[c]).max(1e-6))));
            preview_of(&frame, neutral, orientation)
        }
    };
    let located = lcc_camprof::find_chart(&frame);
    let meta = if raw.metadata.serial_number.is_some() && raw.metadata.capture_time.is_some() { None } else { Some(lcc_meta::extract(bytes)) };
    let serial = raw.metadata.serial_number.clone().or_else(|| meta.as_ref().and_then(|m| m.serial_number.clone()));
    let body = body_key(serial.as_deref(), file_name);
    let serial = serial.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let captured = raw
        .metadata
        .capture_time
        .or_else(|| meta.as_ref().and_then(|m| m.capture_time))
        .map(|t| format!("{:04}-{:02}-{:02}", t.year, t.month, t.day));
    Ok(Prepared { body, serial, captured, frame: Arc::new(frame), camera_model, display, orientation, located, color: raw.color })
}

impl Prepared {
    /// A normalised frame point on [`Self::display`] (normalised).
    pub fn to_display(&self, p: [f64; 2]) -> [f64; 2] {
        let (x, y) = self.orientation.map(p[0], p[1], 1.0, 1.0);
        [x, y]
    }

    /// A normalised point of [`Self::display`] in the frame (normalised).
    pub fn from_display(&self, p: [f64; 2]) -> [f64; 2] {
        let (x, y) = self.orientation.inverse().map(p[0], p[1], 1.0, 1.0);
        [x, y]
    }
}

/// What a quick look at a photo finds (for choosing a chart photo from the catalog): the chart's
/// pattern score, and the light, when a chart is there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChartScan {
    pub score: f64,
    pub light: Option<LightEstimate>,
}

/// Look for a ColorChecker in a raw file without developing it for display: `None` when there is
/// none (or the file isn't a raw photo).
pub fn scan_path(path: &str) -> Option<ChartScan> {
    let len = std::fs::metadata(path).ok()?.len();
    if len > 2 << 30 {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    lcc_raw::probe(&bytes)?;
    let raw = lcc_raw::decode(&bytes).ok()?;
    let frame = frame_of(&raw).ok()?;
    let located = lcc_camprof::find_chart(&frame)?;
    let prep = Prepared {
        frame: Arc::new(frame),
        camera_model: String::new(),
        display: Rgba8::new(1, 1),
        orientation: raw.orientation,
        body: None,
        serial: None,
        captured: None,
        located: Some(located),
        color: raw.color,
    };
    Some(ChartScan { score: located.score.unwrap_or(0.0), light: estimate_light(&prep, &located) })
}

/// The key a camera body is known by: its serial number when the file has one; else `file:XX`, the
/// first two characters of the file name after any leading `_` (`_SA94454.ARW` → `file:SA`, as
/// photographers often name files per body); else `None` (those two aren't both letters or digits).
pub fn body_key(serial: Option<&str>, file_name: Option<&str>) -> Option<String> {
    if let Some(s) = serial.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(s.to_string());
    }
    let name = std::path::Path::new(file_name?).file_stem()?.to_str()?.trim().trim_start_matches('_');
    let two: String = name.chars().take(2).collect();
    (two.chars().count() == 2 && two.chars().all(|c| c.is_ascii_alphanumeric())).then(|| format!("file:{two}"))
}

/// [`prepare`] for a file (refused above 2 GB).
pub fn prepare_path(path: &str) -> Result<Prepared, String> {
    let len = std::fs::metadata(path).map_err(|e| format!("{path}: {e}"))?.len();
    if len > 2 << 30 {
        return Err(format!("{path}: larger than 2 GB"));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let name = std::path::Path::new(path).file_name().and_then(|n| n.to_str());
    prepare_named(&bytes, name)
}

/// Corners in frame pixels from normalised ones (`None` when one isn't a finite number).
pub fn corners_from_normalized(frame: &RawFrame, c: [[f64; 2]; 4]) -> Option<ChartCorners> {
    let (w, h) = (frame.width as f64, frame.height as f64);
    if c.iter().flatten().any(|v| !v.is_finite()) {
        return None;
    }
    let p = |q: [f64; 2]| [q[0].clamp(-1.0, 2.0) * w, q[1].clamp(-1.0, 2.0) * h];
    Some(ChartCorners { top_left: p(c[0]), top_right: p(c[1]), bottom_right: p(c[2]), bottom_left: p(c[3]) })
}

/// Corners normalised to the frame.
pub fn normalized_corners(frame: &RawFrame, c: &ChartCorners) -> [[f64; 2]; 4] {
    let (w, h) = (frame.width.max(1) as f64, frame.height.max(1) as f64);
    c.points().map(|p| [p[0] / w, p[1] / h])
}

/// What the chart's mid greys ([`lcc_camprof::reference::Chart::mid_grey_ids`]) measure, taken to a white point through the file's own
/// colour model (DNG 6.3): information only. `None` without a colour matrix.
pub fn estimate_light(prep: &Prepared, located: &Located) -> Option<LightEstimate> {
    measured_light(prep, located).map(|cct| LightEstimate { cct, illuminant: lcc_camprof::nearest_illuminant(cct) })
}

fn measured_light(prep: &Prepared, located: &Located) -> Option<f64> {
    if !lcc_raw::color::has_matrix(&prep.color) {
        return None;
    }
    let chart = located.chart;
    let greys = chart.mid_grey_ids();
    let m = lcc_camprof::reference::measure_rotated(&prep.frame, chart, &located.corners, lcc_camprof::SAMPLE_FRACTION, located.turns % 4);
    let mut sum = [0.0f64; 3];
    for (id, rgb) in m.ids.iter().zip(&m.camera_rgb) {
        if greys.contains(&id.as_str()) {
            for c in 0..3 {
                sum[c] += rgb[c];
            }
        }
    }
    let peak = sum.iter().copied().fold(0.0, f64::max);
    if peak.is_nan() || peak <= 0.0 || sum.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let xy = lcc_raw::color::neutral_to_xy(&prep.color, sum.map(|v| v / peak));
    let cct = lcc_color::cct::xy_to_temp_tint(xy).0;
    (cct.is_finite() && cct > 1000.0 && cct < 25000.0).then_some(cct)
}

/// What to build from one chart photo.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Request {
    /// Chart corners (normalised, see the module notes); `None`: where [`prepare`] found the chart.
    pub corners: Option<[[f64; 2]; 4]>,
    /// Which corner A01 is in; `None` with corners: decided by fitting.
    pub turns: Option<u8>,
    /// Move the corners onto the chart first.
    pub refine: bool,
    /// The light ([`lcc_camprof::ILLUMINANTS`]); empty or `auto`: [`estimate_light`] (an error
    /// when there is none: the light must be chosen).
    pub illuminant: String,
    /// Which chart the corners outline ([`lcc_camprof::reference::CHARTS`] key); `None`: the one [`prepare`]
    /// found, else the ColorChecker 24.
    pub chart: Option<String>,
}

/// Where the chart of `prep` lies for `req`.
pub fn locate(prep: &Prepared, req: &Request) -> Result<Located, String> {
    let frame = prep.frame.as_ref();
    match req.corners {
        Some(c) => {
            let corners = corners_from_normalized(frame, c).ok_or("the chart corners must be numbers")?;
            if !corners.is_usable() {
                return Err("the chart corners don't make a usable grid".into());
            }
            let chart = match req.chart.as_deref() {
                Some(key) => lcc_camprof::reference::by_key(key).ok_or_else(|| format!("unknown chart `{key}`"))?,
                None => prep.located.map(|l| l.chart).unwrap_or_else(lcc_camprof::chart),
            };
            let corners = if req.refine { lcc_camprof::refine(frame, chart, &corners, 0).map(|l| l.corners).unwrap_or(corners) } else { corners };
            let turns = match req.turns {
                Some(t) => t % 4,
                None => lcc_camprof::orient(frame, chart, &corners),
            };
            Ok(Located { corners, turns, score: None, chart })
        }
        None => prep.located.ok_or_else(|| "no color chart found in the photo: place its corners by hand".into()),
    }
}

/// One calibration of a built profile, and the photo it came from.
#[derive(Clone, Debug)]
pub struct Calibration {
    pub built: Built,
    /// 0: the first photo, 1: the second.
    pub shot: usize,
    /// The light as estimated from the photo, when it could be.
    pub estimated: Option<LightEstimate>,
}

/// A profile from one chart photo, or a dual-illuminant one from two under different lights.
#[derive(Clone, Debug)]
pub struct Outcome {
    pub dcp: lcc_raw::dcp::Dcp,
    /// Things worth telling the caller (e.g. an option no longer supported).
    pub warnings: Vec<String>,
    /// The lower colour temperature first.
    pub calibrations: Vec<Calibration>,
}

/// The light a request asks for ([`DEFAULT_LIGHT`] when none; `auto`, no longer offered, reads as it
/// with a warning in `warnings`).
fn light_of(prep: &Prepared, req: &Request, located: &Located, warnings: &mut Vec<String>) -> (String, Option<LightEstimate>) {
    let estimated = estimate_light(prep, located);
    let chosen = req.illuminant.trim();
    if chosen.eq_ignore_ascii_case("auto") {
        warnings.push(format!(
            "illuminant `auto` is no longer supported (a camera's white balance doesn't say the light): {DEFAULT_LIGHT} was used; choose the light the chart was photographed under"
        ));
        return (DEFAULT_LIGHT.to_string(), estimated);
    }
    (if chosen.is_empty() { DEFAULT_LIGHT.to_string() } else { canonical_light(chosen) }, estimated)
}

/// A light as the commands take it: `daylight` / `flash` are D55, `tungsten` is StdA (A); the
/// standard names pass through.
pub fn canonical_light(light: &str) -> String {
    match light.trim().to_ascii_lowercase().as_str() {
        "daylight" | "flash" | "daylight / flash" => "D55".into(),
        "tungsten" | "stda" => "A".into(),
        _ => light.trim().to_string(),
    }
}

/// How a profile's illuminant reads to people: "Daylight" (D55) and "Tungsten" (StdA, A), else its
/// standard name.
pub fn light_word(illuminant: &str) -> String {
    lcc_camprof::light_code(illuminant).to_string()
}

/// How the name of a profile tells bodies apart (`Canon EOS R5 C 71`): the last two characters of a
/// serial number, or a file-name key's two (`file:SA` → `SA`); `None` without a body.
pub fn body_suffix(body: Option<&str>) -> Option<String> {
    let body = body?.trim();
    if let Some(key) = body.strip_prefix("file:") {
        return (!key.is_empty()).then(|| key.to_string());
    }
    let s: Vec<char> = body.chars().filter(|c| !c.is_whitespace()).collect();
    if s.is_empty() {
        return None;
    }
    Some(s[s.len().saturating_sub(2)..].iter().collect())
}

/// The camera as a profile name says it: the model, then the body's serial suffix.
pub fn camera_label(prep: &Prepared) -> String {
    match body_suffix(prep.body.as_deref()) {
        Some(b) => format!("{} {b}", prep.camera_model),
        None => prep.camera_model.clone(),
    }
}

/// The name a profile gets when none is given: `<camera> <body> · <light(s)> · <HSL|HS> · <date>`
/// ([`lcc_camprof::default_name`]); the date is the first photo's capture date, else today.
pub fn default_name(first: &Prepared, lights: &[&str], hue_saturation_only: bool) -> String {
    let mut lights: Vec<&str> = lights.to_vec();
    lights.sort_by(|a, b| lcc_camprof::illuminant_cct(a).unwrap_or(0.0).total_cmp(&lcc_camprof::illuminant_cct(b).unwrap_or(0.0)));
    let today = crate::import::system_clock();
    let date = first.captured.clone().unwrap_or_else(|| today.get(..10).unwrap_or("").to_string());
    lcc_camprof::default_name(&camera_label(first), &lights.join(" + "), hue_saturation_only, &date)
}

/// Two chart photos from different bodies of the camera: their serial suffixes.
pub fn bodies_differ(a: &Prepared, b: &Prepared) -> Option<(String, String)> {
    match (body_suffix(a.body.as_deref()), body_suffix(b.body.as_deref())) {
        (Some(x), Some(y)) if a.body != b.body => Some((x, y)),
        _ => None,
    }
}

/// Fit a profile to the chart in `first` (and, for a dual-illuminant profile, `second`).
/// `options.illuminant` is ignored: each photo's light comes from its [`Request`].
pub fn build(first: (&Prepared, &Request), second: Option<(&Prepared, &Request)>, options: &BuildOptions) -> Result<Outcome, String> {
    let mut options = options.clone();
    if options.camera_model.trim().is_empty() {
        options.camera_model = first.0.camera_model.clone();
    }
    let la = locate(first.0, first.1)?;
    let mut warnings = Vec::new();
    let (ia, ea) = light_of(first.0, first.1, &la, &mut warnings);
    let named = !options.name.trim().is_empty();
    if !named && second.is_none() {
        options.name = default_name(first.0, &[&ia], options.natural);
    }
    let Some((prep_b, req_b)) = second else {
        let built = lcc_camprof::build(&first.0.frame, &la, &BuildOptions { illuminant: ia, ..options }).map_err(|e| e.to_string())?;
        return Ok(Outcome { dcp: built.dcp.clone(), warnings, calibrations: vec![Calibration { built, shot: 0, estimated: ea }] });
    };
    if prep_b.camera_model != first.0.camera_model {
        return Err(format!("the two chart photos are from different cameras ({} and {})", first.0.camera_model, prep_b.camera_model));
    }
    let lb = locate(prep_b, req_b)?;
    let (ib, eb) = light_of(prep_b, req_b, &lb, &mut warnings);
    if !named {
        options.name = default_name(first.0, &[&ia, &ib], options.natural);
    }
    let dual = lcc_camprof::build_dual(
        &lcc_camprof::Shot { frame: &first.0.frame, located: la, illuminant: &ia },
        &lcc_camprof::Shot { frame: &prep_b.frame, located: lb, illuminant: &ib },
        &options,
    )
    .map_err(|e| e.to_string())?;
    // (build_dual puts the lower colour temperature first)
    let first_is_low = lcc_camprof::illuminant_cct(&ia).unwrap_or(0.0) <= lcc_camprof::illuminant_cct(&ib).unwrap_or(0.0);
    let [low, high] = dual.calibrations;
    let calibrations = if first_is_low {
        vec![Calibration { built: low, shot: 0, estimated: ea }, Calibration { built: high, shot: 1, estimated: eb }]
    } else {
        vec![Calibration { built: low, shot: 1, estimated: eb }, Calibration { built: high, shot: 0, estimated: ea }]
    };
    Ok(Outcome { dcp: dual.dcp, warnings, calibrations })
}

/// The fit report as JSON (`ΔE` is CIEDE2000; corners normalised to each photo's frame). The
/// top-level ΔE figures cover every calibration (the mean of their means, the largest maximum);
/// `calibrations` has each light's own.
pub fn report_json(preps: &[&Prepared], out: &Outcome) -> Value {
    let r3 = |v: f64| (v * 1000.0).round() / 1000.0;
    let calibrations: Vec<Value> = out
        .calibrations
        .iter()
        .map(|c| {
            let b = &c.built;
            let corners = preps.get(c.shot).map(|p| normalized_corners(&p.frame, &b.located.corners));
            json!({
                "shot": c.shot,
                "illuminant": b.illuminant,
                "cct": lcc_camprof::illuminant_cct(&b.illuminant),
                "estimatedCct": c.estimated.map(|e| e.cct.round()),
                "estimatedIlluminant": c.estimated.map(|e| e.illuminant),
                "meanDeltaE": r3(b.mean_delta_e),
                "maxDeltaE": r3(b.max_delta_e),
                "matrixMeanDeltaE": r3(b.matrix_mean_delta_e),
                "patchesRead": b.patches_read,
                "patchesUsed": b.patches_used,
                "neutralIds": b.neutral_ids,
                "corners": corners,
                "turns": b.located.turns,
                "chart": b.located.chart.key,
                "notes": b.notes,
                "patches": b.patches.iter().map(|p| json!({
                    "id": p.id,
                    "name": p.name,
                    "deltaE": r3(p.delta_e),
                    "matrixDeltaE": r3(p.matrix_delta_e),
                    "referenceLab": p.reference_lab.map(r3),
                    "renderedLab": p.rendered_lab.map(r3),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let n = out.calibrations.len().max(1) as f64;
    let mean = out.calibrations.iter().map(|c| c.built.mean_delta_e).sum::<f64>() / n;
    let max = out.calibrations.iter().map(|c| c.built.max_delta_e).fold(0.0, f64::max);
    let matrix = out.calibrations.iter().map(|c| c.built.matrix_mean_delta_e).sum::<f64>() / n;
    let primary = calibrations.iter().find(|c| c["shot"] == 0).cloned().unwrap_or(Value::Null);
    json!({
        "cameraModel": out.dcp.model,
        "name": out.dcp.name,
        "dual": out.calibrations.len() == 2,
        "warnings": out.warnings,
        "meanDeltaE": r3(mean),
        "maxDeltaE": r3(max),
        "matrixMeanDeltaE": r3(matrix),
        "patchesUsed": primary["patchesUsed"],
        "corners": primary["corners"],
        "turns": primary["turns"],
        "chart": primary["chart"],
        "illuminant": primary["illuminant"],
        "patches": primary["patches"],
        "calibrations": calibrations,
    })
}

/// What to do when a profile with the same name for the same camera is already there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnExists {
    /// Save nothing; say which profile is there (the UI then asks).
    Ask,
    /// Write over it in place: same id, so the photos using it follow the new version.
    Replace,
    /// Save beside it as "… 2", "… 3".
    KeepBoth,
    /// An error naming the existing profile and these choices.
    Fail,
}

impl OnExists {
    /// `ask`, `replace`, `keepBoth` or `fail` (the default); `None` for anything else.
    pub fn parse(v: Option<&str>) -> Option<OnExists> {
        Some(match v.unwrap_or("fail") {
            "ask" => OnExists::Ask,
            "replace" => OnExists::Replace,
            "keepBoth" => OnExists::KeepBoth,
            "fail" => OnExists::Fail,
            _ => return None,
        })
    }
}

/// The id of the imported profile named `name` for `model`, if there is one.
pub fn existing(name: &str, model: &str) -> Option<String> {
    lcc_raw::dcp::library().into_iter().find(|(_, d)| d.name.trim() == name.trim() && d.model == model).map(|(id, _)| id)
}

/// How many photos develop with camera profile `id`.
pub fn used_by(s: &crate::Session, id: &str) -> usize {
    s.catalog.photos().filter(|p| p.develop.profile.camera == id).count()
}

/// The first free "name 2", "name 3"… for `model`.
pub fn free_name(name: &str, model: &str) -> String {
    let base = name.trim();
    (2..10_000).map(|n| format!("{base} {n}")).find(|n| existing(n, model).is_none()).unwrap_or_else(|| base.to_string())
}

/// File `dcp` among the user's imported camera profiles, in `library` (the camera-profiles folder's
/// `dcp/`), so it is chosen per photo right away (`develop.cameraProfile`); the photos in `apply_to`
/// develop with it, and `bodies` (body keys, `model|body`) start with it. A profile of the same name
/// for the camera is handled as `on_exists` says. → `{id, name, cameraModel, file, applied, bodies,
/// replaced}`, or, with [`OnExists::Ask`] and a profile there, `{exists: {id, name, usedBy},
/// keepBothName}` (nothing saved).
pub fn install(
    s: &mut crate::Session,
    library: &std::path::Path,
    dcp: &lcc_raw::dcp::Dcp,
    bodies: &[String],
    on_exists: OnExists,
    apply_to: &[lcc_catalog::PhotoId],
) -> crate::Result<Value> {
    const C: &str = "profile.createFromChart";
    let bad = |m: String| crate::cmd::bad(C, m);
    let mut dcp = dcp.clone();
    if dcp.name.trim().is_empty() {
        dcp.name = "Camera Profile".into();
    }
    let mut replaced = false;
    let mut id = None;
    if let Some(there) = existing(&dcp.name, &dcp.model) {
        match on_exists {
            OnExists::Ask => {
                return Ok(
                    json!({"exists": {"id": there, "name": dcp.name, "usedBy": used_by(s, &there)}, "keepBothName": free_name(&dcp.name, &dcp.model)}),
                );
            }
            OnExists::Fail => {
                return Err(bad(format!(
                    "a camera profile “{}” for {} is already there ({there}, used by {} photos): pass onExists: \"replace\" to write over it (the photos follow) or \"keepBoth\" to save this one as “{}”, or give another name",
                    dcp.name,
                    dcp.model,
                    used_by(s, &there),
                    free_name(&dcp.name, &dcp.model)
                )));
            }
            OnExists::KeepBoth => dcp.name = free_name(&dcp.name, &dcp.model),
            OnExists::Replace => {
                replaced = true;
                id = Some(there);
            }
        }
    }
    let bytes = lcc_raw::dcp::write(&dcp).map_err(bad)?;
    let id = match id {
        Some(id) => id,
        None => {
            // (a name that starts with the camera already says it)
            let base = if dcp.name.starts_with(dcp.model.as_str()) { dcp.name.clone() } else { format!("{}-{}", dcp.model, dcp.name) };
            let slug = crate::presets::slug(&base);
            let taken = |stem: &str| library.join(format!("{stem}.dcp")).exists() || lcc_raw::dcp::library_get(&format!("dcp:{stem}")).is_some();
            let stem = std::iter::once(slug.clone()).chain((2..10_000).map(|n| format!("{slug}-{n}"))).find(|s| !taken(s)).unwrap_or(slug);
            format!("dcp:{stem}")
        }
    };
    let stem = id.trim_start_matches("dcp:");
    std::fs::create_dir_all(library).map_err(|e| bad(format!("{}: {e}", library.display())))?;
    let file = library.join(format!("{stem}.dcp"));
    // a profile written over is kept first (in `.replaced/`), and the old bytes go with the undo step
    let old = if replaced && file.exists() {
        let old = std::fs::read(&file).map_err(|e| bad(format!("{}: {e}", file.display())))?;
        keep_replaced(library, stem, &old).map_err(|e| bad(format!("the profile it replaces could not be kept, so nothing was changed: {e}")))?;
        Some(old)
    } else {
        None
    };
    write_atomic(&file, &bytes, &|p, b| std::fs::write(p, b)).map_err(|e| bad(format!("{}: {e}", file.display())))?;
    register(&id, dcp.clone());
    let mut ops = Vec::new();
    let mut applied = Vec::new();
    for &photo in apply_to {
        let Some(ph) = s.catalog.photo(photo) else { continue };
        if applied.contains(&photo.0) {
            continue;
        }
        // a replaced profile's photos are decoded again below; the others switch to it
        if ph.develop.profile.camera != id {
            let mut d = (*ph.develop).clone();
            d.profile.camera = id.clone();
            if let Some(op) = s.develop_op(photo, d, "Camera Profile") {
                ops.push(op);
            }
        }
        applied.push(photo.0);
    }
    if replaced {
        forget_users(s, &id);
    }
    if !ops.is_empty() || old.is_some() {
        s.commit(if old.is_some() { "Replace Camera Profile" } else { C }, crate::Op::Batch { ops })?;
        if let (Some(bytes), Some(e)) = (old, s.undo.last_mut()) {
            e.profile = Some(ProfileSwap { id: id.clone(), file: file.clone(), bytes });
        }
    }
    let mut set_bodies = Vec::new();
    for b in bodies {
        let b = b.trim();
        if !b.is_empty() && b.len() <= 200 {
            s.body_profiles.insert(b.to_string(), id.clone());
            set_bodies.push(b.to_string());
        }
    }
    if !set_bodies.is_empty() {
        s.save_prefs()?;
    }
    Ok(
        json!({"id": id, "name": dcp.name, "cameraModel": dcp.model, "file": file.to_string_lossy(), "bodies": set_bodies, "applied": applied, "replaced": replaced}),
    )
}

/// A camera profile file and the bytes an undo or redo step puts back in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileSwap {
    /// `dcp:…`
    pub id: String,
    pub file: std::path::PathBuf,
    pub bytes: Vec<u8>,
}

/// Write `bytes` to `path` through a temporary file beside it, synced, then renamed over `path`:
/// `path` is either the old file or the new one, never a part of either. `write` writes the
/// temporary file (a test stands in a full disk); a failure leaves `path` as it was and removes
/// the temporary file.
pub fn write_atomic(path: &std::path::Path, bytes: &[u8], write: &dyn Fn(&std::path::Path, &[u8]) -> std::io::Result<()>) -> std::io::Result<()> {
    let tmp = path.with_extension("dcp.tmp");
    // (the flush needs a handle with write access: on Windows `FlushFileBuffers` on a read-only one
    // is "Access is denied"; the handle is closed before the rename)
    let done =
        write(&tmp, bytes).and_then(|()| std::fs::OpenOptions::new().write(true).open(&tmp)?.sync_all()).and_then(|()| rename_over(&tmp, path));
    if done.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    done
}

/// Rename `from` over `to`. Windows refuses for a moment while another process (a virus scanner,
/// the search indexer) has `to` open, so it tries again briefly there before giving up; the old
/// file stays in place until a rename succeeds.
fn rename_over(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    let mut tries = 0;
    loop {
        match std::fs::rename(from, to) {
            Err(e) if cfg!(windows) && e.kind() == std::io::ErrorKind::PermissionDenied && tries < 20 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            done => return done,
        }
    }
}

/// Keep `old` (profile `stem`'s file before a Replace) as `<library>/.replaced/<stem> <time>.dcp`.
fn keep_replaced(library: &std::path::Path, stem: &str, old: &[u8]) -> std::io::Result<std::path::PathBuf> {
    let dir = library.join(".replaced");
    std::fs::create_dir_all(&dir)?;
    // (the clock as the import dates things; `:` is not allowed in file names everywhere)
    let when: String = crate::import::system_clock()
        .chars()
        .take(19)
        .map(|c| {
            if c == ':' {
                '-'
            } else if c == 'T' {
                ' '
            } else {
                c
            }
        })
        .collect();
    let mut to = dir.join(format!("{stem} {when}.dcp"));
    let mut n = 2;
    while to.exists() && n < 10_000 {
        to = dir.join(format!("{stem} {when} {n}.dcp"));
        n += 1;
    }
    write_atomic(&to, old, &|p, b| std::fs::write(p, b))?;
    Ok(to)
}

/// Hand the decoder profile `id` (in place of an older version).
fn register(id: &str, dcp: lcc_raw::dcp::Dcp) {
    let mut list: Vec<(String, lcc_raw::dcp::Dcp)> =
        lcc_raw::dcp::library().into_iter().filter(|(k, _)| k != id).map(|(k, d)| (k, (*d).clone())).collect();
    list.push((id.to_string(), dcp));
    lcc_raw::dcp::install_library(list);
}

/// The photos that develop with profile `id` are decoded again.
fn forget_users(s: &mut crate::Session, id: &str) {
    let users: Vec<lcc_catalog::PhotoId> = s.catalog.photos().filter(|p| p.develop.profile.camera == id).map(|p| p.id).collect();
    for u in users {
        s.media.forget(u);
    }
}

/// Put `swap` in place (undo / redo of a Replace): its bytes back in its file, atomically, the
/// decoder's copy and the photos using it refreshed → the swap that reverses it.
pub(crate) fn apply_swap(s: &mut crate::Session, swap: &ProfileSwap) -> crate::Result<ProfileSwap> {
    let err = |m: String| crate::EngineError::Other(format!("can't put the camera profile back: {m}"));
    let dcp = lcc_raw::dcp::parse(&swap.bytes).map_err(err)?;
    let now = std::fs::read(&swap.file).map_err(|e| err(format!("{}: {e}", swap.file.display())))?;
    write_atomic(&swap.file, &swap.bytes, &|p, b| std::fs::write(p, b)).map_err(|e| err(format!("{}: {e}", swap.file.display())))?;
    register(&swap.id, dcp);
    forget_users(s, &swap.id);
    Ok(ProfileSwap { id: swap.id.clone(), file: swap.file.clone(), bytes: now })
}

/// A ColorChecker photographed by a made-up camera, as a DNG (an RGGB mosaic `2 × width` by
/// `2 × height`), for tests and demonstrations: painted procedurally
/// ([`lcc_camprof::synthetic`]), no photograph involved. The chart grid lies at `corners`
/// (in half-resolution pixels), lit by `light` (an illuminant of
/// [`lcc_camprof::ILLUMINANTS`]). The file's own colour matrix is the made-up camera's linear
/// response when `true_matrix`, else a naive one (sRGB primaries), as an unprofiled camera's might be.
pub fn demo_chart_dng(width: usize, height: usize, corners: &ChartCorners, light: &str, true_matrix: bool) -> Result<Vec<u8>, String> {
    demo_chart_dng_of_body(width, height, corners, light, true_matrix, None)
}

/// [`demo_chart_dng`] from the camera body with serial number `serial` (DNG `CameraSerialNumber`).
pub fn demo_chart_dng_of_body(
    width: usize,
    height: usize,
    corners: &ChartCorners,
    light: &str,
    true_matrix: bool,
    serial: Option<&str>,
) -> Result<Vec<u8>, String> {
    use lcc_raw::{BlackLevel, Cfa, ColorData, DngWriteOptions, Mat3, Metadata, OpcodeLists, Orientation, RawData, RawFormat, RawImage, Rect};
    if width < 8 || height < 8 || width > 8000 || height > 8000 {
        return Err("the demo chart photo must be 8 to 8000 pixels each way".into());
    }
    let tag = lcc_camprof::illuminants::ExifLightSource::from_name(light).ok_or_else(|| format!("unknown light `{light}`"))?;
    let lit = lcc_camprof::illuminants::white_point_by_name("1931_2", light).map_err(|e| e.to_string())?;
    let rgb = lcc_camprof::synthetic::chart_frame(width, height, corners, 0, lit, 0.5);
    let (w, h) = (2 * width, 2 * height);
    let (black, white) = (256.0f32, 16000.0f32);
    let mut data = vec![0u16; w * h];
    let s = |v: f32| (black + v.clamp(0.0, 1.0) * (white - black)).round() as u16;
    for (i, [r, g, b]) in rgb.iter().enumerate() {
        let (x, y) = (i % width, i / width);
        for (dx, dy, v) in [(0, 0, *r), (1, 0, *g), (0, 1, *g), (1, 1, *b)] {
            if let Some(d) = data.get_mut((2 * y + dy) * w + 2 * x + dx) {
                *d = s(v);
            }
        }
    }
    let n = lcc_camprof::synthetic::camera_signal(lit);
    let neutral = [n[0] / n[1].max(1e-9), 1.0, n[2] / n[1].max(1e-9)];
    let matrix = if true_matrix {
        Mat3(lcc_camprof::synthetic::TEST_CAMERA)
    } else {
        Mat3([[3.2406, -1.5372, -0.4986], [-0.9689, 1.8758, 0.0415], [0.0557, -0.2040, 1.0570]])
    };
    let raw = RawImage {
        format: RawFormat::Dng,
        width: w,
        height: h,
        cpp: 1,
        data: RawData::U16(data),
        cfa: Cfa::bayer("RGGB"),
        bits: 14,
        black: BlackLevel::uniform(black),
        white: vec![white],
        active_area: Rect::new(0, 0, w, h),
        crop: Rect::new(0, 0, w, h),
        orientation: Orientation::Normal,
        color: ColorData {
            illuminant: [u16::from(tag.tag()), 0],
            color_matrix: [Some(matrix), None],
            as_shot_neutral: Some(neutral),
            ..Default::default()
        },
        wb_multipliers: None,
        linearized: false,
        opcodes: OpcodeLists::default(),
        metadata: Metadata {
            make: Some("Demo".into()),
            model: Some("Chart Camera".into()),
            serial_number: serial.map(str::to_string),
            ..Default::default()
        },
        pixel_aspect: 1.0,
        lens_metadata: Default::default(),
    };
    lcc_raw::write_dng(&raw, &DngWriteOptions::default()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_corners_are_errors() {
        let frame = RawFrame::from_rgb(40, 30, vec![[0.2; 3]; 1200]).unwrap();
        let prep = Prepared {
            frame: Arc::new(frame),
            camera_model: "X".into(),
            display: Rgba8::new(1, 1),
            orientation: Default::default(),
            body: None,
            serial: None,
            captured: None,
            located: None,
            color: Default::default(),
        };
        for corners in [[[f64::NAN, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]], [[0.5; 2]; 4], [[1e308, -1e308], [0.0; 2], [0.0; 2], [0.0; 2]]] {
            let req = Request { corners: Some(corners), turns: Some(9), ..Default::default() };
            assert!(build((&prep, &req), None, &Default::default()).is_err());
        }
        assert!(build((&prep, &Request::default()), None, &Default::default()).unwrap_err().contains("no color chart"));
        assert!(prepare(b"not a raw file").is_err());
        assert!(prepare(&[]).is_err());
    }
}
