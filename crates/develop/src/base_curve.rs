//! Base curves: a selectable base tone for raw photos, read at runtime from a curve file.
//!
//! A *base curve* replaces the app's base tone (the camera look's tone or the generic raw curve) for the photo that
//! selects it (`profile.base_curve`, an id `base:<file stem>`). Nothing is bundled: the files live in a folder the
//! user fills (the engine's `base_curves` module scans it and installs the result with [`install_library`]).
//!
//! This module holds the file format ([`parse`], documented in `docs/base-curve-format.md`), the evaluation of a
//! curve ([`BaseCurve::eval_luminance`]) and the library of the installed curves. It knows nothing about any
//! other program's file format.
//!
//! The Tone Curve tool (the point curve of `ToneCurve`) is a different thing: it edits display values after the
//! base tone.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock, RwLock};

/// `format` field of a curve file.
pub const FORMAT: &str = "base-curve";
/// The newest format version this build reads. Version 2 adds the `natural-cubic-spline` interpolation and the
/// `encode` and `decode` steps; a version 1 file may not use them.
pub const VERSION: u32 = 2;
/// Largest curve file read (bytes).
pub const MAX_FILE: u64 = 1 << 20;
pub const MAX_CURVES: usize = 8;
pub const MAX_POINTS: usize = 256;
pub const MAX_STEPS: usize = 8;
/// Prefix of a base curve id in the settings.
pub const ID_PREFIX: &str = "base:";
const MAX_ID: usize = 128;

/// How a curve is read between its points.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Interpolation {
    Linear,
    MonotoneCubic,
    /// A C2 cubic spline through the points with natural ends (second derivative 0 at both), clamped to 0..=1. Not
    /// monotone by construction. Format version 2.
    NaturalCubicSpline,
}

/// What a step's curve acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum On {
    /// The luminance of the colour; the colour follows by ratio.
    Luminance,
    /// Each of the three channels on its own.
    EachChannel,
}

/// What a curve does beyond its last point (x above 1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Extension {
    /// Holds the last value.
    #[default]
    Clamp,
    /// Continues with the slope of the last segment.
    Linear,
    /// Closes the gap to 1 exponentially at the last segment's slope.
    Rolloff,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum InputEncoding {
    #[default]
    Linear,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum OutputEncoding {
    #[default]
    DisplayLinear,
}

/// How the scene value becomes the curve's x: `x = gain · v / white`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Input {
    pub gain: f64,
    pub white: f64,
}

#[derive(Deserialize)]
#[serde(default)]
struct RawInput {
    encoding: InputEncoding,
    gain: f64,
    white: f64,
}

impl Default for RawInput {
    fn default() -> Self {
        Self { encoding: InputEncoding::Linear, gain: 1.0, white: 1.0 }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawOutput {
    encoding: OutputEncoding,
}

#[derive(Deserialize)]
struct RawCurve {
    interpolation: Interpolation,
    points: Vec<[f64; 2]>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum EncodeKind {
    #[default]
    PowerSegment,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum DecodeKind {
    #[default]
    PowerToe,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
enum RawStep {
    Curve {
        curve: String,
        on: On,
    },
    Mix {
        a: String,
        b: String,
        t: f64,
    },
    Encode {
        #[serde(default)]
        kind: EncodeKind,
        gamma: f64,
        #[serde(default)]
        offset: f64,
    },
    Decode {
        #[serde(default)]
        kind: DecodeKind,
        gamma: f64,
        #[serde(default)]
        knee: f64,
    },
}

#[derive(Deserialize)]
struct RawFile {
    format: String,
    version: u32,
    #[serde(default)]
    name: String,
    #[serde(default)]
    camera: String,
    #[serde(default)]
    input: RawInput,
    #[serde(default)]
    output: RawOutput,
    #[serde(default)]
    extension: Extension,
    #[serde(default)]
    channel_share: f64,
    curves: BTreeMap<String, RawCurve>,
    apply: Vec<RawStep>,
}

/// One validated curve: points over x in 0..=1 (first x 0, last x 1), y non-decreasing within 0..=1.
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    pub interpolation: Interpolation,
    pub points: Vec<[f64; 2]>,
    /// Second derivatives at the points for [`Interpolation::NaturalCubicSpline`] (empty otherwise).
    second: Vec<f64>,
}

/// One step of a curve file's `apply` list.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Curve {
        curve: String,
        on: On,
    },
    Mix {
        a: String,
        b: String,
        t: f64,
    },
    /// Linear x in 0..=1 to an encoded value: `(1 + offset) · x^(1/gamma) − offset` above a knee, a straight line
    /// below it (value and slope continuous), `1` at 1. Offset 0 is the plain power.
    Encode {
        gamma: f64,
        offset: f64,
    },
    /// An encoded value in 0..=1 to a display-linear one: `e^gamma` from `knee` up, a straight line below it
    /// (`knee^(gamma − 1) · e`). Knee 0 is the plain power.
    Decode {
        gamma: f64,
        knee: f64,
    },
}

/// The `encode` step.
fn encode(x: f64, gamma: f64, offset: f64) -> f64 {
    let x = if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) };
    let p = 1.0 / gamma;
    if offset <= 0.0 {
        return x.powf(p);
    }
    let knee = (offset / ((1.0 + offset) * (1.0 - p))).powf(gamma);
    if x < knee { (1.0 + offset) * p * knee.powf(p - 1.0) * x } else { (1.0 + offset) * x.powf(p) - offset }
}

/// The `decode` step.
fn decode(e: f64, gamma: f64, knee: f64) -> f64 {
    let e = if e.is_nan() { 0.0 } else { e.clamp(0.0, 1.0) };
    if knee > 0.0 && e < knee { knee.powf(gamma - 1.0) * e } else { e.powf(gamma) }
}

/// Second derivatives of the natural cubic spline through `p` (x strictly increasing).
fn natural_second(p: &[[f64; 2]]) -> Vec<f64> {
    let n = p.len();
    let mut m = vec![0.0; n];
    if n < 3 {
        return m;
    }
    let h: Vec<f64> = (0..n - 1).map(|i| p[i + 1][0] - p[i][0]).collect();
    // tridiagonal system for the interior points, solved forwards and back
    let (mut diag, mut rhs) = (vec![0.0; n], vec![0.0; n]);
    for i in 1..n - 1 {
        diag[i] = 2.0 * (h[i - 1] + h[i]);
        rhs[i] = 6.0 * ((p[i + 1][1] - p[i][1]) / h[i] - (p[i][1] - p[i - 1][1]) / h[i - 1]);
    }
    for i in 2..n - 1 {
        let f = h[i - 1] / diag[i - 1];
        diag[i] -= f * h[i - 1];
        rhs[i] -= f * rhs[i - 1];
    }
    m[n - 2] = rhs[n - 2] / diag[n - 2];
    for i in (1..n - 2).rev() {
        m[i] = (rhs[i] - h[i] * m[i + 1]) / diag[i];
    }
    m
}

/// A parsed, validated curve file.
#[derive(Clone, Debug, PartialEq)]
pub struct BaseCurve {
    pub name: String,
    /// The camera model the curve is meant for (free text, e.g. `Nikon Z 8`; empty: any). A selector lists the curves of the
    /// photo's camera first; rendering ignores it.
    pub camera: String,
    pub input: Input,
    pub extension: Extension,
    /// How much of the curve acts on each colour channel on its own instead of on luminance, 0..=1 (0: luminance only, the
    /// default). See `docs/base-curve-format.md`.
    pub channel_share: f64,
    pub curves: BTreeMap<String, Curve>,
    pub apply: Vec<Step>,
}

impl Curve {
    fn new(interpolation: Interpolation, points: Vec<[f64; 2]>) -> Curve {
        let second = if interpolation == Interpolation::NaturalCubicSpline { natural_second(&points) } else { Vec::new() };
        Curve { interpolation, points, second }
    }

    /// The curve at `x` (clamped to 0..=1).
    pub fn eval(&self, x: f64) -> f64 {
        let p = &self.points;
        let x = if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) };
        let Some(i) = (0..p.len() - 1).find(|&i| x <= p[i + 1][0]) else {
            return p[p.len() - 1][1];
        };
        let (a, b) = (p[i], p[i + 1]);
        let h = b[0] - a[0];
        let t = (x - a[0]) / h;
        match self.interpolation {
            Interpolation::Linear => a[1] + t * (b[1] - a[1]),
            Interpolation::NaturalCubicSpline => {
                let (m0, m1) = (self.second[i], self.second[i + 1]);
                let (u, v) = (b[0] - x, x - a[0]);
                (m0 * u * u * u / (6.0 * h) + m1 * v * v * v / (6.0 * h) + (a[1] / h - m0 * h / 6.0) * u + (b[1] / h - m1 * h / 6.0) * v)
                    .clamp(0.0, 1.0)
            }
            Interpolation::MonotoneCubic => {
                let (m0, m1) = (self.tangent(i), self.tangent(i + 1));
                let (t2, t3) = (t * t, t * t * t);
                ((2.0 * t3 - 3.0 * t2 + 1.0) * a[1] + (t3 - 2.0 * t2 + t) * h * m0 + (-2.0 * t3 + 3.0 * t2) * b[1] + (t3 - t2) * h * m1)
                    .clamp(a[1], b[1])
            }
        }
    }

    /// Fritsch-Carlson tangent at point `i`.
    fn tangent(&self, i: usize) -> f64 {
        let p = &self.points;
        let d = |k: usize| (p[k + 1][1] - p[k][1]) / (p[k + 1][0] - p[k][0]);
        let n = p.len();
        if i == 0 {
            return d(0);
        }
        if i == n - 1 {
            return d(n - 2);
        }
        let (d0, d1) = (d(i - 1), d(i));
        if d0 * d1 <= 0.0 {
            return 0.0;
        }
        let (h0, h1) = (p[i][0] - p[i - 1][0], p[i + 1][0] - p[i][0]);
        let (w1, w2) = (2.0 * h1 + h0, h1 + 2.0 * h0);
        (w1 + w2) / (w1 / d0 + w2 / d1)
    }

    /// Slope of the last segment.
    fn end_slope(&self) -> f64 {
        let n = self.points.len();
        (self.points[n - 1][1] - self.points[n - 2][1]) / (self.points[n - 1][0] - self.points[n - 2][0])
    }

    /// [`Curve::eval`] with the file's extension beyond x = 1 (negative x gives the first value).
    fn eval_extended(&self, x: f64, ext: Extension) -> f64 {
        if x <= 1.0 {
            return self.eval(x);
        }
        let end = self.points[self.points.len() - 1][1];
        let slope = self.end_slope().max(0.0);
        match ext {
            Extension::Clamp => end,
            Extension::Linear => end + slope * (x - 1.0),
            Extension::Rolloff => {
                let gap = 1.0 - end;
                if gap <= 1e-9 || slope <= 0.0 { end } else { 1.0 - gap * (-(slope * (x - 1.0)) / gap).exp() }
            }
        }
    }
}

impl BaseCurve {
    /// Why this build cannot render the curve, if it cannot: this build evaluates luminance curve steps only.
    pub fn unsupported(&self) -> Option<&'static str> {
        self.apply.iter().find_map(|s| match s {
            Step::Curve { on: On::Luminance, .. } | Step::Encode { .. } | Step::Decode { .. } => None,
            Step::Curve { on: On::EachChannel, .. } => Some("per-channel curves are not supported by this version"),
            Step::Mix { .. } => Some("mix steps are not supported by this version"),
        })
    }

    /// The display-linear luminance for scene value `v` (what the curve's `white` and `gain` are relative to), the
    /// steps applied in order: each step's output is the next step's x. Non-finite or negative `v` gives 0.
    ///
    /// Only valid when [`BaseCurve::unsupported`] is `None` (other steps are skipped).
    pub fn eval_luminance(&self, v: f32) -> f32 {
        if !v.is_finite() || v <= 0.0 {
            return 0.0;
        }
        let mut x = self.input.gain * f64::from(v) / self.input.white;
        for s in &self.apply {
            match s {
                Step::Curve { curve, on: On::Luminance } => {
                    if let Some(c) = self.curves.get(curve) {
                        x = c.eval_extended(x, self.extension);
                    }
                }
                Step::Encode { gamma, offset } => x = encode(x, *gamma, *offset),
                Step::Decode { gamma, knee } => x = decode(x, *gamma, *knee),
                _ => {}
            }
        }
        x.max(0.0) as f32
    }
}

fn valid_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 32 && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn validate_curve(name: &str, c: &RawCurve) -> Result<Curve, String> {
    let p = &c.points;
    if p.len() < 2 || p.len() > MAX_POINTS {
        return Err(format!("curve {name}: {} points (2 to {MAX_POINTS})", p.len()));
    }
    if p.iter().flatten().any(|v| !v.is_finite()) {
        return Err(format!("curve {name}: a point is not a finite number"));
    }
    if p[0][0] != 0.0 || p[p.len() - 1][0] != 1.0 {
        return Err(format!("curve {name}: x must run from 0 to 1"));
    }
    for w in p.windows(2) {
        if w[1][0] <= w[0][0] {
            return Err(format!("curve {name}: x is not strictly increasing"));
        }
        if w[1][1] < w[0][1] {
            return Err(format!("curve {name}: y decreases"));
        }
    }
    if p[0][1] < 0.0 || p[p.len() - 1][1] > 1.0 {
        return Err(format!("curve {name}: y must lie within 0 to 1"));
    }
    Ok(Curve::new(c.interpolation, p.clone()))
}

/// Parse and validate a curve file. Unknown fields are ignored; an unknown step, encoding or interpolation, a
/// version above [`VERSION`], or any limit exceeded is an error.
pub fn parse(bytes: &[u8]) -> Result<BaseCurve, String> {
    if bytes.len() as u64 > MAX_FILE {
        return Err("file too large".into());
    }
    let f: RawFile = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if f.format != FORMAT {
        return Err(format!("not a base curve file (format {:?})", f.format));
    }
    if f.version == 0 || f.version > VERSION {
        return Err(format!("format version {} is not supported (this build reads up to {VERSION})", f.version));
    }
    let _ = f.output.encoding;
    let _ = f.input.encoding;
    if !(f.input.gain.is_finite()
        && f.input.gain > 0.0
        && f.input.gain <= 1000.0
        && f.input.white.is_finite()
        && f.input.white > 0.0
        && f.input.white <= 1000.0)
    {
        return Err("input gain and white must be finite numbers above 0".into());
    }
    if f.curves.is_empty() || f.curves.len() > MAX_CURVES {
        return Err(format!("{} curves (1 to {MAX_CURVES})", f.curves.len()));
    }
    let mut curves = BTreeMap::new();
    for (name, c) in &f.curves {
        if !valid_name(name) {
            return Err(format!("curve name {name:?} is not valid"));
        }
        if f.version < 2 && c.interpolation == Interpolation::NaturalCubicSpline {
            return Err(format!("curve {name}: the natural-cubic-spline interpolation needs format version 2"));
        }
        curves.insert(name.clone(), validate_curve(name, c)?);
    }
    if f.apply.is_empty() || f.apply.len() > MAX_STEPS {
        return Err(format!("{} steps (1 to {MAX_STEPS})", f.apply.len()));
    }
    let mut apply = Vec::new();
    for s in f.apply {
        match s {
            RawStep::Curve { curve, on } => {
                if !curves.contains_key(&curve) {
                    return Err(format!("step names a curve that does not exist: {curve}"));
                }
                apply.push(Step::Curve { curve, on });
            }
            RawStep::Mix { a, b, t } => {
                if !curves.contains_key(&a) || !curves.contains_key(&b) {
                    return Err("mix step names a curve that does not exist".into());
                }
                if !(t.is_finite() && (0.0..=1.0).contains(&t)) {
                    return Err("mix weight must be within 0 to 1".into());
                }
                apply.push(Step::Mix { a, b, t });
            }
            RawStep::Encode { kind, gamma, offset } => {
                let EncodeKind::PowerSegment = kind;
                if f.version < 2 {
                    return Err("the encode step needs format version 2".into());
                }
                // the straight part must end before x = 1 (knee below 1), which holds for offset below gamma − 1
                if !(gamma.is_finite() && gamma > 1.0 && gamma <= 4.0 && offset.is_finite() && (0.0..=1.0).contains(&offset) && offset < gamma - 1.0)
                {
                    return Err("encode: gamma must be above 1 and at most 4, offset within 0 to 1 and below gamma − 1".into());
                }
                apply.push(Step::Encode { gamma, offset });
            }
            RawStep::Decode { kind, gamma, knee } => {
                let DecodeKind::PowerToe = kind;
                if f.version < 2 {
                    return Err("the decode step needs format version 2".into());
                }
                if !(gamma.is_finite() && (1.0..=4.0).contains(&gamma) && knee.is_finite() && (0.0..1.0).contains(&knee)) {
                    return Err("decode: gamma must be within 1 to 4, knee within 0 to below 1".into());
                }
                apply.push(Step::Decode { gamma, knee });
            }
        }
    }
    if !(f.channel_share.is_finite() && (0.0..=1.0).contains(&f.channel_share)) {
        return Err("channel_share must be within 0 to 1".into());
    }
    if f.channel_share != 0.0 && f.version < 2 {
        return Err("channel_share needs format version 2".into());
    }
    let name: String = f.name.chars().filter(|c| !c.is_control()).take(120).collect();
    let camera: String = f.camera.chars().filter(|c| !c.is_control()).take(80).collect();
    Ok(BaseCurve {
        name,
        camera,
        input: Input { gain: f.input.gain, white: f.input.white },
        extension: f.extension,
        channel_share: f.channel_share,
        curves,
        apply,
    })
}

/// FNV-1a over a curve file's bytes: the content hash that goes into the settings hash.
pub fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// The id of the curve file `stem`.
pub fn id_for(stem: &str) -> String {
    format!("{ID_PREFIX}{stem}")
}

/// Whether `id` has the form of a base curve id (`base:<stem>`, no path separators or control characters).
pub fn is_valid_id(id: &str) -> bool {
    id.strip_prefix(ID_PREFIX)
        .is_some_and(|s| !s.is_empty() && id.len() <= MAX_ID && !s.chars().any(|c| c.is_control() || matches!(c, '/' | '\\' | ':')))
}

/// One installed curve file.
#[derive(Clone, Debug)]
pub struct Entry {
    pub id: String,
    pub curve: Arc<BaseCurve>,
    /// [`hash_bytes`] of the file.
    pub hash: u64,
}

fn registry() -> &'static RwLock<Vec<Entry>> {
    static R: OnceLock<RwLock<Vec<Entry>>> = OnceLock::new();
    R.get_or_init(|| RwLock::new(Vec::new()))
}

/// Replace the installed curves.
pub fn install_library(list: Vec<(String, BaseCurve, u64)>) {
    let entries = list.into_iter().map(|(id, c, hash)| Entry { id, curve: Arc::new(c), hash }).collect();
    *registry().write().unwrap_or_else(|e| e.into_inner()) = entries;
}

/// The installed curves, in id order as installed.
pub fn library() -> Vec<Entry> {
    registry().read().unwrap_or_else(|e| e.into_inner()).clone()
}

pub fn get(id: &str) -> Option<Entry> {
    registry().read().unwrap_or_else(|e| e.into_inner()).iter().find(|e| e.id == id).cloned()
}

/// Content hash of the installed curve `id`: what a changed curve file changes in the settings hash. `None`: not installed.
pub fn content_hash(id: &str) -> Option<u64> {
    registry().read().unwrap_or_else(|e| e.into_inner()).iter().find(|e| e.id == id).map(|e| e.hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn file(points: serde_json::Value) -> Vec<u8> {
        json!({
            "format": "base-curve", "version": 1, "name": "synthetic",
            "curves": { "a": { "interpolation": "linear", "points": points } },
            "apply": [ { "op": "curve", "curve": "a", "on": "luminance" } ]
        })
        .to_string()
        .into_bytes()
    }

    fn doc() -> serde_json::Value {
        json!({
            "format": "base-curve", "version": 1,
            "curves": { "a": { "interpolation": "linear", "points": [[0, 0], [0.5, 0.8], [1, 1]] },
                        "b": { "interpolation": "monotone-cubic", "points": [[0, 0], [0.25, 0.5], [1, 1]] } },
            "apply": [ { "op": "curve", "curve": "a", "on": "luminance" } ]
        })
    }

    #[test]
    fn a_valid_file_parses_with_defaults() {
        let c = parse(&file(json!([[0, 0], [0.5, 0.8], [1, 1]]))).unwrap();
        assert_eq!(c.name, "synthetic");
        assert_eq!((c.input.gain, c.input.white, c.extension), (1.0, 1.0, Extension::Clamp));
        assert_eq!(c.curves["a"].points.len(), 3);
        assert!(c.unsupported().is_none());
    }

    #[test]
    fn the_camera_is_optional_text_and_never_affects_the_curve() {
        let mut d = doc();
        assert_eq!(parse(d.to_string().as_bytes()).unwrap().camera, "");
        d["camera"] = json!("Some Maker X 1\u{7}");
        let with = parse(d.to_string().as_bytes()).unwrap();
        assert_eq!(with.camera, "Some Maker X 1");
        assert_eq!(with.eval_luminance(0.3), parse(doc().to_string().as_bytes()).unwrap().eval_luminance(0.3));
        d["camera"] = json!("y".repeat(500));
        assert_eq!(parse(d.to_string().as_bytes()).unwrap().camera.len(), 80);
        d["camera"] = json!(5);
        assert!(parse(d.to_string().as_bytes()).is_err(), "a camera that is not text");
    }

    #[test]
    fn linear_interpolation_hits_the_points() {
        let c = parse(&file(json!([[0, 0], [0.5, 0.8], [1, 1]]))).unwrap();
        for (v, y) in [(0.0, 0.0), (0.25, 0.4), (0.5, 0.8), (0.75, 0.9), (1.0, 1.0)] {
            assert!((c.eval_luminance(v) - y).abs() < 1e-6, "{v}");
        }
        assert_eq!(c.eval_luminance(-1.0), 0.0);
        assert_eq!(c.eval_luminance(f32::NAN), 0.0);
        assert_eq!(c.eval_luminance(5.0), 1.0, "clamp holds the last value");
    }

    #[test]
    fn monotone_cubic_hits_points_and_stays_monotone_and_bounded() {
        let mut d = doc();
        d["apply"] = json!([{ "op": "curve", "curve": "b", "on": "luminance" }]);
        let c = parse(d.to_string().as_bytes()).unwrap();
        for (v, y) in [(0.0, 0.0), (0.25, 0.5), (1.0, 1.0)] {
            assert!((c.eval_luminance(v) - y).abs() < 1e-6, "{v}");
        }
        let mut prev = 0.0;
        for i in 0..=1000 {
            let y = c.eval_luminance(i as f32 / 1000.0);
            assert!(y >= prev - 1e-6 && (0.0..=1.0 + 1e-6).contains(&y), "{i}: {y}");
            prev = y;
        }
    }

    #[test]
    fn gain_white_and_steps_compose() {
        let mut d = doc();
        d["input"] = json!({ "encoding": "linear", "gain": 2.0, "white": 4.0 });
        d["apply"] = json!([{ "op": "curve", "curve": "a", "on": "luminance" }, { "op": "curve", "curve": "a", "on": "luminance" }]);
        let c = parse(d.to_string().as_bytes()).unwrap();
        // x = 2 · 1 / 4 = 0.5 → 0.8 → (0.5..1: 0.8 + (0.3/0.5)·0.2) = 0.92
        assert!((c.eval_luminance(1.0) - 0.92).abs() < 1e-6);
    }

    #[test]
    fn extensions_beyond_the_domain() {
        let mut d = doc();
        d["curves"]["a"]["points"] = json!([[0, 0], [0.5, 0.4], [1, 0.8]]);
        for (ext, f) in [("clamp", 0.8f32), ("linear", 0.8 + 0.8 * 1.0), ("rolloff", 0.0)] {
            d["extension"] = json!(ext);
            let c = parse(d.to_string().as_bytes()).unwrap();
            let y = c.eval_luminance(2.0);
            if ext == "rolloff" {
                assert!(y > 0.8 && y < 1.0, "{y}");
                assert!(c.eval_luminance(50.0) <= 1.0);
            } else {
                assert!((y - f).abs() < 1e-6, "{ext}: {y}");
            }
        }
    }

    #[test]
    fn unsupported_steps_load_but_are_reported() {
        let mut d = doc();
        d["apply"] = json!([{ "op": "curve", "curve": "a", "on": "each-channel" }]);
        assert!(parse(d.to_string().as_bytes()).unwrap().unsupported().is_some());
        d["apply"] = json!([{ "op": "mix", "a": "a", "b": "b", "t": 0.5 }]);
        assert!(parse(d.to_string().as_bytes()).unwrap().unsupported().is_some());
    }

    #[test]
    fn unknown_fields_are_ignored_and_bad_files_are_rejected() {
        let mut d = doc();
        d["notes"] = json!({ "anything": [1, 2, 3] });
        d["something_new"] = json!(true);
        d["curves"]["a"]["future"] = json!(1);
        assert!(parse(d.to_string().as_bytes()).is_ok());
        let bad = |f: &dyn Fn(&mut serde_json::Value)| {
            let mut d = doc();
            f(&mut d);
            assert!(parse(d.to_string().as_bytes()).is_err(), "{d}");
        };
        bad(&|d| d["format"] = json!("other"));
        bad(&|d| d["version"] = json!(3));
        bad(&|d| d["version"] = json!(0));
        bad(&|d| d["curves"]["a"]["points"] = json!([[0, 0]]));
        bad(&|d| d["curves"]["a"]["points"] = json!([[0, 0], [0.6, 0.5], [0.5, 0.6], [1, 1]]));
        bad(&|d| d["curves"]["a"]["points"] = json!([[0, 0], [0.5, 0.9], [0.7, 0.2], [1, 1]]));
        bad(&|d| d["curves"]["a"]["points"] = json!([[0.1, 0], [1, 1]]));
        bad(&|d| d["curves"]["a"]["points"] = json!([[0, 0], [1, 1.5]]));
        bad(&|d| d["curves"]["a"]["points"] = json!([[0, 0], [1, null]]));
        bad(&|d| d["curves"]["a"]["interpolation"] = json!("spline"));
        bad(&|d| d["input"] = json!({ "encoding": "log2" }));
        bad(&|d| d["input"] = json!({ "gain": 0 }));
        bad(&|d| d["output"] = json!({ "encoding": "srgb" }));
        bad(&|d| d["apply"] = json!([]));
        bad(&|d| d["apply"] = json!([{ "op": "curve", "curve": "nope", "on": "luminance" }]));
        bad(&|d| d["apply"] = json!([{ "op": "warp", "curve": "a", "on": "luminance" }]));
        bad(&|d| d["apply"] = json!([{ "op": "mix", "a": "a", "b": "b", "t": 2.0 }]));
        bad(&|d| d["curves"] = json!({}));
        bad(&|d| d["curves"]["a b"] = d["curves"]["a"].clone());
        bad(&|d| d["apply"] = json!((0..9).map(|_| json!({ "op": "curve", "curve": "a", "on": "luminance" })).collect::<Vec<_>>()));
        bad(&|d| d["curves"]["a"]["points"] = json!((0..=300).map(|i| json!([i as f64 / 300.0, i as f64 / 300.0])).collect::<Vec<_>>()));
        assert!(parse(b"not json").is_err());
        assert!(parse(&vec![b' '; (MAX_FILE as usize) + 1]).is_err());
    }

    fn v2(curve: serde_json::Value, apply: serde_json::Value) -> serde_json::Value {
        json!({ "format": "base-curve", "version": 2, "curves": { "a": curve }, "apply": apply })
    }

    #[test]
    fn the_natural_cubic_spline_matches_the_closed_form_and_hits_its_points() {
        // three points: the one interior second derivative solves 2 (h0 + h1) m = 6 (d1 − d0) = −7.2, so m = −3.6,
        // and at x = 0.25 the spline is −3.6 · 0.25³ / (6 · 0.5) + (0.8 / 0.5 + 3.6 · 0.5 / 6) · 0.25 = 0.45625
        let d = v2(
            json!({ "interpolation": "natural-cubic-spline", "points": [[0, 0], [0.5, 0.8], [1, 1]] }),
            json!([{ "op": "curve", "curve": "a", "on": "luminance" }]),
        );
        let c = parse(d.to_string().as_bytes()).unwrap();
        assert!((c.eval_luminance(0.25) - 0.45625).abs() < 1e-6, "{}", c.eval_luminance(0.25));
        for (v, y) in [(0.0, 0.0), (0.5, 0.8), (1.0, 1.0)] {
            assert!((c.eval_luminance(v) - y).abs() < 1e-6, "{v}");
        }
        // two points: a straight line
        let d = v2(
            json!({ "interpolation": "natural-cubic-spline", "points": [[0, 0.1], [1, 0.7]] }),
            json!([{ "op": "curve", "curve": "a", "on": "luminance" }]),
        );
        assert!((parse(d.to_string().as_bytes()).unwrap().eval_luminance(0.5) - 0.4).abs() < 1e-6);
    }

    #[test]
    fn the_spline_is_smooth_where_a_piecewise_one_is_not_and_stays_within_zero_and_one() {
        let pts = json!([[0, 0], [0.05, 0.3], [0.1, 0.32], [0.4, 0.8], [0.6, 0.85], [1, 1]]);
        let d = v2(json!({ "interpolation": "natural-cubic-spline", "points": pts }), json!([{ "op": "curve", "curve": "a", "on": "luminance" }]));
        let c = parse(d.to_string().as_bytes()).unwrap();
        let f = |x: f64| c.curves["a"].eval(x);
        // continuous first derivative at every interior knot (the slopes either side agree)
        for x in [0.05, 0.1, 0.4, 0.6] {
            let e = 1e-6;
            let (l, r) = ((f(x) - f(x - e)) / e, (f(x + e) - f(x)) / e);
            assert!((l - r).abs() < 1e-3 * (1.0 + l.abs()), "{x}: {l} against {r}");
        }
        for i in 0..=2000 {
            let y = f(i as f64 / 2000.0);
            assert!((0.0..=1.0).contains(&y), "{i}: {y}");
        }
    }

    #[test]
    fn version_two_features_need_version_two() {
        let steps = json!([{ "op": "curve", "curve": "a", "on": "luminance" }]);
        let mut d = v2(json!({ "interpolation": "natural-cubic-spline", "points": [[0, 0], [1, 1]] }), steps.clone());
        assert!(parse(d.to_string().as_bytes()).is_ok());
        d["version"] = json!(1);
        assert!(parse(d.to_string().as_bytes()).is_err(), "spline in a version 1 file");
        let mut d = v2(json!({ "interpolation": "linear", "points": [[0, 0], [1, 1]] }), json!([{ "op": "encode", "gamma": 2.0, "offset": 0.1 }]));
        assert!(parse(d.to_string().as_bytes()).is_ok());
        d["version"] = json!(1);
        assert!(parse(d.to_string().as_bytes()).is_err(), "encode in a version 1 file");
        let mut d = v2(json!({ "interpolation": "linear", "points": [[0, 0], [1, 1]] }), json!([{ "op": "decode", "gamma": 1.5 }]));
        assert!(parse(d.to_string().as_bytes()).is_ok());
        d["version"] = json!(1);
        assert!(parse(d.to_string().as_bytes()).is_err(), "decode in a version 1 file");
    }

    #[test]
    fn encode_and_decode_are_bounded_monotone_and_continuous_with_the_right_ends() {
        for (gamma, offset) in [(1.5, 0.0), (2.5, 0.12), (3.0, 0.3), (1.2, 0.19)] {
            assert!((encode(0.0, gamma, offset)).abs() < 1e-12 && (encode(1.0, gamma, offset) - 1.0).abs() < 1e-12, "{gamma} {offset}");
            let mut prev = -1.0;
            for i in 0..=4000 {
                let e = encode(i as f64 / 4000.0, gamma, offset);
                assert!(e >= prev - 1e-12 && (0.0..=1.0 + 1e-12).contains(&e), "{gamma} {offset} {i}");
                if i > 0 {
                    assert!(e - prev < 0.05 + 4.0 * offset, "a jump at {i}: {prev} to {e}");
                }
                prev = e;
            }
        }
        for (gamma, knee) in [(1.0, 0.0), (1.8, 0.0), (2.2, 0.02), (1.5, 0.5)] {
            assert!(decode(0.0, gamma, knee).abs() < 1e-12 && (decode(1.0, gamma, knee) - 1.0).abs() < 1e-12);
            let mut prev = -1.0;
            for i in 0..=4000 {
                let y = decode(i as f64 / 4000.0, gamma, knee);
                assert!(y >= prev - 1e-12 && (0.0..=1.0 + 1e-12).contains(&y), "{gamma} {knee} {i}");
                prev = y;
            }
            // continuous at the knee
            if knee > 0.0 {
                let (a, b) = (decode(knee - 1e-9, gamma, knee), decode(knee + 1e-9, gamma, knee));
                assert!((a - b).abs() < 1e-7, "{gamma} {knee}: {a} {b}");
            }
        }
        // the encode is continuous in value and slope where its straight part meets the power
        let (gamma, offset): (f64, f64) = (2.5, 0.12);
        let p = 1.0 / gamma;
        let knee = (offset / ((1.0 + offset) * (1.0 - p))).powf(gamma);
        let (a, b) = (encode(knee - 1e-9, gamma, offset), encode(knee + 1e-9, gamma, offset));
        assert!((a - b).abs() < 1e-7, "{a} {b}");
        let (sl, sr) = (
            (encode(knee - 1e-6, gamma, offset) - encode(knee - 2e-6, gamma, offset)) / 1e-6,
            (encode(knee + 2e-6, gamma, offset) - encode(knee + 1e-6, gamma, offset)) / 1e-6,
        );
        assert!((sl - sr).abs() < 1e-3 * sl, "{sl} {sr}");
    }

    #[test]
    fn a_curve_then_encode_then_decode_composes_in_order() {
        let steps = json!([{ "op": "curve", "curve": "a", "on": "luminance" }, { "op": "encode", "gamma": 2.5, "offset": 0.12 }, { "op": "decode", "gamma": 1.7, "knee": 0.01 }]);
        let mut d = v2(json!({ "interpolation": "natural-cubic-spline", "points": [[0, 0], [0.5, 0.6], [1, 1]] }), steps);
        d["input"] = json!({ "gain": 0.9 });
        let c = parse(d.to_string().as_bytes()).unwrap();
        for v in [0.0f32, 0.001, 0.02, 0.2, 0.6, 1.0] {
            let x = 0.9 * f64::from(v);
            let want = decode(encode(c.curves["a"].eval(x), 2.5, 0.12), 1.7, 0.01) as f32;
            assert!((c.eval_luminance(v) - want).abs() < 1e-6, "{v}: {} {want}", c.eval_luminance(v));
        }
        // above x = 1 the clamped curve holds 1, so the whole transfer holds white
        assert!((c.eval_luminance(5.0) - 1.0).abs() < 1e-6);
        // the steps are monotone together
        let mut prev = 0.0;
        for i in 0..=3000 {
            let y = c.eval_luminance(i as f32 / 1000.0);
            assert!(y >= prev - 1e-6, "{i}");
            prev = y;
        }
        // bad parameters are rejected
        let bad = |step: serde_json::Value| {
            let d = v2(json!({ "interpolation": "linear", "points": [[0, 0], [1, 1]] }), json!([step]));
            assert!(parse(d.to_string().as_bytes()).is_err(), "{d}");
        };
        bad(json!({ "op": "encode", "gamma": 1.0 }));
        bad(json!({ "op": "encode", "gamma": 5.0 }));
        bad(json!({ "op": "encode", "gamma": 2.0, "offset": -0.1 }));
        bad(json!({ "op": "encode", "gamma": 2.0, "offset": 1.5 }));
        bad(json!({ "op": "encode", "gamma": 1.2, "offset": 0.5 }));
        bad(json!({ "op": "encode", "kind": "log", "gamma": 2.0 }));
        bad(json!({ "op": "decode", "gamma": 0.5 }));
        bad(json!({ "op": "decode", "gamma": 2.0, "knee": 1.0 }));
        bad(json!({ "op": "decode", "kind": "x", "gamma": 2.0 }));
        bad(json!({ "op": "encode" }));
    }

    #[test]
    fn channel_share_is_optional_validated_and_needs_version_2() {
        let with = |version: u32, share: serde_json::Value| {
            let mut d = doc();
            d["version"] = json!(version);
            d["channel_share"] = share;
            parse(d.to_string().as_bytes())
        };
        assert_eq!(parse(doc().to_string().as_bytes()).unwrap().channel_share, 0.0);
        assert_eq!(with(2, json!(0.5)).unwrap().channel_share, 0.5);
        assert_eq!(with(2, json!(0)).unwrap().channel_share, 0.0);
        assert_eq!(with(2, json!(1)).unwrap().channel_share, 1.0);
        assert_eq!(with(1, json!(0)).unwrap().channel_share, 0.0);
        for bad in [json!(-0.1), json!(1.0001), json!(2), json!("half")] {
            assert!(with(2, bad.clone()).is_err(), "{bad}");
        }
        assert!(with(1, json!(0.5)).is_err(), "version 1 has no channel share");
    }

    #[test]
    fn ids_and_hashes() {
        assert_eq!(id_for("Flat"), "base:Flat");
        assert!(is_valid_id("base:Flat 2"));
        for bad in ["", "base:", "curve:x", "base:a/b", "base:a\\b", "base:..:x", "base:a\u{7}"] {
            assert!(!is_valid_id(bad), "{bad:?}");
        }
        assert_ne!(hash_bytes(b"a"), hash_bytes(b"b"));
        assert_eq!(hash_bytes(b"a"), hash_bytes(b"a"));
    }
}
