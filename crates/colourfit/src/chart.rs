//! Colour-chart calibration: a camera's colour matrices measured from raw shots of an X-Rite ColorChecker Classic
//! (24 patches), under one or two lights.
//!
//! For each shot the person gives the centres of the chart's four corner patches (as fractions of the photo's width
//! and height, in any order around the chart); a homography places the 24 patch centres, and the orientation is the
//! one of the eight ways round whose colours fit best. Each patch is the per-channel median of a box in its middle,
//! read from the raw's camera RGB (black-subtracted, white = 1, not white balanced). The neutral patches give the
//! shot's camera neutral and exposure; a white-preserving forward matrix (white-balanced camera RGB → XYZ D50) is
//! fitted to the chart's published reference values, by least squares in XYZ and then Levenberg–Marquardt on
//! CIE L*a*b* error. With the light's white `w` and the measured neutral `n`, the colour matrix follows exactly:
//! `ColorMatrix = diag(n) · ForwardMatrix⁻¹ · Bradford(w → D50)`, so the DNG model reads the shot's white as `w`.
//!
//! The result is a camera database entry (`lcc_camdb::Source::Chart`, measured: ranked above every estimate) and,
//! for the person's own use, a `.dcp`. With 24 patches the fit is a 3 × 3 matrix per light; no hue/saturation table.
//!
//! Sources: own design; DNG specification 1.7 chapter 6 (colour model); CIEDE2000 (Sharma, Wu, Dalal 2005);
//! reference values: X-Rite's 2016 ColorChecker Classic data (CIE L*a*b*, ICC D50), as collected in colour-science
//! (BSD-3-Clause).
use crate::deltae::{de2000, lab};
use crate::fit::linalg;
use crate::{Error, Result};
use lcc_color::{D50, Mat3, Xy};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Patch names, row by row from the dark-skin corner.
pub const NAMES: [&str; 24] = [
    "dark skin",
    "light skin",
    "blue sky",
    "foliage",
    "blue flower",
    "bluish green",
    "orange",
    "purplish blue",
    "moderate red",
    "purple",
    "yellow green",
    "orange yellow",
    "blue",
    "green",
    "red",
    "yellow",
    "magenta",
    "cyan",
    "white 9.5",
    "neutral 8",
    "neutral 6.5",
    "neutral 5",
    "neutral 3.5",
    "black 2",
];

/// X-Rite (2016), ColorChecker Classic made after November 2014: CIE L*a*b* under ICC D50.
const LAB_2014: [[f64; 3]; 24] = [
    [37.54, 14.37, 14.92],
    [64.66, 19.27, 17.5],
    [49.32, -3.82, -22.54],
    [43.46, -12.74, 22.72],
    [54.94, 9.61, -24.79],
    [70.48, -32.26, -0.37],
    [62.73, 35.83, 56.5],
    [39.43, 10.75, -45.17],
    [50.57, 48.64, 16.67],
    [30.1, 22.54, -20.87],
    [71.77, -24.13, 58.19],
    [71.51, 18.24, 67.37],
    [28.37, 15.42, -49.8],
    [54.38, -39.72, 32.27],
    [42.43, 51.05, 28.62],
    [81.8, 2.67, 80.41],
    [50.63, 51.28, -14.12],
    [49.57, -29.71, -28.32],
    [95.19, -1.03, 2.93],
    [81.29, -0.57, 0.44],
    [66.89, -0.75, -0.06],
    [50.76, -0.13, 0.14],
    [35.63, -0.46, -0.48],
    [20.64, 0.07, -0.46],
];

/// X-Rite (2016), ColorChecker Classic made before November 2014 (matches GretagMacbeth's 2005 values).
const LAB_2005: [[f64; 3]; 24] = [
    [37.986, 13.555, 14.059],
    [65.711, 18.13, 17.81],
    [49.927, -4.88, -21.905],
    [43.139, -13.095, 21.905],
    [55.112, 8.844, -25.399],
    [70.719, -33.397, -0.199],
    [62.661, 36.067, 57.096],
    [40.02, 10.41, -45.964],
    [51.124, 48.239, 16.248],
    [30.325, 22.976, -21.587],
    [72.532, -23.709, 57.255],
    [71.941, 19.363, 67.857],
    [28.778, 14.179, -50.297],
    [55.261, -38.342, 31.37],
    [42.101, 53.378, 28.19],
    [81.733, 4.039, 79.819],
    [51.935, 49.986, -14.574],
    [51.038, -28.631, -28.638],
    [96.539, -0.425, 1.186],
    [81.257, -0.638, -0.335],
    [66.766, -0.734, -0.504],
    [50.867, -0.153, -0.27],
    [35.656, -0.421, -1.231],
    [20.461, -0.079, -0.973],
];

/// Which edition of the chart (X-Rite changed the pigments in November 2014).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub enum Edition {
    #[default]
    After2014,
    Before2014,
}

impl Edition {
    pub fn parse(s: &str) -> Option<Edition> {
        match s.trim().to_lowercase().as_str() {
            "2014" | "after2014" | "new" | "current" => Some(Edition::After2014),
            "2005" | "before2014" | "old" | "classic2005" => Some(Edition::Before2014),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Edition::After2014 => "ColorChecker Classic (after November 2014)",
            Edition::Before2014 => "ColorChecker Classic (before November 2014)",
        }
    }

    /// The reference colours as XYZ relative to D50 (white Y = 1).
    pub fn reference_xyz(self) -> [[f64; 3]; 24] {
        let lab = match self {
            Edition::After2014 => &LAB_2014,
            Edition::Before2014 => &LAB_2005,
        };
        lab.map(lab_to_xyz)
    }
}

/// ICC D50 white (Y = 1).
const WHITE_D50: [f64; 3] = [0.9642, 1.0, 0.8249];
/// Neutral patches used for the white and the exposure (white 9.5 is a little yellow, black 2 too noisy).
const NEUTRALS: [usize; 4] = [19, 20, 21, 22];
/// Camera value at or above which a patch counts as clipped.
const CLIP: f64 = 0.97;
/// Half the side of the box read in each patch, as a fraction of the patch pitch: the middle half of a patch, clear of
/// the black gaps and of a little misplacement.
const PATCH_HALF: f64 = 0.25;

fn lab_to_xyz(l: [f64; 3]) -> [f64; 3] {
    let fy = (l[0] + 16.0) / 116.0;
    let fx = fy + l[1] / 500.0;
    let fz = fy - l[2] / 200.0;
    let inv = |t: f64| if t > 6.0 / 29.0 { t * t * t } else { 3.0 * (6.0f64 / 29.0).powi(2) * (t - 4.0 / 29.0) };
    [WHITE_D50[0] * inv(fx), WHITE_D50[1] * inv(fy), WHITE_D50[2] * inv(fz)]
}

/// The light a shot was taken under.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Light {
    /// Its white.
    pub xy: [f64; 2],
    /// Its correlated colour temperature (K).
    pub kelvin: f64,
    /// The DNG `CalibrationIlluminant` code the matrices are filed under (the standard light nearest in mired).
    pub code: u16,
}

impl Light {
    /// `d65`, `d55` (also `daylight`), `d50`, `d75` (also `shade`), `a` (also `tungsten`, `incandescent`), or a
    /// temperature in kelvin (`3200`, `3200k`), read on the Planckian locus.
    pub fn parse(s: &str) -> Option<Light> {
        let s = s.trim().to_lowercase();
        let named = |x: f64, y: f64, k: f64, code: u16| Some(Light { xy: [x, y], kelvin: k, code });
        match s.as_str() {
            "d65" => named(0.3127, 0.3290, 6504.0, 21),
            "d55" | "daylight" | "sun" => named(0.3324, 0.3474, 5503.0, 20),
            "d50" => named(0.3457, 0.3585, 5003.0, 23),
            "d75" | "shade" => named(0.2990, 0.3149, 7504.0, 22),
            "a" | "tungsten" | "incandescent" => named(0.44757, 0.40745, 2856.0, 17),
            other => {
                let k: f64 = other.trim_end_matches('k').trim().parse().ok()?;
                if !(k.is_finite() && (1800.0..=12000.0).contains(&k)) {
                    return None;
                }
                let xy = lcc_color::cct::temp_tint_to_xy(k, 0.0);
                let code = [(17u16, 2856.0), (23, 5003.0), (20, 5503.0), (21, 6504.0), (22, 7504.0)]
                    .into_iter()
                    .min_by(|a, b| (1e6 / a.1 - 1e6 / k).abs().total_cmp(&(1e6 / b.1 - 1e6 / k).abs()))
                    .map(|(c, _)| c)
                    .unwrap_or(21);
                Some(Light { xy: [xy.x, xy.y], kelvin: k, code })
            }
        }
    }

    fn white(&self) -> Xy {
        Xy::new(self.xy[0], self.xy[1])
    }
}

/// One chart shot as given.
#[derive(Clone, Debug)]
pub struct Shot {
    pub path: PathBuf,
    pub light: Light,
    /// The centres of the four corner patches, as fractions (x of the width, y of the height) of the photo as the
    /// app shows it (default crop, oriented), in any order.
    pub corners: [[f64; 2]; 4],
}

/// A shot's camera data at the 24 patches.
#[derive(Clone, Debug)]
pub struct Sampled {
    pub make: String,
    pub model: String,
    /// Camera RGB per patch (black-subtracted, white = 1, not white balanced).
    pub camera: [[f64; 3]; 24],
    /// Patches with a clipped channel (left out of the fit).
    pub clipped: Vec<usize>,
    /// Each patch's sampling box `[x0, y0, x1, y1]` (fractions), for the preview.
    pub boxes: Vec<[f64; 4]>,
    /// The developed shot (camera RGB, oriented), for the preview.
    pub image: lcc_raster::Rgb32f,
    /// The raw's own colour data, for the before numbers.
    pub color: lcc_raw::ColorData,
}

/// The homography taking the unit square's corners (0,0), (1,0), (1,1), (0,1) to `p`, as a 3 × 3 row-major matrix.
fn homography(p: &[[f64; 2]; 4]) -> Option<[[f64; 3]; 3]> {
    let src = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    let mut a = Vec::with_capacity(8);
    let mut b = Vec::with_capacity(8);
    for (s, d) in src.iter().zip(p) {
        let (x, y, u, v) = (s[0], s[1], d[0], d[1]);
        a.push(vec![x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y]);
        b.push(u);
        a.push(vec![0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y]);
        b.push(v);
    }
    let h = linalg::solve(a, b)?;
    let g = |i: usize| h.get(i).copied().unwrap_or(0.0);
    let m = [[g(0), g(1), g(2)], [g(3), g(4), g(5)], [g(6), g(7), 1.0]];
    m.iter().flatten().all(|v| v.is_finite()).then_some(m)
}

fn map(h: &[[f64; 3]; 3], x: f64, y: f64) -> Option<[f64; 2]> {
    let w = h[2][0] * x + h[2][1] * y + h[2][2];
    if w.abs() < 1e-12 {
        return None;
    }
    let p = [(h[0][0] * x + h[0][1] * y + h[0][2]) / w, (h[1][0] * x + h[1][1] * y + h[1][2]) / w];
    p.iter().all(|v| v.is_finite()).then_some(p)
}

/// The eight ways the four corners can stand for (dark skin, bluish green, black, white) going round the chart.
fn orientations(corners: &[[f64; 2]; 4]) -> Vec<[[f64; 2]; 4]> {
    let c = [corners.iter().map(|p| p[0]).sum::<f64>() / 4.0, corners.iter().map(|p| p[1]).sum::<f64>() / 4.0];
    let mut ring: Vec<[f64; 2]> = corners.to_vec();
    ring.sort_by(|a, b| (a[1] - c[1]).atan2(a[0] - c[0]).total_cmp(&(b[1] - c[1]).atan2(b[0] - c[0])));
    let at = |i: usize| ring.get(i % 4).copied().unwrap_or([0.0, 0.0]);
    let mut out = Vec::with_capacity(8);
    for start in 0..4 {
        out.push([at(start), at(start + 1), at(start + 2), at(start + 3)]);
        out.push([at(start + 4), at(start + 3), at(start + 2), at(start + 1)]);
    }
    out
}

/// Patch centres (fractions) for corners `p` (dark skin, bluish green, black, white): 6 columns × 4 rows.
fn centres(p: &[[f64; 2]; 4]) -> Option<(Vec<[f64; 2]>, [[f64; 3]; 3])> {
    let h = homography(p)?;
    let mut out = Vec::with_capacity(24);
    for row in 0..4 {
        for col in 0..6 {
            out.push(map(&h, col as f64 / 5.0, row as f64 / 3.0)?);
        }
    }
    Some((out, h))
}

/// Per-channel median of `img` over the box around unit-grid point (`u`, `v`) of half-size `half` (grid units).
fn read_patch(img: &lcc_raster::Rgb32f, h: &[[f64; 3]; 3], u: f64, v: f64, half: [f64; 2]) -> Option<([f64; 3], f64, [f64; 4])> {
    const N: usize = 9;
    let (w, ht) = (img.width as f64, img.height as f64);
    let mut ch: [Vec<f64>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut mx = 0.0f64;
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for j in 0..N {
        for i in 0..N {
            let du = (i as f64 / (N - 1) as f64 * 2.0 - 1.0) * half[0];
            let dv = (j as f64 / (N - 1) as f64 * 2.0 - 1.0) * half[1];
            let p = map(h, u + du, v + dv)?;
            if !(0.0..1.0).contains(&p[0]) || !(0.0..1.0).contains(&p[1]) {
                return None;
            }
            (x0, y0, x1, y1) = (x0.min(p[0]), y0.min(p[1]), x1.max(p[0]), y1.max(p[1]));
            let (px, py) = ((p[0] * w) as usize, (p[1] * ht) as usize);
            let v = img.data.get(py.saturating_mul(img.width).saturating_add(px)).copied()?;
            for (c, list) in ch.iter_mut().enumerate() {
                let x = f64::from(v.get(c).copied().unwrap_or(0.0));
                mx = mx.max(x);
                list.push(x);
            }
        }
    }
    let med = |mut l: Vec<f64>| {
        l.sort_by(f64::total_cmp);
        l.get(l.len() / 2).copied().unwrap_or(0.0)
    };
    let [r, g, b] = ch;
    let rgb = [med(r), med(g), med(b)];
    rgb.iter().all(|v| v.is_finite()).then_some((rgb, mx, [x0, y0, x1, y1]))
}

/// Largest raw file read.
const MAX_FILE: u64 = 2 << 30;

/// Decode and develop a shot (camera RGB, oriented), and read the patches for the best of the eight orientations.
pub fn sample(shot: &Shot, edition: Edition) -> Result<Sampled> {
    let name = crate::dataset::file_name(&shot.path);
    if !shot.corners.iter().flatten().all(|v| v.is_finite() && (0.0..=1.0).contains(v)) {
        return Err(Error(format!("{name}: corners must be fractions of the photo's width and height (0 to 1)")));
    }
    let len = std::fs::metadata(&shot.path).map_err(|e| Error(format!("{name}: {e}")))?.len();
    if len > MAX_FILE {
        return Err(Error(format!("{name}: file too large")));
    }
    let bytes = std::fs::read(&shot.path).map_err(|e| Error(format!("{name}: {e}")))?;
    let raw = lcc_raw::decode(&bytes).map_err(|e| Error(format!("{name}: not a raw this app reads: {e}")))?;
    drop(bytes);
    let img = match raw.develop_binned(2, 1.0) {
        Ok(Some(img)) => img,
        _ => raw.develop(lcc_raw::Method::Bilinear).map_err(|e| Error(format!("{name}: {e}")))?,
    };
    let img = img.into_oriented(raw.orientation);
    if img.width < 64 || img.height < 64 {
        return Err(Error(format!("{name}: too small")));
    }
    let reference = edition.reference_xyz();
    let mut best: Option<(f64, Vec<[f64; 3]>, Vec<f64>, Vec<[f64; 4]>)> = None;
    for corners in orientations(&shot.corners) {
        let Some((_, h)) = centres(&corners) else { continue };
        let mut cams = Vec::with_capacity(24);
        let mut maxes = Vec::with_capacity(24);
        let mut boxes = Vec::with_capacity(24);
        let mut ok = true;
        for row in 0..4 {
            for col in 0..6 {
                match read_patch(&img, &h, col as f64 / 5.0, row as f64 / 3.0, [PATCH_HALF / 5.0, PATCH_HALF / 3.0]) {
                    Some((rgb, mx, bx)) => {
                        cams.push(rgb);
                        maxes.push(mx);
                        boxes.push(bx);
                    }
                    None => ok = false,
                }
            }
        }
        if !ok || cams.len() != 24 {
            continue;
        }
        let cam: [[f64; 3]; 24] = std::array::from_fn(|i| cams.get(i).copied().unwrap_or([0.0; 3]));
        let clipped: Vec<usize> = (0..24).filter(|i| maxes.get(*i).is_some_and(|m| *m >= CLIP)).collect();
        let score = match balance(&cam, &reference) {
            Some(b) => linear_fit(&b, &clipped, &reference).map(|(_, e)| e).unwrap_or(f64::INFINITY),
            None => f64::INFINITY,
        };
        if score.is_finite() && best.as_ref().is_none_or(|b| score < b.0) {
            best = Some((score, cams, maxes, boxes));
        }
    }
    let Some((_, cams, maxes, boxes)) = best else {
        return Err(Error(format!(
            "{name}: could not read the chart at those corners (are they the centres of the four corner patches, inside the photo?)"
        )));
    };
    let camera: [[f64; 3]; 24] = std::array::from_fn(|i| cams.get(i).copied().unwrap_or([0.0; 3]));
    let clipped: Vec<usize> = (0..24).filter(|i| maxes.get(*i).is_some_and(|m| *m >= CLIP)).collect();
    if NEUTRALS.iter().any(|i| clipped.contains(i)) {
        return Err(Error(format!("{name}: the grey patches are clipped: expose the chart darker")));
    }
    let m = &raw.metadata;
    Ok(Sampled {
        make: m.make.clone().unwrap_or_default().trim().to_string(),
        model: m.model.clone().unwrap_or_default().trim().to_string(),
        camera,
        clipped,
        boxes,
        image: img,
        color: raw.color.clone(),
    })
}

/// A shot white-balanced and exposed: per patch, camera RGB divided by the measured neutral and scaled so the grey
/// patches land on their reference luminance; and the neutral (green = 1).
struct Balanced {
    rgb: [[f64; 3]; 24],
    neutral: [f64; 3],
    /// The camera value of a perfect white (luminance 1), green channel.
    exposure: f64,
}

fn balance(cam: &[[f64; 3]; 24], reference: &[[f64; 3]; 24]) -> Option<Balanced> {
    // the white patch is a little yellow (b* ≈ 3) and often near clipping: the four middle greys set the white
    let used: Vec<usize> = NEUTRALS.to_vec();
    let mut sum = [0.0; 3];
    let mut y = 0.0;
    for &i in &used {
        let (c, r) = (cam.get(i)?, reference.get(i)?);
        for k in 0..3 {
            sum[k] += c[k];
        }
        y += r[1];
    }
    if !(y > 0.0 && sum.iter().all(|v| v.is_finite() && *v > 1e-6)) {
        return None;
    }
    let per = sum.map(|s| s / y);
    let rgb = cam.map(|c| [c[0] / per[0], c[1] / per[1], c[2] / per[2]]);
    Some(Balanced { rgb, neutral: [per[0] / per[1], 1.0, per[2] / per[1]], exposure: per[1] })
}

/// The forward matrix from its six free entries: each row's third entry makes the row sum to D50's white.
fn fm_of(x: &[f64]) -> Mat3 {
    let g = |i: usize| x.get(i).copied().unwrap_or(0.0);
    Mat3(std::array::from_fn(|r| {
        let (a, b) = (g(r * 2), g(r * 2 + 1));
        [a, b, WHITE_D50.get(r).copied().unwrap_or(0.0) - a - b]
    }))
}

fn usable(clipped: &[usize]) -> Vec<usize> {
    // black 2 is noise-dominated and adds nothing a matrix can use
    (0..24).filter(|i| *i != 23 && !clipped.contains(i)).collect()
}

/// White-preserving least squares in XYZ (each row of the forward matrix separately, weighted by 1 / Y so dark
/// patches count); returns the six free entries and the mean ΔE76 it reaches.
fn linear_fit(b: &Balanced, clipped: &[usize], reference: &[[f64; 3]; 24]) -> Option<(Vec<f64>, f64)> {
    let idx = usable(clipped);
    if idx.len() < 8 {
        return None;
    }
    let mut x = Vec::with_capacity(6);
    for r in 0..3 {
        let white = WHITE_D50.get(r).copied()?;
        // target_r = a·(c0 − c2) + b·(c1 − c2) + white·c2
        let (mut a11, mut a12, mut a22, mut b1, mut b2) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for &i in &idx {
            let (c, t) = (b.rgb.get(i)?, reference.get(i)?);
            let w = 1.0 / t[1].max(0.02);
            let (u, v) = (c[0] - c[2], c[1] - c[2]);
            let rhs = t.get(r)? - white * c[2];
            a11 += w * u * u;
            a12 += w * u * v;
            a22 += w * v * v;
            b1 += w * u * rhs;
            b2 += w * v * rhs;
        }
        let sol = linalg::solve(vec![vec![a11, a12], vec![a12, a22]], vec![b1, b2])?;
        x.extend(sol);
    }
    let fm = fm_of(&x);
    let err = idx
        .iter()
        .filter_map(|&i| {
            let (p, t) = (lab(fm.apply(*b.rgb.get(i)?)), lab(*reference.get(i)?));
            Some(((p[0] - t[0]).powi(2) + (p[1] - t[1]).powi(2) + (p[2] - t[2]).powi(2)).sqrt())
        })
        .sum::<f64>()
        / idx.len() as f64;
    err.is_finite().then_some((x, err))
}

/// Levenberg–Marquardt on CIE L*a*b* error over every usable patch of `parts`: the forward matrix's six free entries
/// and a red / blue white-balance correction (the grey patches are not perfectly neutral, so the measured balance is
/// refined with the matrix). Returns the forward matrix and the correction (divisors of the balanced red and blue).
fn refine(parts: &[(&Balanced, &[usize])], x0: Vec<f64>, reference: &[[f64; 3]; 24]) -> (Mat3, [f64; 2]) {
    let mut start = x0;
    start.extend([1.0, 1.0]);
    let residuals = |x: &[f64]| -> Vec<f64> {
        let fm = fm_of(x);
        let (cr, cb) = (x.get(6).copied().unwrap_or(1.0), x.get(7).copied().unwrap_or(1.0));
        let mut r = Vec::new();
        for (b, c) in parts {
            for i in usable(c) {
                let (Some(cam), Some(t)) = (b.rgb.get(i), reference.get(i)) else { continue };
                let (p, t) = (lab(fm.apply([cam[0] / cr, cam[1], cam[2] / cb])), lab(*t));
                r.extend([p[0] - t[0], p[1] - t[1], p[2] - t[2]]);
            }
        }
        r
    };
    let clamp = |x: &mut [f64]| {
        for v in x.iter_mut().skip(6) {
            *v = v.clamp(0.7, 1.4);
        }
    };
    let x = linalg::levenberg_marquardt(start, 80, 1e-6, residuals, clamp);
    (fm_of(&x), [x.get(6).copied().unwrap_or(1.0), x.get(7).copied().unwrap_or(1.0)])
}

/// One light's fitted matrices.
#[derive(Clone, Debug, Serialize)]
pub struct LightFit {
    pub light: Light,
    pub shots: usize,
    pub color_matrix: Mat3,
    pub forward_matrix: Mat3,
    /// The measured camera neutral (green = 1).
    pub neutral: [f64; 3],
}

/// Per-patch error of one shot, before (the camera's current colour) and after (the fitted entry), through the DNG
/// colour model at the light's fitted neutral.
#[derive(Clone, Debug, Serialize)]
pub struct ShotReport {
    pub file: String,
    pub light: Light,
    pub clipped: Vec<&'static str>,
    pub before: Vec<f64>,
    pub after: Vec<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChartReport {
    pub edition: Edition,
    pub lights: Vec<LightFit>,
    pub shots: Vec<ShotReport>,
    /// What the camera's colour came from before (the database's provenance, the file's own matrices, or the
    /// neutral fallback).
    pub before_from: String,
}

/// Fit every light's matrices to the shots (each already sampled), joining shots of the same light.
fn fit_lights(shots: &[(&Shot, Sampled)], edition: Edition) -> Result<Vec<LightFit>> {
    let reference = edition.reference_xyz();
    let mut codes: Vec<u16> = shots.iter().map(|(s, _)| s.light.code).collect();
    codes.sort_unstable();
    codes.dedup();
    if codes.len() > 2 {
        return Err(Error("shots under at most two lights (e.g. daylight and tungsten): the DNG model has two calibration illuminants".into()));
    }
    let mut out = Vec::new();
    for code in codes {
        let group: Vec<(&Shot, Balanced, Vec<usize>)> = shots
            .iter()
            .filter(|(s, _)| s.light.code == code)
            .filter_map(|(s, d)| balance(&d.camera, &reference).map(|b| (*s, b, d.clipped.clone())))
            .collect();
        let Some((first, _, _)) = group.first() else { continue };
        let light = first.light;
        // start from the first shot's linear fit, then minimise L*a*b* error over every shot of this light
        let x0 = group
            .iter()
            .find_map(|(_, b, c)| linear_fit(b, c, &reference).map(|(x, _)| x))
            .ok_or_else(|| Error(format!("{}: too few usable patches to fit", crate::dataset::file_name(&first.path))))?;
        let parts: Vec<(&Balanced, &[usize])> = group.iter().map(|(_, b, c)| (b, c.as_slice())).collect();
        let (fm, corr) = refine(&parts, x0, &reference);
        // the light's neutral: the shots' mean, refined with the fit's red and blue balance
        let n = group.iter().fold([0.0; 3], |a, (_, b, _)| [a[0] + b.neutral[0], a[1] + b.neutral[1], a[2] + b.neutral[2]]);
        let k = group.len() as f64;
        let neutral = [n[0] / k * corr[0], 1.0, n[2] / k * corr[1]];
        let fm_inv = fm.inverse().ok_or_else(|| Error("the fitted forward matrix is singular".into()))?;
        let cm = Mat3::diag(neutral[0], neutral[1], neutral[2]).mul(&fm_inv).mul(&lcc_color::bradford(light.white(), D50));
        if !cm.0.iter().chain(fm.0.iter()).flatten().all(|v| v.is_finite() && v.abs() < 50.0) || cm.inverse().is_none() {
            return Err(Error("the fit gave unusable matrices: check the corners and the chart edition".into()));
        }
        out.push(LightFit { light, shots: group.len(), color_matrix: cm, forward_matrix: fm, neutral });
    }
    if out.is_empty() {
        return Err(Error("no shot could be fitted".into()));
    }
    out.sort_by(|a, b| a.light.kelvin.total_cmp(&b.light.kelvin));
    Ok(out)
}

/// ΔE00 of every patch of a shot through `color` (the DNG model) at the white `neutral` reads as, with the
/// exposure matched on the grey patches. `None` for clipped patches.
fn errors_through(color: &lcc_raw::ColorData, d: &Sampled, b: &Balanced, neutral: [f64; 3], reference: &[[f64; 3]; 24]) -> Vec<f64> {
    let m = if lcc_raw::color::has_matrix(color) {
        let xy = lcc_raw::color::neutral_to_xy(color, neutral);
        lcc_raw::color::camera_to_xyz_d50(color, xy)
    } else {
        // the neutral fallback: white-balanced camera RGB read as linear sRGB
        lcc_color::bradford(lcc_color::D65, D50).mul(&lcc_color::SRGB.to_xyz()).mul(&Mat3::diag(1.0 / neutral[0], 1.0, 1.0 / neutral[2]))
    };
    let exp = b.exposure.max(1e-9);
    let pred: Vec<[f64; 3]> = d.camera.iter().map(|c| m.apply([c[0] / exp, c[1] / exp, c[2] / exp])).collect();
    let (mut py, mut ry) = (0.0, 0.0);
    for &i in &NEUTRALS {
        py += pred.get(i).map_or(0.0, |p| p[1]);
        ry += reference.get(i).map_or(0.0, |r| r[1]);
    }
    let k = if py > 1e-9 { ry / py } else { 1.0 };
    (0..24)
        .map(|i| match (pred.get(i), reference.get(i)) {
            (Some(p), Some(t)) if !d.clipped.contains(&i) => de2000(lab(p.map(|v| v * k)), lab(*t)),
            _ => f64::NAN,
        })
        .collect()
}

/// Sample and fit `shots`: the camera database entry, its report and the samples (for previews).
pub fn calibrate(shots: &[Shot], edition: Edition, contributor: Option<&str>) -> Result<(lcc_camdb::Entry, ChartReport, Vec<Sampled>)> {
    if shots.is_empty() {
        return Err(Error("no chart shots".into()));
    }
    if shots.len() > 64 {
        return Err(Error("at most 64 chart shots".into()));
    }
    let sampled: Vec<Sampled> = shots.iter().map(|s| sample(s, edition)).collect::<Result<_>>()?;
    let Some(first) = sampled.first() else { return Err(Error("no chart shots".into())) };
    let (make, model) = (first.make.clone(), first.model.clone());
    if model.is_empty() {
        return Err(Error("the raw names no camera model".into()));
    }
    if sampled.iter().any(|s| !s.model.eq_ignore_ascii_case(&model) || !s.make.eq_ignore_ascii_case(&make)) {
        return Err(Error("the shots come from more than one camera model: calibrate one at a time".into()));
    }
    let pairs: Vec<(&Shot, Sampled)> = shots.iter().zip(sampled.iter().cloned()).collect();
    let lights = fit_lights(&pairs, edition)?;
    let (lo, hi) = (lights.first(), lights.get(1));
    let lo = lo.ok_or_else(|| Error("no light fitted".into()))?;
    let entry = lcc_camdb::Entry {
        make: make.clone(),
        model: model.clone(),
        aliases: Vec::new(),
        illuminants: [lo.light.code, hi.map_or(0, |h| h.light.code)],
        color_matrix: [Some(lo.color_matrix), hi.map(|h| h.color_matrix)],
        forward_matrix: [Some(lo.forward_matrix), hi.map(|h| h.forward_matrix)],
        wb_presets: Vec::new(),
        black: None,
        white: None,
        provenance: lcc_camdb::Provenance {
            source: lcc_camdb::Source::Chart,
            detail: format!(
                "{}, {} shot(s): {}",
                edition.name(),
                shots.len(),
                lights.iter().map(|l| format!("{:.0} K", l.light.kelvin)).collect::<Vec<_>>().join(" and ")
            ),
            contributor: contributor.map(|c| c.trim().to_string()).filter(|c| !c.is_empty()),
        },
    };
    entry.validate().map_err(|e| Error(format!("the fitted entry is invalid: {}", e.0)))?;

    // before: the camera's colour as the app had it (database, else the file's own matrices, else the fallback)
    let reference = edition.reference_xyz();
    let prior = lcc_camdb::find(Some(&make), &model);
    let before_from = match &prior {
        Some(e) => format!("camera database ({:?})", e.provenance.source),
        None if lcc_raw::color::has_matrix(&first.color) => "the file's own matrices".into(),
        None => "the neutral fallback".into(),
    };
    let mut reports = Vec::new();
    for (shot, d) in shots.iter().zip(&sampled) {
        let Some(b) = balance(&d.camera, &reference) else { continue };
        let before_color = match &prior {
            Some(e) => e.color_data(),
            None => d.color.clone(),
        };
        // both read at the light's fitted neutral (what a camera's own white balance of the light gives)
        let neutral = lights.iter().find(|l| l.light.code == shot.light.code).map_or(b.neutral, |l| l.neutral);
        reports.push(ShotReport {
            file: crate::dataset::file_name(&shot.path),
            light: shot.light,
            clipped: d.clipped.iter().filter_map(|i| NAMES.get(*i).copied()).collect(),
            before: errors_through(&before_color, d, &b, neutral, &reference),
            after: errors_through(&entry.color_data(), d, &b, neutral, &reference),
        });
    }
    Ok((entry, ChartReport { edition, lights, shots: reports, before_from }, sampled))
}

/// A preview of a sampled shot: the camera RGB roughly balanced and tone-mapped, with each patch's sampling box
/// outlined, as 8-bit RGB `(width, height, pixels)`.
pub fn preview(d: &Sampled) -> (usize, usize, Vec<u8>) {
    let (w, h) = (d.image.width, d.image.height);
    let reference = Edition::After2014.reference_xyz();
    let (n, exp) = balance(&d.camera, &reference).map_or(([1.0; 3], 1.0), |b| (b.neutral, b.exposure));
    let mut px: Vec<u8> = d
        .image
        .data
        .iter()
        .flat_map(|p| {
            let v = [f64::from(p[0]) / n[0], f64::from(p[1]) / n[1], f64::from(p[2]) / n[2]];
            v.map(|c| (lcc_color::transfer::linear_to_srgb(((c / exp.max(1e-9)) * 0.9).clamp(0.0, 1.0) as f32) * 255.0).round() as u8)
        })
        .collect();
    for (i, b) in d.boxes.iter().enumerate() {
        let colour: [u8; 3] = if d.clipped.contains(&i) { [255, 40, 40] } else { [255, 255, 255] };
        let (x0, y0) = ((b[0] * w as f64) as usize, (b[1] * h as f64) as usize);
        let (x1, y1) = (((b[2] * w as f64) as usize).min(w.saturating_sub(1)), ((b[3] * h as f64) as usize).min(h.saturating_sub(1)));
        let mut put = |x: usize, y: usize| {
            let at = y.saturating_mul(w).saturating_add(x).saturating_mul(3);
            if let Some(s) = px.get_mut(at..at.saturating_add(3)) {
                s.copy_from_slice(&colour);
            }
        };
        for x in x0..=x1 {
            put(x, y0);
            put(x, y1);
        }
        for y in y0..=y1 {
            put(x0, y);
            put(x1, y);
        }
    }
    (w, h, px)
}

/// The report as text.
pub fn text(r: &ChartReport) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let mean = |v: &[f64]| {
        let ok: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
        if ok.is_empty() { f64::NAN } else { ok.iter().sum::<f64>() / ok.len() as f64 }
    };
    let max = |v: &[f64]| v.iter().copied().filter(|x| x.is_finite()).fold(0.0, f64::max);
    for l in &r.lights {
        let _ = writeln!(
            s,
            "light {:.0} K (DNG illuminant {}): {} shot(s), neutral {:.3} / 1 / {:.3}",
            l.light.kelvin, l.light.code, l.shots, l.neutral[0], l.neutral[2]
        );
    }
    for sh in &r.shots {
        let _ = writeln!(
            s,
            "\n{} ({:.0} K): ΔE00 mean {:.2} → {:.2}, max {:.2} → {:.2} (before: {})",
            sh.file,
            sh.light.kelvin,
            mean(&sh.before),
            mean(&sh.after),
            max(&sh.before),
            max(&sh.after),
            r.before_from
        );
        if !sh.clipped.is_empty() {
            let _ = writeln!(s, "  clipped, left out: {}", sh.clipped.join(", "));
        }
        for (i, name) in NAMES.iter().enumerate() {
            let (b, a) = (sh.before.get(i).copied().unwrap_or(f64::NAN), sh.after.get(i).copied().unwrap_or(f64::NAN));
            if a.is_finite() {
                let _ = writeln!(s, "  {name:<14} {b:>6.2} → {a:>5.2}");
            }
        }
    }
    s
}

/// Parse `x,y x,y x,y x,y` (or `;`-separated pairs) into four corners.
pub fn parse_corners(s: &str) -> Option<[[f64; 2]; 4]> {
    let nums: Vec<f64> =
        s.split(|c: char| c == ',' || c == ';' || c.is_whitespace()).filter(|t| !t.is_empty()).map(|t| t.parse().ok()).collect::<Option<_>>()?;
    if nums.len() != 8 || !nums.iter().all(|v| v.is_finite()) {
        return None;
    }
    let g = |i: usize| nums.get(i).copied().unwrap_or(f64::NAN);
    Some([[g(0), g(1)], [g(2), g(3)], [g(4), g(5)], [g(6), g(7)]])
}

/// Where a shot's preview goes inside `dir`.
pub fn preview_path(dir: &Path, shot: &Shot) -> PathBuf {
    let stem = shot.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "shot".into());
    dir.join(format!("{stem}.chart.png"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_white_is_near_white_and_greys_are_neutral() {
        for e in [Edition::After2014, Edition::Before2014] {
            let x = e.reference_xyz();
            assert!((x[18][1] - 0.88).abs() < 0.04, "{:?}", x[18]);
            for i in 18..24 {
                let l = lab(x[i]);
                assert!(l[1].abs() < 3.5 && l[2].abs() < 3.5, "{i}: {l:?}");
            }
        }
    }

    #[test]
    fn lights_parse_and_file_under_the_nearest_standard() {
        assert_eq!(Light::parse("D65").unwrap().code, 21);
        assert_eq!(Light::parse("tungsten").unwrap().code, 17);
        assert_eq!(Light::parse("3200K").unwrap().code, 17);
        assert_eq!(Light::parse("5000").unwrap().code, 23);
        assert_eq!(Light::parse("daylight").unwrap().code, 20);
        for bad in ["", "purple", "100", "NaN", "1e9"] {
            assert!(Light::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn corners_parse_and_orientations_cover_every_way_round() {
        let c = parse_corners("0.1,0.2 0.9,0.2; 0.9,0.8 0.1,0.8").unwrap();
        let o = orientations(&c);
        assert_eq!(o.len(), 8);
        assert!(o.contains(&c), "the order given is one of them");
        for bad in ["", "1,2,3", "a,b c,d e,f g,h", "0.1,0.2 0.9,0.2 0.9,0.8 0.1,NaN"] {
            assert!(parse_corners(bad).is_none(), "{bad}");
        }
        let (centres, _) = centres(&c).unwrap();
        assert_eq!(centres.len(), 24);
        assert!((centres[0][0] - 0.1).abs() < 1e-9 && (centres[23][1] - 0.8).abs() < 1e-9);
    }

    /// The fit recovers a known camera: patches rendered through a known forward matrix and neutral come back.
    #[test]
    fn the_fit_recovers_a_known_matrix() {
        let reference = Edition::After2014.reference_xyz();
        let fm_true = fm_of(&[0.62, 0.25, 0.30, 0.78, 0.06, -0.12]);
        let inv = fm_true.inverse().unwrap();
        let neutral = [0.55, 1.0, 0.72];
        let cam: [[f64; 3]; 24] = reference.map(|t| {
            let wb = inv.apply(t);
            [wb[0] * neutral[0] * 0.4, wb[1] * 0.4, wb[2] * neutral[2] * 0.4]
        });
        let b = balance(&cam, &reference).unwrap();
        assert!((b.neutral[0] - 0.55).abs() < 0.02 && (b.neutral[2] - 0.72).abs() < 0.02, "{:?}", b.neutral);
        let (x, err) = linear_fit(&b, &[], &reference).unwrap();
        assert!(err < 3.0, "{err}");
        let (fm, corr) = refine(&[(&b, &[][..])], x, &reference);
        let d: Vec<f64> = (0..23)
            .map(|i| {
                let c = b.rgb[i];
                de2000(lab(fm.apply([c[0] / corr[0], c[1], c[2] / corr[1]])), lab(reference[i]))
            })
            .collect();
        assert!(d.iter().all(|e| *e < 0.3), "{d:?}");
        for (r, t) in fm.0.iter().flatten().zip(fm_true.0.iter().flatten()) {
            assert!((r - t).abs() < 0.02, "{fm:?} vs {fm_true:?}");
        }
        assert!(linear_fit(&b, &(0..20).collect::<Vec<_>>(), &reference).is_none(), "too few patches");
    }
}
