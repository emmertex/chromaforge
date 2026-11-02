//! Eye focus for culling: each person's sharper eye, cut out at 100 % (one pixel of the photo per
//! pixel), with a measure of how sharp it is — what Capture One's Cull view offers as Face Focus.
//!
//! The picture is the camera's full-size embedded JPEG of a raw (what culling shows anyway, and a
//! fraction of a raw decode), or the file itself for other images. Faces are found with MediaPipe
//! BlazeFace on a downscaled copy and their eye contours with FaceMesh on a full-resolution crop
//! around each face (the Face Landmarker model, Apache-2.0, downloaded on demand: `ModelId::Face`).
//! Without the model, or in a build without AI masks, [`EyeScanner`] answers with an error the UI
//! turns into "Get the AI Model…".
//!
//! **Focus measure** ([`focus_score`]): the high-frequency energy of the eye region (the RMS of a
//! Laplacian over the eye contour's box, lashes and lid edges included), less the photo's noise at
//! that scale (the median absolute Laplacian of a patch of cheek below the eye, which has no detail
//! of its own), divided by the eye's own contrast (the standard deviation of its luminance), so a
//! dark eye in shade and a bright one in sun score alike. Mapped to 0–100 by [`sharpness`].
//!
//! Nothing here blocks the UI: [`EyeScanner`] runs on its own thread, newest wish first, and the
//! caller keeps the results (they are small: a few crops of 160 × 100 pixels).

use std::sync::Arc;

use lcc_catalog::PhotoId;
use lcc_raster::Rgba8;

/// What changes the results (bump when the measure or the crops change, so caches drop them).
pub const EYES_VERSION: u64 = 3;
/// At most this many people get an eye thumbnail (the largest faces).
pub const MAX_PEOPLE: usize = 120;
/// The eye crop's size in photo pixels (shown at 100 %).
pub const CROP_W: usize = 160;
pub const CROP_H: usize = 100;
/// An eye whose contour is less than this share of the other's width is turned away (its
/// lashes and lid edge seen side on): the other one is the eye to judge.
const HIDDEN_EYE: f32 = 0.5;
/// Long edge of the copy faces are looked for in.
#[cfg_attr(not(feature = "sam"), allow(dead_code))]
const DETECT_EDGE: usize = 1600;
/// How sharp an eye looks, for a dot in the strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Focus {
    Sharp,
    Soft,
    Blurry,
}

/// [`sharpness`] at or above this is [`Focus::Sharp`]; below [`SOFT_FROM`], [`Focus::Blurry`].
/// Set by eye on 280 eyes 30–260 pixels wide from 112 frames of a wedding's EOS R5 C camera JPEGs
/// (ceremony, portraits, groups; 35–85 mm, f/1.4–f/4), measured at a scale that follows the eye's
/// size ([`MEASURE_EYE_W`]): crisp lashes and iris rims read 28–56 (an 85 mm f/1.4 portrait wide
/// open 23–29), soft ones 16–28, missed focus and motion 2–15. Within a photo, eyes close to its
/// sharpest are eased up ([`focus_in_frame`]).
pub const SHARP_FROM: f32 = 28.0;
pub const SOFT_FROM: f32 = 16.0;

impl Focus {
    /// Better is higher.
    fn rank(self) -> u8 {
        match self {
            Focus::Sharp => 2,
            Focus::Soft => 1,
            Focus::Blurry => 0,
        }
    }

    pub fn of(sharpness: f32) -> Focus {
        if !sharpness.is_finite() || sharpness < SOFT_FROM {
            Focus::Blurry
        } else if sharpness < SHARP_FROM {
            Focus::Soft
        } else {
            Focus::Sharp
        }
    }
}

/// Whether an eye is open, from [`openness`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Lids {
    Open,
    /// Half closed: a blink caught midway, heavy lids (a laughing squint usually stays open).
    Half,
    /// A blink.
    Closed,
    /// FaceMesh gave no lids to measure.
    Unknown,
    /// No eye to be seen: a dark lens (sunglasses) over it.
    Hidden,
}

/// [`openness`] below this is [`Lids::Closed`]; below [`HALF_BELOW`], [`Lids::Half`]. Set on
/// 400 eyes of a wedding's frames: blinks and lowered lids read 0.07–0.29 (FaceMesh draws closed
/// lids slightly apart). Half closed was first set at 0.36 and missed every half-open eye the
/// photographer flagged: on a second wedding's tables, groups and dance floor, half-closed and
/// heavy lids read 0.33–0.47, open eyes (squints with the iris well in view included) 0.45 and up,
/// most above 0.5: at 0.48, 10 of 11 half-closed eyes are flagged and 3 of about 40 open ones.
pub const CLOSED_BELOW: f32 = 0.30;
pub const HALF_BELOW: f32 = 0.48;
/// A half-closed gap below this with no iris showing is a closed eye (FaceMesh draws closed lids
/// 0.3–0.36 apart).
pub const CLOSED_GAP: f32 = 0.36;

impl Lids {
    pub fn of(openness: Option<f32>) -> Lids {
        match openness {
            Some(o) if o.is_finite() && o < CLOSED_BELOW => Lids::Closed,
            Some(o) if o.is_finite() && o < HALF_BELOW => Lids::Half,
            Some(o) if o.is_finite() => Lids::Open,
            _ => Lids::Unknown,
        }
    }

    /// Better first: open, unknown, half, closed, hidden (a closed eye beside a hidden one is
    /// the one to show).
    fn rank(self) -> u8 {
        match self {
            Lids::Open => 4,
            Lids::Unknown => 3,
            Lids::Half => 2,
            Lids::Closed => 1,
            Lids::Hidden => 0,
        }
    }
}

/// FaceMesh's lid points per eye (the image's left eye first, as `lcc_segment`'s `EYES`):
/// the corners, then two upper-lid points above two lower-lid points (the six-point eye aspect
/// ratio's points).
pub const LIDS: [([usize; 2], [usize; 2], [usize; 2]); 2] = [([33, 133], [160, 158], [144, 153]), ([263, 362], [387, 385], [373, 380])];
/// FaceMesh's iris points per eye: centre, then four on the rim.
pub const IRIS_POINTS: [(usize, [usize; 4]); 2] = [(468, [469, 470, 471, 472]), (473, [474, 475, 476, 477])];

/// How far eye `k` is open in FaceMesh landmarks `lm` (photo pixels): the mean gap between the
/// upper and lower lid over the iris's diameter (about 0.8-1.1 wide open, under 0.2 in a blink).
/// The iris is the yardstick because FaceMesh places it at its full size whether the lids hide it
/// or not, and it doesn't shrink when the head turns the way the eye's width does; without a
/// plausible iris, the gap over a third of the eye's width (an eye is about three irises wide).
pub fn openness(lm: &[[f32; 2]], k: usize) -> Option<f32> {
    let d = |a: usize, b: usize| -> Option<f32> {
        let (p, q) = (lm.get(a)?, lm.get(b)?);
        let v = ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt();
        v.is_finite().then_some(v)
    };
    let (corners, upper, lower) = LIDS.get(k)?;
    let gap = (d(upper[0], lower[0])? + d(upper[1], lower[1])?) / 2.0;
    let width = d(corners[0], corners[1])?;
    let iris = IRIS_POINTS.get(k).and_then(|(c, rim)| {
        let r = rim.iter().map(|&p| d(*c, p)).collect::<Option<Vec<f32>>>()?;
        Some(r.iter().sum::<f32>() / 2.0)
    });
    let yardstick = iris.filter(|v| *v > 0.5 && *v < width * 0.8).unwrap_or(width / 3.0);
    (yardstick > 0.0).then(|| gap / yardstick).filter(|v| v.is_finite())
}

/// How much the iris shows in eye `k`: the luminance just above the iris's centre (its upper
/// half, where an open eye shows the iris and a closed one its lid) over the luminance of the skin
/// below the eye (two iris diameters under it). Well under 1 when the iris shows (it is darker than
/// skin, light irises too), near 1 or above when the lid covers it. `None` without landmarks or
/// pixels to compare.
pub fn iris_showing(img: &Rgba8, lm: &[[f32; 2]], k: usize) -> Option<f32> {
    let (c, rim) = IRIS_POINTS.get(k)?;
    let c = *lm.get(*c)?;
    let r = rim.iter().filter_map(|p| lm.get(*p)).map(|p| ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2)).sqrt()).sum::<f32>() / 4.0;
    if !(r.is_finite() && r >= 2.0 && c.iter().all(|v| v.is_finite())) {
        return None;
    }
    let mean = |reg: Region| {
        let mut sum = 0.0f32;
        let mut n = 0.0f32;
        for y in reg.y..reg.y + reg.h {
            for x in reg.x..reg.x + reg.w {
                if let Some(p) = img.data.get(y * img.width + x) {
                    sum += luma(*p);
                    n += 1.0;
                }
            }
        }
        (n > 0.0).then(|| sum / n)
    };
    let iris = mean(Region::around(c[0], c[1] - r * 0.35, r * 0.9, r * 0.5, img.width, img.height)?)?;
    let skin = mean(Region::around(c[0], c[1] + r * 4.0, r * 2.0, r, img.width, img.height)?)?;
    (skin > 0.02).then(|| iris / skin).filter(|v| v.is_finite())
}

/// How much darker the iris is than the whites beside it (the brighter of the two points halfway
/// from the iris's centre to each corner), over the cheek's luminance: an eye has a dark iris
/// between lighter whites, a sunglass lens is one tone across. `None` without landmarks.
pub fn iris_contrast(img: &Rgba8, lm: &[[f32; 2]], k: usize) -> Option<f32> {
    let (corners, _, _) = LIDS.get(k)?;
    let (a, b) = (*lm.get(corners[0])?, *lm.get(corners[1])?);
    let c = *lm.get(IRIS_POINTS.get(k)?.0)?;
    let ew = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt();
    if !(ew.is_finite() && ew >= 4.0 && c.iter().all(|v| v.is_finite())) {
        return None;
    }
    let rad = (ew * 0.06).max(0.8);
    let disk = |p: [f32; 2]| -> Option<f32> {
        let mut v = Vec::new();
        for dy in [-rad, 0.0, rad] {
            for dx in [-rad, 0.0, rad] {
                v.push(luma_at(img, p[0] + dx, p[1] + dy)?);
            }
        }
        Some(v.iter().sum::<f32>() / v.len() as f32)
    };
    let iris = disk(c)?;
    let side = |q: [f32; 2]| disk([(c[0] + q[0]) / 2.0, (c[1] + q[1]) / 2.0]);
    let sides = side(a).into_iter().chain(side(b)).fold(f32::NEG_INFINITY, f32::max);
    let skin = luma_at(img, c[0], c[1] + ew * 0.75)?.max(0.02);
    let v = (sides - iris) / skin;
    v.is_finite().then_some(v)
}

/// The lids' verdict from their gap ([`openness`]) and how much iris shows ([`iris_showing`]):
/// FaceMesh tends to draw a closed eye a little open (gaps of 0.3–0.36 on closed lids are common),
/// so a narrow gap counts as closed when the iris doesn't show. A half-closed eye shows some iris
/// under the lowered lid, so the iris showing doesn't make a narrow gap open.
pub fn lids_from(openness: Option<f32>, iris: Option<f32>) -> Lids {
    match (Lids::of(openness), iris.filter(|v| v.is_finite())) {
        (Lids::Half, Some(i)) if i >= IRIS_HIDDEN && openness.is_some_and(|o| o < CLOSED_GAP) => Lids::Closed,
        (Lids::Open, Some(i)) if i >= IRIS_HIDDEN && openness.is_some_and(|o| o < WIDE_OPEN) => Lids::Half,
        (l, _) => l,
    }
}

/// [`iris_showing`] at or above this: no iris to be seen (lid skin where the iris would be).
pub const IRIS_HIDDEN: f32 = 0.85;
/// Two eyes whose mean [`openness`] is under this are a half-closed person (FACES-DETECT: one eye
/// alone may read just open while the pair is clearly heavy-lidded).
pub const PAIR_HALF_BELOW: f32 = 0.50;

/// Whether two measured eyes, together, are half closed ([`PAIR_HALF_BELOW`]).
pub fn both_half_closed(a: f32, b: f32) -> bool {
    a.is_finite() && b.is_finite() && (a + b) / 2.0 < PAIR_HALF_BELOW
}

/// An eye this open ([`openness`]) is open whatever the iris check says (a pale iris in shade).
pub const WIDE_OPEN: f32 = 0.45;

/// Of two measured eyes, the one to show: the more open (open, unknown, half, closed), then the
/// sharper. Both closed: the sharper, still marked closed.
pub fn pick_eye<T>(a: Option<(f32, T)>, b: Option<(f32, T)>, lids: impl Fn(&(f32, T)) -> Lids) -> Option<(f32, T)> {
    match (a, b) {
        (Some(a), Some(b)) => {
            let (ra, rb) = (lids(&a).rank(), lids(&b).rank());
            Some(if rb > ra || (rb == ra && b.0 > a.0) { b } else { a })
        }
        (a, b) => a.or(b),
    }
}

/// One person's sharper eye.
#[derive(Clone, Debug)]
pub struct EyeShot {
    /// The face's box, normalised to the picture (x0, y0, x1, y1).
    pub face: [f32; 4],
    /// The eye's centre, normalised to the picture.
    pub center: [f32; 2],
    /// The crop at 100 % ([`CROP_W`] × [`CROP_H`], smaller at the picture's edges).
    pub crop: Arc<Rgba8>,
    /// 0–100 (see [`sharpness`]).
    pub sharpness: f32,
    /// The sharpness as a share of the photo's sharpest open eye (0–100; that eye is 100).
    pub relative: f32,
    pub focus: Focus,
    /// Whether the other eye was measured too (a face turned away shows one).
    pub both_eyes: bool,
    /// How far the eye is open: the lid gap over the iris's diameter (see [`openness`]).
    pub openness: Option<f32>,
    /// How much of the iris shows (see [`iris_showing`]; lower shows more).
    pub iris: Option<f32>,
    /// How much darker the iris is than the whites beside it ([`iris_contrast`]).
    pub contrast: Option<f32>,
    /// The eye's look on the photo (see [`eye_look`]).
    pub look: Option<EyeLook>,
    /// The eye's width in photo pixels (corner to corner as placed).
    pub eye_width: f32,
    /// How sure the face detector was (0–1).
    pub score: f32,
    /// One of the photo's main subjects: a face at least half as wide as its largest (not a guest
    /// further back). Automatic rejects look at these only.
    pub main: bool,
    /// Whether FaceMesh placed the eye (else BlazeFace's eye point, refined: a profile).
    pub landmarks: bool,
    pub lids: Lids,
}

impl EyeShot {
    /// What the culler wants: sharp and open.
    pub fn good(&self) -> bool {
        self.focus == Focus::Sharp && self.lids == Lids::Open
    }

    /// The strip's verdict for this person: the worse of the lids and the focus (closed is
    /// closed however sharp, a hidden eye has no verdict).
    pub fn verdict(&self) -> Verdict {
        match (self.lids, self.focus) {
            (Lids::Hidden, _) => Verdict::NotVisible,
            (Lids::Closed, _) => Verdict::Closed,
            (_, Focus::Blurry) => Verdict::Blurry,
            (Lids::Half, _) => Verdict::HalfClosed,
            (_, Focus::Soft) => Verdict::Soft,
            _ => Verdict::Sharp,
        }
    }
}

/// What the strip says about a person (see [`EyeShot::verdict`]): green only for open and
/// sharp, red for closed or blurry, amber for soft or half closed, grey when no eye shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Verdict {
    Sharp,
    Soft,
    HalfClosed,
    Blurry,
    Closed,
    /// Sunglasses: no eye to judge.
    NotVisible,
}

impl Verdict {
    /// For an automatic reject: closed or blurry.
    pub fn is_reject(self) -> bool {
        matches!(self, Verdict::Closed | Verdict::Blurry)
    }
}

/// The eyes of one photo, left to right as the people stand.
#[derive(Clone, Debug)]
pub struct EyeReport {
    pub photo: PhotoId,
    pub eyes: Vec<EyeShot>,
    /// Faces found (before the [`MAX_PEOPLE`] cap).
    pub faces: usize,
    /// Of the people shown, those whose eyes couldn't be placed (a strong profile, a face buried
    /// in a shoulder): BlazeFace finds them, FaceMesh doesn't take them, and BlazeFace's own eye
    /// points on such faces land on ears and fingers as often as on eyes, so they get no crop.
    pub no_eyes: usize,
    /// Every face found (normalised boxes, left to right), shown or not.
    pub boxes: Vec<[f32; 4]>,
    /// The picture's size (pixels).
    pub size: (usize, usize),
    pub ms: f64,
}

impl EyeReport {
    /// A report of nothing found.
    pub fn empty() -> EyeReport {
        EyeReport { photo: PhotoId(0), eyes: Vec::new(), faces: 0, no_eyes: 0, boxes: Vec::new(), size: (0, 0), ms: 0.0 }
    }

    /// Where Zoom to Focus Point goes when the camera recorded no point (normalized): the
    /// sharpest open eye of the photo's main subjects, else the largest face's centre.
    pub fn zoom_target(&self) -> Option<[f32; 2]> {
        let open = |e: &&EyeShot| matches!(e.lids, Lids::Open | Lids::Unknown | Lids::Half);
        if let Some(e) = self.eyes.iter().filter(|e| e.main).filter(open).max_by(|a, b| a.sharpness.total_cmp(&b.sharpness)) {
            return Some(e.center);
        }
        let area = |b: &[f32; 4]| (b[2] - b[0]) * (b[3] - b[1]);
        self.boxes
            .iter()
            .filter(|b| b.iter().all(|v| v.is_finite()))
            .max_by(|a, b| area(a).total_cmp(&area(b)))
            .map(|b| [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0])
    }

    /// People whose shown eye is closed or half closed (a cull filter could use it later).
    pub fn blinks(&self) -> usize {
        self.eyes.iter().filter(|e| matches!(e.lids, Lids::Closed | Lids::Half)).count()
    }
}

/// Where a photo's picture comes from.
#[derive(Clone)]
pub enum EyeSource {
    /// A raw's embedded camera JPEG, through the preview loader at full size.
    Embedded(String, crate::media::PreviewLoader),
    /// An image file, decoded whole.
    File(String),
}

/// A photo to look at.
#[derive(Clone)]
pub struct EyeJob {
    pub key: u64,
    pub photo: PhotoId,
    pub source: EyeSource,
    /// Where its result is kept on disk ([`crate::eye_store`]): read before the photo is looked
    /// at, written after. `None`: kept in memory only.
    pub store: Option<crate::eye_store::Store>,
}

/// Luminance (0–1, display-referred) of an sRGB pixel.
fn luma(p: [u8; 4]) -> f32 {
    (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) / 255.0
}

/// A rectangle of pixels, inside the picture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

impl Region {
    /// `(cx, cy)` ± half of `w × h`, clipped to an `iw × ih` picture; `None` when nothing is left.
    pub fn around(cx: f32, cy: f32, w: f32, h: f32, iw: usize, ih: usize) -> Option<Region> {
        if !(cx.is_finite() && cy.is_finite() && w.is_finite() && h.is_finite()) || w < 1.0 || h < 1.0 {
            return None;
        }
        let x0 = (cx - w / 2.0).round().clamp(0.0, iw as f32) as usize;
        let y0 = (cy - h / 2.0).round().clamp(0.0, ih as f32) as usize;
        let x1 = (cx + w / 2.0).round().clamp(0.0, iw as f32) as usize;
        let y1 = (cy + h / 2.0).round().clamp(0.0, ih as f32) as usize;
        (x1 > x0 + 2 && y1 > y0 + 2).then_some(Region { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
    }
}

/// The luminance inside `r`, box-averaged over `k × k` pixels (`k` at least 2): (values, width,
/// height). A defocused eye has lost its detail at this scale too, while the photo's grain (mostly
/// finer) weighs less. [`measure_eye`] scales `k` with the eye's size, so the measure looks at the
/// same share of an eye whether it is 40 or 400 pixels wide.
fn luma_scaled(img: &Rgba8, r: Region, k: f32) -> (Vec<f32>, usize, usize) {
    let k = if k.is_finite() { k.max(2.0) } else { 2.0 };
    let (gw, gh) = ((r.w as f32 / k) as usize, (r.h as f32 / k) as usize);
    let mut v = Vec::with_capacity(gw * gh);
    for gy in 0..gh {
        let (y0, y1) = (r.y + (gy as f32 * k) as usize, r.y + ((gy + 1) as f32 * k) as usize);
        for gx in 0..gw {
            let (x0, x1) = (r.x + (gx as f32 * k) as usize, r.x + ((gx + 1) as f32 * k) as usize);
            let (mut sum, mut n) = (0.0f32, 0u32);
            for y in y0..y1.max(y0 + 1) {
                for p in img.data.get(y * img.width + x0..y * img.width + x1.max(x0 + 1)).unwrap_or(&[]) {
                    sum += luma(*p);
                    n += 1;
                }
            }
            v.push(if n > 0 { sum / n as f32 } else { 0.0 });
        }
    }
    (v, gw, gh)
}

/// The 4-neighbour Laplacian of a `w × h` grid (its border left out).
fn laplacian(g: &[f32], w: usize, h: usize) -> Vec<f32> {
    let at = |x: usize, y: usize| g.get(y * w + x).copied().unwrap_or(0.0);
    let mut out = Vec::with_capacity(w.saturating_sub(2) * h.saturating_sub(2));
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            out.push(4.0 * at(x, y) - at(x - 1, y) - at(x + 1, y) - at(x, y - 1) - at(x, y + 1));
        }
    }
    out
}

/// The noise at the measure's scale in `r` (a region without detail of its own): the median
/// absolute Laplacian, scaled to a standard deviation.
pub fn noise_level(img: &Rgba8, r: Region, k: f32) -> f32 {
    let (g, w, h) = luma_scaled(img, r, k);
    let mut v: Vec<f32> = laplacian(&g, w, h).into_iter().map(f32::abs).filter(|v| v.is_finite()).collect();
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f32::total_cmp);
    v.get(v.len() / 2).copied().unwrap_or(0.0) * 1.4826
}

/// How sharp the detail in `r` is: RMS Laplacian (of the 2 × 2-averaged luminance) with
/// `noise`'s energy taken out, over the region's contrast (its luminance's standard deviation).
/// Unitless; `0.0` for an empty or flat region.
pub fn focus_score(img: &Rgba8, r: Region, noise: f32, k: f32) -> f32 {
    let (g, w, h) = luma_scaled(img, r, k);
    let lap = laplacian(&g, w, h);
    if lap.is_empty() || g.len() < 2 {
        return 0.0;
    }
    let rms = (lap.iter().map(|v| v * v).sum::<f32>() / lap.len() as f32).sqrt();
    let n = g.len() as f32;
    let mean = g.iter().sum::<f32>() / n;
    let std = (g.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / n).max(0.0).sqrt();
    let noise = if noise.is_finite() { noise.max(0.0) } else { 0.0 };
    // the noise's own energy taken out (in quadrature: the two add as variances)
    let s = (rms * rms - noise * noise).max(0.0).sqrt() / (std + 0.02);
    if s.is_finite() { s } else { 0.0 }
}

/// [`focus_score`] on 0–100: 50 at a score of [`HALF_SCORE`] (a crisp eye scores well above it,
/// a missed one well below).
pub fn sharpness(score: f32) -> f32 {
    if !score.is_finite() || score <= 0.0 {
        return 0.0;
    }
    100.0 * (1.0 - (-score * std::f32::consts::LN_2 / HALF_SCORE).exp())
}

/// The [`focus_score`] that reads 50 on [`sharpness`].
pub const HALF_SCORE: f32 = 0.6;

/// The eyes worth judging of two measured ones `(score, (centre, width))`: both, unless one is
/// much narrower than the other ([`HIDDEN_EYE`]: turned away from the camera).
pub fn visible_eyes<C>(a: Option<(f32, (C, f32))>, b: Option<(f32, (C, f32))>) -> (Option<(f32, (C, f32))>, Option<(f32, (C, f32))>) {
    match (a, b) {
        (Some(a), Some(b)) if a.1.1 < b.1.1 * HIDDEN_EYE => (None, Some(b)),
        (Some(a), Some(b)) if b.1.1 < a.1.1 * HIDDEN_EYE => (Some(a), None),
        other => other,
    }
}

/// Of two measured eyes (score, anything), the sharper.
pub fn sharper<T>(a: Option<(f32, T)>, b: Option<(f32, T)>) -> Option<(f32, T)> {
    match (a, b) {
        (Some(a), Some(b)) => Some(if b.0 > a.0 { b } else { a }),
        (a, b) => a.or(b),
    }
}

/// The faces to show: the [`MAX_PEOPLE`] largest (by box area), then left to right.
pub fn people_order(boxes: &[[f32; 4]]) -> Vec<usize> {
    let area = |b: &[f32; 4]| ((b[2] - b[0]) * (b[3] - b[1])).max(0.0);
    let mut idx: Vec<usize> = (0..boxes.len()).filter(|&i| boxes.get(i).is_some_and(|b| b.iter().all(|v| v.is_finite()))).collect();
    idx.sort_by(|&a, &b| boxes.get(b).map(area).unwrap_or(0.0).total_cmp(&boxes.get(a).map(area).unwrap_or(0.0)));
    idx.truncate(MAX_PEOPLE);
    idx.sort_by(|&a, &b| {
        let cx = |i: usize| boxes.get(i).map(|b| (b[0] + b[2]) / 2.0).unwrap_or(0.0);
        cx(a).total_cmp(&cx(b))
    });
    idx
}

/// The 100 % crop centred on `(cx, cy)` ([`CROP_W`] × [`CROP_H`], kept inside the picture).
pub fn crop_at(img: &Rgba8, cx: f32, cy: f32) -> Option<Rgba8> {
    let w = CROP_W.min(img.width);
    let h = CROP_H.min(img.height);
    if w == 0 || h == 0 || !cx.is_finite() || !cy.is_finite() {
        return None;
    }
    let x0 = (cx - w as f32 / 2.0).round().clamp(0.0, (img.width - w) as f32) as usize;
    let y0 = (cy - h as f32 / 2.0).round().clamp(0.0, (img.height - h) as f32) as usize;
    let mut out = Rgba8::new(w, h);
    for (y, row) in out.data.chunks_mut(w).enumerate() {
        let start = (y0 + y) * img.width + x0;
        if let Some(src) = img.data.get(start..start + w) {
            row.copy_from_slice(src);
        }
    }
    Some(out)
}

/// An eye this many pixels wide is measured at [`luma_scaled`]'s finest scale (2 × 2 pixels);
/// a wider one at a proportionally coarser one, so a large face shot at 45 megapixels isn't
/// judged on detail finer than any lens draws at that size.
pub const MEASURE_EYE_W: f32 = 80.0;

/// One eye's measurement: (score, centre, width) from its outline points (photo pixels). `None`
/// when the outline is degenerate.
pub fn measure_eye(img: &Rgba8, outline: &[[f32; 2]]) -> Option<(f32, [f32; 2], f32)> {
    let xs = outline.iter().map(|p| p[0]).filter(|v| v.is_finite());
    let ys = outline.iter().map(|p| p[1]).filter(|v| v.is_finite());
    let (x0, x1) = xs.clone().fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), v| (a.min(v), b.max(v)));
    let (y0, y1) = ys.clone().fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), v| (a.min(v), b.max(v)));
    if !(x1 > x0 && y1 >= y0) {
        return None;
    }
    let ew = (x1 - x0).max(4.0);
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    Some((measure_eye_at(img, [cx, cy], ew)?, [cx, cy], ew))
}

/// [`focus_score`] of the eye centred on `c` and `ew` pixels wide: the eye with its lids, lashes,
/// creases and the brow above it (a laughing or blinking eye has little detail of its own, and
/// the brow hairs and lid creases are in the same focal plane), at least 12 pixels each way, at a
/// scale that follows the eye's size ([`MEASURE_EYE_W`]).
pub fn measure_eye_at(img: &Rgba8, c: [f32; 2], ew: f32) -> Option<f32> {
    if !(ew.is_finite() && ew > 0.0) {
        return None;
    }
    // a small eye (a face at the back of a group) with the brow and the bridge of the nose: too
    // few pixels of its own to tell detail from grain
    let eye = Region::around(c[0], c[1] - ew * 0.2, (ew * 1.5).max(32.0), (ew * 1.2).max(24.0), img.width, img.height)?;
    let k = 2.0 * (ew / MEASURE_EYE_W).max(1.0);
    // the grain at the measure's scale, from the region itself: its median absolute Laplacian
    // (the edges that make an eye sharp are a minority of its pixels)
    let noise = noise_level(img, eye, k);
    Some(focus_score(img, eye, noise, k))
}

/// How an eye's surroundings look on the photo, around its centre `c` (`ew` pixels wide), against
/// the cheek below it: what tells an eye behind a dark lens (sunglasses) from an eye.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EyeLook {
    /// Share of the eye's opening that is white of the eye: lighter than the lid would be and
    /// without skin's warmth.
    pub sclera: f32,
    /// Mean luminance of the eye and the brow's underside over the cheek's: a lens is a fraction
    /// of the cheek's, an eye in its socket's shade still about half or more.
    pub socket: f32,
    /// Share of the socket much darker than the cheek (all of it behind a lens).
    pub socket_dark: f32,
    /// Just under the eye (lower lid, top of the cheek) over the cheek further down: skin over
    /// skin (0.6–0.9), or a lens's lower half over skin (0.15–0.25).
    pub under: f32,
}

/// Chroma over luminance of an sRGB pixel: skin is warm (high), the white of the eye, grey
/// lenses and shade-blue whites low.
fn warmth(p: [u8; 4]) -> f32 {
    let (r, g, b) = (f32::from(p[0]), f32::from(p[1]), f32::from(p[2]));
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    // warm: red above blue; a cold (bluish) tint counts as no warmth
    let warm = (r - b).max(0.0);
    if mx < 1.0 { 0.0 } else { (0.5 * (mx - mn) + 0.5 * warm) / mx }
}

/// [`EyeLook`] of the eye at `c`, `ew` wide, its axis at `angle` (radians); `None` without skin
/// to compare (off the picture).
pub fn eye_look(img: &Rgba8, c: [f32; 2], ew: f32, angle: f32) -> Option<EyeLook> {
    eye_look_with(img, c, ew, angle, |_, _| {})
}

/// Luminance at `(x, y)` (bilinear; `None` off the picture).
fn luma_at(img: &Rgba8, x: f32, y: f32) -> Option<f32> {
    if !(x.is_finite() && y.is_finite()) || x < 0.0 || y < 0.0 {
        return None;
    }
    let (x0, y0) = (x as usize, y as usize);
    if x0 + 1 >= img.width || y0 + 1 >= img.height {
        return None;
    }
    let (tx, ty) = (x - x0 as f32, y - y0 as f32);
    let p = |x: usize, y: usize| img.data.get(y * img.width + x).copied().map(luma).unwrap_or(0.0);
    let top = p(x0, y0) * (1.0 - tx) + p(x0 + 1, y0) * tx;
    let bot = p(x0, y0 + 1) * (1.0 - tx) + p(x0 + 1, y0 + 1) * tx;
    Some(top * (1.0 - ty) + bot * ty)
}

/// [`eye_look`], telling `mark` which pixels counted as white of the eye (for looking at it).
pub fn eye_look_with(img: &Rgba8, c: [f32; 2], ew: f32, angle: f32, mut mark: impl FnMut(usize, usize)) -> Option<EyeLook> {
    if !(ew.is_finite() && ew >= 4.0 && c.iter().all(|v| v.is_finite())) {
        return None;
    }
    let pixels = |r: Region| {
        let mut px = Vec::with_capacity(r.w * r.h);
        for y in r.y..r.y + r.h {
            px.extend_from_slice(img.data.get(y * img.width + r.x..y * img.width + r.x + r.w).unwrap_or(&[]));
        }
        px
    };
    // the cheek: under the eye, clear of the lower lid and of the nostril
    let cheek = pixels(Region::around(c[0], c[1] + ew * 0.75, ew * 0.7, ew * 0.35, img.width, img.height)?);
    if cheek.is_empty() {
        return None;
    }
    let mut ls: Vec<f32> = cheek.iter().map(|p| luma(*p)).collect();
    ls.sort_by(f32::total_cmp);
    let skin_l = ls.get(ls.len() / 2).copied().unwrap_or(0.0).max(0.02);
    let mut ws: Vec<f32> = cheek.iter().map(|p| warmth(*p)).collect();
    ws.sort_by(f32::total_cmp);
    let skin_w = ws.get(ws.len() / 2).copied().unwrap_or(0.0);
    // the opening: the eye's width, a third of it tall
    let or = Region::around(c[0], c[1], ew * 0.9, (ew * 0.32).max(3.0), img.width, img.height)?;
    let mut white = 0usize;
    for y in or.y..or.y + or.h {
        for x in or.x..or.x + or.w {
            let Some(p) = img.data.get(y * img.width + x).copied() else { continue };
            if luma(p) > skin_l * 0.55 && warmth(p) < skin_w * 0.55 + 0.02 {
                white += 1;
                mark(x, y);
            }
        }
    }
    let sclera = white as f32 / (or.w * or.h).max(1) as f32;
    // the socket: the eye and the shade under the brow, as wide as a lens
    let sock = pixels(Region::around(c[0], c[1] - ew * 0.1, ew * 1.3, ew * 0.7, img.width, img.height)?);
    let sn = sock.len().max(1) as f32;
    let socket = sock.iter().map(|p| luma(*p)).sum::<f32>() / sn / skin_l;
    let socket_dark = sock.iter().filter(|p| luma(**p) < skin_l * 0.4).count() as f32 / sn;
    // just under the eye against the cheek further down, along the levelled axis
    let (ca, sa) = (angle.cos(), angle.sin());
    let row = |v0: f32| -> Option<f32> {
        let px: Vec<f32> = (0..20)
            .flat_map(|i| (0..3).map(move |j| ((i as f32 / 19.0 - 0.5) * 0.8 * ew, (v0 + 0.04 * j as f32) * ew)))
            .filter_map(|(u, v)| luma_at(img, c[0] + u * ca - v * sa, c[1] + u * sa + v * ca))
            .collect();
        (!px.is_empty()).then(|| px.iter().sum::<f32>() / px.len() as f32)
    };
    let under = match (row(0.3), row(0.95)) {
        (Some(a), Some(b)) if b > 0.005 => a / b,
        _ => 1.0,
    };
    let v = EyeLook { sclera, socket, socket_dark, under };
    [v.sclera, v.socket, v.socket_dark, v.under].iter().all(|x| x.is_finite()).then_some(v)
}

/// A dark lens over an eye FaceMesh didn't place (a face turned away): most of the socket much
/// darker than the cheek, no white of the eye showing, the top of the cheek behind it too.
pub fn behind_lens(look: &EyeLook) -> bool {
    look.socket < LENS_SOCKET && look.socket_dark > LENS_DARK && look.sclera < 0.03 && look.under < LENS_UNDER
}

/// [`EyeLook::socket`] below this: dark enough for a lens.
pub const LENS_SOCKET: f32 = 0.40;
/// [`EyeLook::socket_dark`] above this (for an eye without landmarks): a lens.
pub const LENS_DARK: f32 = 0.7;

/// The faces in `img` (photo pixels), on a copy [`DETECT_EDGE`] pixels long: BlazeFace short range
/// for the people near the camera, and BlazeFace full range for the small faces of a group (those
/// of a 60-person group on a 45-megapixel frame are about 12 pixels wide there, inside its range).
#[cfg(feature = "sam")]
pub fn find_faces(model: &lcc_segment::people::FaceModel, img: &Rgba8) -> Result<Vec<lcc_segment::mediapipe::Detection>, String> {
    use lcc_segment::mediapipe::Detection;
    let (w, h) = (img.width, img.height);
    let scaled = |d: &Detection, k: f32| Detection {
        x0: d.x0 * k,
        y0: d.y0 * k,
        x1: d.x1 * k,
        y1: d.y1 * k,
        score: d.score,
        keypoints: d.keypoints.map(|p| [p[0] * k, p[1] * k]),
    };
    let s = (DETECT_EDGE as f32 / w.max(h) as f32).min(1.0);
    let (sw, sh) = (((w as f32 * s).round() as usize).max(1), ((h as f32 * s).round() as usize).max(1));
    let small = rgb_resized(img, sw, sh);
    let found = model.detect_all(&small, sw, sh).map_err(|e| e.to_string())?;
    Ok(found.iter().map(|d| scaled(d, 1.0 / s)).collect())
}

/// The axis of eye `k` from its FaceMesh corners (radians; 0 when they are missing).
pub fn eye_angle(lm: &[[f32; 2]], k: usize) -> f32 {
    let Some((corners, _, _)) = LIDS.get(k) else { return 0.0 };
    let (Some(a), Some(b)) = (lm.get(corners[0]), lm.get(corners[1])) else { return 0.0 };
    // left to right on the picture, whichever corner FaceMesh names first
    let (a, b) = if a[0] <= b[0] { (a, b) } else { (b, a) };
    let t = (b[1] - a[1]).atan2(b[0] - a[0]);
    if t.is_finite() { t } else { 0.0 }
}

/// A face at least this share of the widest one's width is a main subject ([`EyeShot::main`]).
pub const MAIN_FACE: f32 = 0.5;

/// What became of a face found.
#[cfg(feature = "sam")]
enum Judged {
    Eye(Box<EyeShot>),
    /// A face whose eyes couldn't be placed (turned away, hidden).
    NoEyes,
    /// Not a face after all.
    NotAFace,
}

/// A face without landmarks counts only when the detector is at least this sure of it (a card on a
/// table scored 0.76 on a wedding's reception frames; real faces in profile 0.81 and up).
#[cfg_attr(not(feature = "sam"), allow(dead_code))]
const NO_MESH_SCORE: f32 = 0.80;
/// Detections below this are not faces. Set on 173 faces of a wedding's frames (groups, tables,
/// the dance floor with phone lights): 10 of the 16 false ones (dot highlights, bokeh, a hand,
/// a phone's camera) scored 0.50–0.58, 2 of 153 real faces under 0.60.
pub const MIN_FACE_SCORE: f32 = 0.60;
/// Detections below this score are checked with the people model ([`head_share`]), when it is
/// installed.
pub const HEAD_CHECKED_BELOW: f32 = 0.75;
/// [`head_share`] below this: no head there (a vase read 0.05, a light 0.06; most real faces,
/// blurred ones in the background too, from 0.1).
pub const HEAD_MIN: f32 = 0.08;

/// One eye of a face, measured.
#[cfg(feature = "sam")]
#[derive(Clone, Copy, Debug)]
struct Eye {
    score: f32,
    centre: [f32; 2],
    width: f32,
    openness: Option<f32>,
    iris: Option<f32>,
    contrast: Option<f32>,
    look: Option<EyeLook>,
    lids: Lids,
}

/// What FaceMesh's landmarks say about an eye, all measures together:
/// - an eye under [`SMALL_EYE`] pixels wide (a face at the back of a group): FaceMesh's lids are
///   guesses at that size, so only a lid gap near zero with no iris showing is closed;
/// - a face turned away (one eye judged): FaceMesh draws a closed eye seen side on a little open,
///   but the lid hides the iris ([`iris_showing`] at [`TURNED_IRIS_HIDDEN`] or more);
/// - otherwise [`lids_from`].
///
/// A dark lens ([`iris_contrast`] under [`LENS_CONTRAST`], the socket dark and the lens's lower
/// half darkening the top of the cheek, [`EyeLook::under`] under [`LENS_UNDER`]) is no eye.
pub fn lids_of_eye(openness: Option<f32>, iris: Option<f32>, contrast: Option<f32>, look: Option<&EyeLook>, ew: f32, turned: bool) -> Lids {
    if contrast.is_some_and(|c| c < LENS_CONTRAST) && look.is_some_and(|l| l.socket < LENS_SOCKET && l.under < LENS_UNDER) {
        return Lids::Hidden;
    }
    if ew < SMALL_EYE {
        return match (openness, iris) {
            (Some(o), i) if o < 0.08 && i.is_none_or(|i| i >= IRIS_HIDDEN) => Lids::Closed,
            _ => Lids::Unknown,
        };
    }
    let l = lids_from(openness, iris);
    // (a lid gap FaceMesh calls half closed needs less to be closed)
    let hidden = |i: f32| i >= TURNED_IRIS_HIDDEN || (openness.is_some_and(|o| (CLOSED_BELOW..CLOSED_GAP).contains(&o)) && i >= TURNED_IRIS_HALF);
    if turned && l != Lids::Closed && iris.is_some_and(hidden) {
        return Lids::Closed;
    }
    l
}

/// Eyes narrower than this (pixels) have no reliable lids (see [`lids_of_eye`]).
pub const SMALL_EYE: f32 = 22.0;
/// On a face turned away, [`iris_showing`] at or above this: the lid covers the iris.
pub const TURNED_IRIS_HIDDEN: f32 = 0.22;
/// The same with a lid gap FaceMesh already calls half closed.
pub const TURNED_IRIS_HALF: f32 = 0.15;
/// [`iris_contrast`] below this, with a dark socket: a lens.
pub const LENS_CONTRAST: f32 = 0.06;
/// [`EyeLook::under`] below this: the top of the cheek is behind a lens too. Closed eyes with
/// dark lashes (whose iris contrast is as low as a lens's) read 0.35–0.7, lenses 0.15–0.25.
pub const LENS_UNDER: f32 = 0.3;

/// Look at `img` (the photo at full size) with `model`: each person's sharper eye.
#[cfg(feature = "sam")]
pub fn analyze(
    model: &lcc_segment::people::FaceModel,
    people: Option<&lcc_segment::people::PeopleModel>,
    img: &Rgba8,
    photo: PhotoId,
) -> Result<EyeReport, String> {
    use lcc_segment::mediapipe::{Detection, EYES, IRISES};
    let t0 = web_time::Instant::now();
    let (w, h) = (img.width, img.height);
    if w < 16 || h < 16 {
        return Ok(EyeReport { photo, size: (w, h), ..EyeReport::empty() });
    }
    let found = find_faces(model, img)?;
    let t_detect = t0.elapsed().as_secs_f64() * 1000.0;
    let boxes: Vec<[f32; 4]> = found.iter().map(|d| [d.x0, d.y0, d.x1, d.y1]).collect();
    use rayon::prelude::*;
    // each face on its own (FaceMesh is small: faces side by side use the cores better)
    let judged: Vec<Judged> = people_order(&boxes)
        .par_iter()
        .filter_map(|&i| found.get(i))
        .map(|full| {
            // a low-scoring detection is a pattern far more often than a face: dot highlights,
            // bokeh, a hand (not worth FaceMesh's time)
            if full.score < MIN_FACE_SCORE {
                return Judged::NotAFace;
            }
            // FaceMesh on a full-resolution crop around the face (the eye contours are a few pixels
            // wide in the small copy)
            let landmarks = face_crop(img, full).and_then(|(rgb, cw, ch, ox, oy)| {
                let local = Detection {
                    x0: full.x0 - ox,
                    y0: full.y0 - oy,
                    x1: full.x1 - ox,
                    y1: full.y1 - oy,
                    score: full.score,
                    keypoints: full.keypoints.map(|p| [p[0] - ox, p[1] - oy]),
                };
                model.landmarks(&rgb, cw, ch, &local).ok().flatten().map(|l| l.into_iter().map(|p| [p[0] + ox, p[1] + oy]).collect::<Vec<_>>())
            });
            let measured: [Option<Eye>; 2] = match &landmarks {
                Some(l) => {
                    let outline = |e: &[usize; 16]| e.iter().filter_map(|&j| l.get(j).copied()).collect::<Vec<_>>();
                    [0, 1].map(|k| {
                        let (score, c, ew) = EYES.get(k).and_then(|e| measure_eye(img, &outline(e)))?;
                        // shown centred on the iris (the lashes and the pupil's rim are where a missed
                        // focus shows first), unless FaceMesh put it outside the eye (a face looking down)
                        let iris = IRISES.get(k).and_then(|(c, _)| l.get(*c)).copied().filter(|p| p.iter().all(|v| v.is_finite()));
                        let iris = iris.filter(|p| (p[0] - c[0]).abs() < ew * 0.6 && (p[1] - c[1]).abs() < ew * 0.4);
                        let (open, shows) = (openness(l, k), iris_showing(img, l, k));
                        let look = eye_look(img, c, ew, eye_angle(l, k));
                        // the lids are judged once it is known whether the face is turned away
                        Some(Eye {
                            score,
                            centre: iris.unwrap_or(c),
                            width: ew,
                            openness: open,
                            iris: shows,
                            contrast: iris_contrast(img, l, k),
                            look,
                            lids: Lids::Unknown,
                        })
                    })
                }
                None => keypoint_eyes(img, full),
            };
            let [a, b] = measured.map(|e| e.map(|e| (e.score, (e, e.width))));
            let (a, b) = visible_eyes(a, b);
            let both = a.is_some() && b.is_some();
            let judge = |e: Option<(f32, (Eye, f32))>| {
                e.map(|(s, (mut e, w))| {
                    if landmarks.is_some() {
                        e.lids = lids_of_eye(e.openness, e.iris, e.contrast, e.look.as_ref(), e.width, !both);
                    }
                    (s, (e, w))
                })
            };
            let (mut a, mut b) = (judge(a), judge(b));
            // sunglasses cover both eyes: one seen as a lens and the other not clearly an eye is a
            // person whose eyes don't show
            let hidden = |e: &Option<(f32, (Eye, f32))>| e.as_ref().is_some_and(|e| e.1.0.lids == Lids::Hidden);
            let clear = |e: &Option<(f32, (Eye, f32))>| {
                e.as_ref().is_none_or(|e| {
                    e.1.0.contrast.is_none_or(|c| c >= 2.0 * LENS_CONTRAST)
                        || e.1.0.look.is_none_or(|l| l.socket >= LENS_SOCKET || l.under >= LENS_UNDER * 1.3)
                })
            };
            if (hidden(&a) && !clear(&b)) || (hidden(&b) && !clear(&a)) {
                for e in [&mut a, &mut b].into_iter().flatten() {
                    e.1.0.lids = Lids::Hidden;
                }
            }
            // both eyes measured and, together, under half open: a half-closed person, even when
            // the more open eye alone passes (the eye shown is the more open one)
            if let (Some(x), Some(y)) = (&mut a, &mut b)
                && let (Some(ox), Some(oy)) = (x.1.0.openness, y.1.0.openness)
                && both_half_closed(ox, oy)
            {
                for e in [x, y] {
                    if e.1.0.lids == Lids::Open {
                        e.1.0.lids = Lids::Half;
                    }
                }
            }
            // the open eye if there is one, then the sharper
            // a face FaceMesh doesn't take, found by the far detector with a middling score, is as
            // often a pattern (a painted sign, foliage) as a face: not counted
            if landmarks.is_none() && full.score < NO_MESH_SCORE {
                return Judged::NotAFace;
            }
            // a middling detection the people model doesn't see a head in: a light, a vase, a
            // phone's camera (FaceMesh fits a face to anything it is given)
            if full.score < HEAD_CHECKED_BELOW
                && people.is_some_and(|p| head_share(p, img, [full.x0, full.y0, full.x1, full.y1]).is_some_and(|h| h < HEAD_MIN))
            {
                return Judged::NotAFace;
            }
            let Some((score, (e, _))) = pick_eye(a, b, |e| e.1.0.lids) else {
                return Judged::NoEyes;
            };
            let Some(crop) = crop_at(img, e.centre[0], e.centre[1]) else { return Judged::NoEyes };
            let sh = sharpness(score);
            Judged::Eye(Box::new(EyeShot {
                face: [full.x0 / w as f32, full.y0 / h as f32, full.x1 / w as f32, full.y1 / h as f32],
                center: [e.centre[0] / w as f32, e.centre[1] / h as f32],
                crop: Arc::new(crop),
                sharpness: sh,
                relative: 100.0,
                focus: Focus::of(sh),
                both_eyes: both,
                openness: e.openness,
                iris: e.iris,
                contrast: e.contrast,
                look: e.look,
                eye_width: e.width,
                score: full.score,
                main: true,
                landmarks: landmarks.is_some(),
                lids: e.lids,
            }))
        })
        .collect();
    let no_eyes = judged.iter().filter(|j| matches!(j, Judged::NoEyes)).count();
    let not_faces = judged.iter().filter(|j| matches!(j, Judged::NotAFace)).count();
    let mut eyes: Vec<EyeShot> = judged.into_iter().filter_map(|j| if let Judged::Eye(e) = j { Some(*e) } else { None }).collect();
    let widest = eyes.iter().map(|e| e.face[2] - e.face[0]).fold(0.0f32, f32::max);
    for e in &mut eyes {
        e.main = e.face[2] - e.face[0] >= widest * MAIN_FACE;
    }
    judge_relative(&mut eyes);
    log::debug!("eyes: {} faces, detect {t_detect:.0} ms, judge {:.0} ms", found.len(), t0.elapsed().as_secs_f64() * 1000.0 - t_detect);
    let boxes = boxes.iter().map(|b| [b[0] / w as f32, b[1] / h as f32, b[2] / w as f32, b[3] / h as f32]).collect();
    Ok(EyeReport { photo, eyes, faces: found.len().saturating_sub(not_faces), no_eyes, boxes, size: (w, h), ms: t0.elapsed().as_secs_f64() * 1000.0 })
}

/// The people model beside the face model's folder `face_dir` (the models folder's
/// `mediapipe-selfie-multiclass`), when it is installed: it tells a face from a pattern in
/// [`analyze`].
#[cfg(feature = "sam")]
pub fn people_beside(face_dir: &std::path::Path) -> Option<lcc_segment::people::PeopleModel> {
    let dir = face_dir.parent()?.join(lcc_segment::models::PEOPLE.dir_name);
    lcc_segment::models::PEOPLE.installed_in(&dir).then(|| lcc_segment::people::PeopleModel::load(&dir).ok()).flatten()
}

/// How much of a face box (`[x0, y0, x1, y1]`, pixels of `img`) the people model sees as a
/// person's head — face skin, hair or skin, averaged over the box's middle — on a crop twice the
/// box's size around it (0..1; `None` when there is no crop to take): a light, a vase or a phone's
/// camera reads near 0, a face, even a blurred one in the background, well above.
#[cfg(feature = "sam")]
pub fn head_share(people: &lcc_segment::people::PeopleModel, img: &Rgba8, b: [f32; 4]) -> Option<f32> {
    let (cx, cy) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
    let side = (b[2] - b[0]).max(b[3] - b[1]) * 2.0;
    if !(side.is_finite() && side >= 4.0) {
        return None;
    }
    let r = Region::around(cx, cy, side, side, img.width, img.height)?;
    let mut rgb = Vec::with_capacity(r.w * r.h * 3);
    for y in r.y..r.y + r.h {
        for p in img.data.get(y * img.width + r.x..y * img.width + r.x + r.w)? {
            rgb.extend_from_slice(&p[..3]);
        }
    }
    let map = people.segment(&rgb, r.w, r.h).ok()?;
    // the middle half of the face box, in the crop's normalized coordinates
    let (mut sum, mut n) = (0.0f32, 0.0f32);
    for j in 0..5 {
        for i in 0..5 {
            let x = b[0] + (b[2] - b[0]) * (0.25 + 0.5 * (i as f32 + 0.5) / 5.0);
            let y = b[1] + (b[3] - b[1]) * (0.25 + 0.5 * (j as f32 + 0.5) / 5.0);
            let (u, v) = ((x - r.x as f32) / r.w.max(1) as f32, (y - r.y as f32) / r.h.max(1) as f32);
            sum += map.at(&[1, 2, 3], u, v);
            n += 1.0;
        }
    }
    Some(sum / n)
}

/// A face FaceMesh doesn't take (a profile, a face half hidden): its eyes from BlazeFace's eye
/// points, each moved onto the darkest eye-sized spot near it (the points are rough on such
/// faces) and judged by their look. A point that lands off the face (on hair, an ear, the
/// background) finds no skin around it, or no eye-like spot, and is dropped.
#[cfg(feature = "sam")]
fn keypoint_eyes(img: &Rgba8, d: &lcc_segment::mediapipe::Detection) -> [Option<Eye>; 2] {
    let ew = (d.width() * 0.2).max(4.0);
    let [r, l] = [d.keypoints[0], d.keypoints[1]];
    let angle = (l[1] - r[1]).atan2(l[0] - r[0]);
    let angle = if angle.is_finite() && angle.abs() < 0.8 { angle } else { 0.0 };
    [0usize, 1].map(|k| {
        let p = *d.keypoints.get(k)?;
        if !p.iter().all(|v| v.is_finite()) {
            return None;
        }
        let c = darkest_spot(img, p, ew)?;
        let score = measure_eye_at(img, c, ew)?;
        let look = eye_look(img, c, ew, angle)?;
        // without lids to measure, only a lens is told apart
        let lids = if behind_lens(&look) { Lids::Hidden } else { Lids::Unknown };
        Some(Eye { score, centre: c, width: ew, openness: None, iris: None, contrast: None, look: Some(look), lids })
    })
}

/// The centre of the darkest `ew × ew/3` spot within half an eye's width of `p` sideways and a
/// third of it up or down (lashes, iris and the lid's crease are the darkest things there; the brow
/// is further up), with the cheek below it lighter than the spot. `None` when nothing there is
/// darker than the cheek.
pub fn darkest_spot(img: &Rgba8, p: [f32; 2], ew: f32) -> Option<[f32; 2]> {
    if !(ew.is_finite() && ew > 0.0 && p.iter().all(|v| v.is_finite())) {
        return None;
    }
    let (bw, bh) = (ew.max(3.0), (ew / 3.0).max(2.0));
    let step = (ew / 8.0).max(1.0);
    let mean = |c: [f32; 2], w: f32, h: f32| -> Option<f32> {
        let r = Region::around(c[0], c[1], w, h, img.width, img.height)?;
        let (mut s, mut n) = (0.0f32, 0u32);
        let stride = ((r.w * r.h) / 400).max(1);
        for (i, y) in (r.y..r.y + r.h).enumerate() {
            for x in (r.x + i % stride..r.x + r.w).step_by(stride) {
                if let Some(px) = img.data.get(y * img.width + x) {
                    s += luma(*px);
                    n += 1;
                }
            }
        }
        (n > 0).then(|| s / n as f32)
    };
    let steps = |half: f32| -> Vec<f32> {
        let n = (half / step).floor().min(64.0) as i32;
        (-n..=n).map(|i| i as f32 * step).collect()
    };
    let mut best: Option<([f32; 2], f32)> = None;
    for dy in steps(ew / 3.0) {
        for dx in steps(ew / 2.0) {
            let c = [p[0] + dx, p[1] + dy];
            let Some(v) = mean(c, bw, bh) else { continue };
            if best.is_none_or(|b| v < b.1) {
                best = Some((c, v));
            }
        }
    }
    let (c, v) = best?;
    let cheek = mean([c[0], c[1] + ew * 0.75], ew * 0.7, ew * 0.35)?;
    (v < cheek * 0.85).then_some(c)
}

/// Each eye's sharpness against the sharpest of the photo ([`EyeShot::relative`]), and its focus
/// verdict eased when it is close to it: a fast lens wide open and people across the frame make
/// an eye as sharp as that photo gets read low on the absolute scale, and the culler compares
/// faces within the frame. An eye well below the photo's best keeps its own verdict.
pub fn judge_relative(eyes: &mut [EyeShot]) {
    let best = eyes.iter().filter(|e| matches!(e.lids, Lids::Open | Lids::Half)).map(|e| e.sharpness).fold(0.0f32, f32::max);
    for e in eyes.iter_mut() {
        e.relative = if best > 0.0 { (e.sharpness / best * 100.0).min(100.0) } else { 0.0 };
        e.focus = focus_in_frame(e.sharpness, best);
    }
}

/// The focus verdict of an eye scoring `sharpness` in a photo whose best open eye scores `best`
/// (see [`judge_relative`]).
pub fn focus_in_frame(sharpness: f32, best: f32) -> Focus {
    let own = Focus::of(sharpness);
    if !(best.is_finite() && sharpness.is_finite()) || best < RELATIVE_FROM {
        return own;
    }
    let rel = sharpness / best;
    // never above the photo's best eye
    let ceiling = Focus::of(best);
    let eased = if rel >= RELATIVE_SHARP {
        ceiling
    } else if rel >= RELATIVE_SOFT && ceiling != Focus::Blurry {
        Focus::Soft
    } else {
        own
    };
    if eased.rank() > own.rank() { eased } else { own }
}

/// The best eye must read at least this for the others to be judged against it (in a frame
/// where every eye is missed, all stay blurry).
pub const RELATIVE_FROM: f32 = SOFT_FROM;
/// At this share of the best eye or more: as sharp as it (never above it); at [`RELATIVE_SOFT`]
/// or more, at worst soft. On the 85 mm f/1.4 portraits of five people in one plane, the eyes
/// read 62–100 % of the best.
pub const RELATIVE_SHARP: f32 = 0.75;
pub const RELATIVE_SOFT: f32 = 0.55;

/// `img` box-averaged to `w × h`, as packed RGB.
#[cfg_attr(not(feature = "sam"), allow(dead_code))]
pub fn rgb_resized(img: &Rgba8, w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 3);
    let (sx, sy) = (img.width as f32 / w as f32, img.height as f32 / h as f32);
    for y in 0..h {
        let (y0, y1) = ((y as f32 * sy) as usize, (((y + 1) as f32 * sy) as usize).max((y as f32 * sy) as usize + 1).min(img.height));
        for x in 0..w {
            let (x0, x1) = ((x as f32 * sx) as usize, (((x + 1) as f32 * sx) as usize).max((x as f32 * sx) as usize + 1).min(img.width));
            let mut acc = [0u32; 3];
            let mut n = 0u32;
            for yy in y0..y1 {
                for p in img.data.get(yy * img.width + x0..yy * img.width + x1).unwrap_or(&[]) {
                    acc[0] += u32::from(p[0]);
                    acc[1] += u32::from(p[1]);
                    acc[2] += u32::from(p[2]);
                    n += 1;
                }
            }
            let n = n.max(1);
            out.extend(acc.map(|v| (v / n) as u8));
        }
    }
    out
}

/// A full-resolution RGB crop around a face (its box 1.8 × as wide and tall), with its offset:
/// (rgb, w, h, x offset, y offset).
#[cfg(feature = "sam")]
fn face_crop(img: &Rgba8, d: &lcc_segment::mediapipe::Detection) -> Option<(Vec<u8>, usize, usize, f32, f32)> {
    let c = d.center();
    let side = d.width().max(d.height()) * 1.8;
    let r = Region::around(c[0], c[1], side, side, img.width, img.height)?;
    let mut rgb = Vec::with_capacity(r.w * r.h * 3);
    for y in r.y..r.y + r.h {
        for p in img.data.get(y * img.width + r.x..y * img.width + r.x + r.w)? {
            rgb.extend_from_slice(&p[..3]);
        }
    }
    Some((rgb, r.w, r.h, r.x as f32, r.y as f32))
}

/// The picture of a job, at full size (sRGB 8-bit, upright).
pub fn load(source: &EyeSource) -> Option<Rgba8> {
    match source {
        EyeSource::Embedded(path, loader) => loader(path, usize::MAX),
        EyeSource::File(path) => {
            let bytes = std::fs::read(path).ok()?;
            let d = lcc_codecs::decode(&bytes, Default::default()).ok()?;
            let o = lcc_geom::Orientation::from_exif(d.orientation);
            Some(d.to_srgb8().oriented(o))
        }
    }
}

impl crate::Session {
    /// The eye finder's job for photo `id`: a raw's camera JPEG, else the image file itself.
    /// Results depend on the picture only, so the key is the photo's content (not its edits).
    pub fn eye_job(&self, id: PhotoId) -> Option<EyeJob> {
        let p = self.catalog.photo(id)?;
        let lcc_catalog::Source::File { path } = &p.source else { return None };
        let key = lcc_preview::Hasher128::new().str(&crate::media::content_key(p)).str("eyes").u64(EYES_VERSION).finish().0 as u64;
        let source = match (&p.kind, &self.media.preview_loader) {
            (lcc_catalog::MediaKind::Raw, Some(l)) => EyeSource::Embedded(path.clone(), l.clone()),
            (lcc_catalog::MediaKind::Image, _) => EyeSource::File(path.clone()),
            _ => return None,
        };
        let store = Some(crate::eye_store::Store { cache: self.media.rendered.clone(), path: path.clone() });
        Some(EyeJob { key, photo: id, source, store })
    }

    /// Where the eye finder's model lives, when it is installed (`None`: it needs downloading,
    /// or this build has no AI masks).
    /// An installed model that lacks the small-face detector (installed by an older the app)
    /// fetches it in the background.
    pub fn eye_model_dir(&mut self) -> Option<std::path::PathBuf> {
        if !self.segmenter.open_model_installed("face") {
            return None;
        }
        if !self.segmenter.open_model_installed("faceFar")
            && let Err(e) = self.segmenter.start_model_download("faceFar", &self.activity)
        {
            log::warn!("eyes: the small-face detector can't be fetched ({e})");
        }
        self.segmenter.open_model_dir("face")
    }
}

/// Wait while the photo on screen is being made (the foreground goes first). The app has no
/// background-preparation scheduler yet, so this only reports whether the run was cancelled.
pub(crate) fn wait_for_foreground(cancel: &std::sync::atomic::AtomicBool) -> bool {
    cancel.load(std::sync::atomic::Ordering::Relaxed)
}

/// Wait (at most `_most`) while a cell or the loupe still lacks its first picture; true when
/// `stop` says to give up. Without a preparation scheduler there is nothing to wait for.
pub(crate) fn wait_for_pictures(stop: &dyn Fn() -> bool, _most: std::time::Duration) -> bool {
    stop()
}

/// What came back for a job.
pub type EyeResult = (u64, PhotoId, Result<Arc<EyeReport>, String>);

/// The eye finder's thread: give it the photos wanted, most wanted first ([`EyeScanner::want`]);
/// it looks at them one at a time and hands back results ([`EyeScanner::finished`]). The model is
/// loaded on the first job and kept.
pub struct EyeScanner {
    shared: Arc<Shared>,
    rx: std::sync::mpsc::Receiver<EyeResult>,
}

struct Shared {
    queue: std::sync::Mutex<(std::collections::VecDeque<EyeJob>, bool)>,
    cv: std::sync::Condvar,
    model_dir: std::path::PathBuf,
}

impl EyeScanner {
    /// A scanner using the Face Landmarker model in `model_dir`. `None` where there are no threads.
    pub fn start(model_dir: std::path::PathBuf) -> Option<EyeScanner> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = model_dir;
            None
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let shared = Arc::new(Shared { queue: Default::default(), cv: Default::default(), model_dir });
            let (tx, rx) = std::sync::mpsc::channel();
            let s = shared.clone();
            std::thread::Builder::new().name("eyes".into()).spawn(move || run(&s, &tx)).ok()?;
            Some(EyeScanner { shared, rx })
        }
    }

    /// The photos to look at, most wanted first; replaces the ones not started yet.
    pub fn want(&self, jobs: Vec<EyeJob>) {
        let mut q = self.shared.queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        q.0 = jobs.into();
        self.shared.cv.notify_one();
    }

    /// Results since the last call.
    pub fn finished(&self) -> Vec<EyeResult> {
        self.rx.try_iter().collect()
    }

    /// The model folder it was started with.
    pub fn model_dir(&self) -> &std::path::Path {
        &self.shared.model_dir
    }
}

impl Drop for EyeScanner {
    fn drop(&mut self) {
        let mut q = self.shared.queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        q.0.clear();
        q.1 = true;
        self.shared.cv.notify_all();
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn run(s: &Shared, tx: &std::sync::mpsc::Sender<EyeResult>) {
    #[cfg(feature = "sam")]
    let mut model: Option<lcc_segment::people::FaceModel> = None;
    #[cfg(feature = "sam")]
    let mut far_checked = false;
    #[cfg(feature = "sam")]
    let mut people: Option<lcc_segment::people::PeopleModel> = None;
    loop {
        let job = {
            let mut q = s.queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            loop {
                if q.1 {
                    return;
                }
                if let Some(j) = q.0.pop_front() {
                    break j;
                }
                q = s.cv.wait(q).unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        };
        // looked at before (this session or another, or by a prep run): read from the preview cache,
        // without waiting for the pictures on screen (a small file, no raw is read)
        let kept = job.store.as_ref().and_then(|st| Some((st.cache.clone(), st.key(&s.model_dir)?)));
        if let Some((cache, key)) = &kept
            && let Some(r) = crate::eye_store::read(cache, *key, job.photo)
        {
            if tx.send((job.key, job.photo, Ok(Arc::new(r)))).is_err() {
                return;
            }
            continue;
        }
        // the pictures on screen first: no raw is read for eyes while a cell or the loupe still
        // waits for its first picture (bounded: the eyes of the photo on screen still come)
        let closing = || s.queue.lock().map(|q| q.1).unwrap_or(true);
        if crate::eyes::wait_for_pictures(&closing, std::time::Duration::from_secs(5)) {
            return;
        }
        let result = crate::guard::catch("finding eyes", || -> Result<Arc<EyeReport>, String> {
            #[cfg(feature = "sam")]
            {
                // the small-face detector may arrive after the model was loaded (fetched in the
                // background for an older install): loaded again once it is there
                let far_arrived =
                    model.as_ref().is_some_and(|m| !m.has_far()) && !far_checked && s.model_dir.join(lcc_segment::people::FAR_FILE).is_file();
                if far_arrived {
                    far_checked = true;
                }
                if model.is_none() || far_arrived {
                    model = Some(lcc_segment::people::FaceModel::load(&s.model_dir).map_err(|e| e.to_string())?);
                    people = people_beside(&s.model_dir);
                }
                let m = model.as_ref().ok_or("no face model")?;
                let img = crate::memory::in_background(|| load(&job.source)).ok_or("the picture could not be read")?;
                let r = analyze(m, people.as_ref(), &img, job.photo).map(Arc::new);
                drop(img);
                crate::memory::release();
                r
            }
            #[cfg(not(feature = "sam"))]
            {
                Err("eye focus needs a build with AI masks".to_string())
            }
        })
        .and_then(|r| r);
        if let (Ok(r), Some((cache, key))) = (&result, &kept) {
            crate::eye_store::write(cache, *key, r);
        }
        if tx.send((job.key, job.photo, result)).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic eye: a dark iris disc with a bright catch-light on mid-grey skin, blurred by
    /// `blur` box passes, with `noise` added (deterministic).
    fn eye(blur: usize, noise: f32) -> Rgba8 {
        let (w, h) = (200usize, 140usize);
        let mut v: Vec<f32> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                let d = ((x - 100.0).powi(2) + (y - 50.0).powi(2)).sqrt();
                let lid = (y - 50.0).abs() > 22.0;
                if (x - 112.0).powi(2) + (y - 44.0).powi(2) < 9.0 {
                    0.95
                } else if d < 18.0 && !lid {
                    0.12
                } else if (x - 100.0).abs() < 40.0 && !lid {
                    0.8
                } else {
                    0.55
                }
            })
            .collect();
        for _ in 0..blur {
            let src = v.clone();
            for y in 1..h - 1 {
                for x in 1..w - 1 {
                    let mut s = 0.0;
                    for dy in 0..3 {
                        for dx in 0..3 {
                            s += src[(y + dy - 1) * w + x + dx - 1];
                        }
                    }
                    v[y * w + x] = s / 9.0;
                }
            }
        }
        let mut seed = 12345u32;
        let data = v
            .into_iter()
            .map(|l| {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                let n = ((seed >> 16) as f32 / 65535.0 - 0.5) * 2.0 * noise;
                let b = ((l + n).clamp(0.0, 1.0) * 255.0) as u8;
                [b, b, b, 255]
            })
            .collect();
        Rgba8 { width: w, height: h, data }
    }

    fn outline() -> Vec<[f32; 2]> {
        vec![[60.0, 50.0], [100.0, 30.0], [140.0, 50.0], [100.0, 70.0]]
    }

    /// A sharp eye scores higher than the same eye blurred, with or without noise, and noise alone
    /// doesn't make a blurred eye look sharp.
    #[test]
    fn a_sharp_eye_scores_above_a_blurred_one() {
        for noise in [0.0, 0.015] {
            let s = |b| measure_eye(&eye(b, noise), &outline()).unwrap().0;
            let (sharp, soft, blurry) = (s(0), s(3), s(24));
            assert!(sharp > soft && soft > blurry, "noise {noise}: {sharp} {soft} {blurry}");
            assert_eq!(Focus::of(sharpness(sharp)), Focus::Sharp, "noise {noise}: {sharp} -> {}", sharpness(sharp));
            assert_eq!(Focus::of(sharpness(blurry)), Focus::Blurry, "noise {noise}: {blurry} -> {}", sharpness(blurry));
        }
        // the measure follows the detail, not the brightness: a darker copy scores alike
        let bright = eye(0, 0.0);
        let dark = Rgba8 { data: bright.data.iter().map(|p| [p[0] / 3, p[1] / 3, p[2] / 3, 255]).collect(), ..bright.clone() };
        let (a, b) = (measure_eye(&bright, &outline()).unwrap().0, measure_eye(&dark, &outline()).unwrap().0);
        assert!((a / b - 1.0).abs() < 0.35, "{a} vs {b}");
    }

    #[test]
    fn the_sharper_eye_is_chosen() {
        assert_eq!(sharper(Some((0.4, "left")), Some((0.9, "right"))).map(|e| e.1), Some("right"));
        assert_eq!(sharper(Some((0.9, "left")), Some((0.4, "right"))).map(|e| e.1), Some("left"));
        assert_eq!(sharper(None, Some((0.1, "right"))).map(|e| e.1), Some("right"));
        assert_eq!(sharper::<&str>(None, None), None);
        // an open eye wins over a sharper closed one; both closed: the sharper
        let lid = |e: &(f32, (&str, Lids))| e.1.1;
        assert_eq!(pick_eye(Some((0.9, ("closed", Lids::Closed))), Some((0.4, ("open", Lids::Open))), lid).map(|e| e.1.0), Some("open"));
        assert_eq!(pick_eye(Some((0.9, ("a", Lids::Closed))), Some((0.4, ("b", Lids::Closed))), lid).map(|e| e.1.0), Some("a"));
        assert_eq!(pick_eye(Some((0.2, ("a", Lids::Open))), Some((0.4, ("b", Lids::Open))), lid).map(|e| e.1.0), Some("b"));
        // a far eye seen side on (a third of the near one's width) is not judged, however textured
        let (near, far) = (Some((0.3, ("near", 90.0))), Some((0.9, ("far", 30.0))));
        let (a, b) = visible_eyes(near, far);
        assert_eq!(sharper(a, b).map(|e| e.1.0), Some("near"));
        let (a, b) = visible_eyes(Some((0.3, ("l", 80.0))), Some((0.9, ("r", 70.0))));
        assert_eq!(sharper(a, b).map(|e| e.1.0), Some("r"), "both visible: the sharper");
        // on pictures: a sharp right eye beside a blurred left one
        let (sharp, blurred) = (eye(0, 0.01), eye(8, 0.01));
        let pick = sharper(measure_eye(&blurred, &outline()).map(|m| (m.0, "blurred")), measure_eye(&sharp, &outline()).map(|m| (m.0, "sharp")));
        assert_eq!(pick.map(|p| p.1), Some("sharp"));
    }

    /// Synthetic FaceMesh eyes: corners 60 px apart, an iris 20 px across, lids `gap` px apart.
    fn lids(gap: f32, iris: bool) -> Vec<[f32; 2]> {
        let mut lm = vec![[f32::NAN, f32::NAN]; 478];
        for (k, ox) in [(0usize, 100.0f32), (1, 300.0)] {
            let (corners, upper, lower) = LIDS[k];
            lm[corners[0]] = [ox, 100.0];
            lm[corners[1]] = [ox + 60.0, 100.0];
            lm[upper[0]] = [ox + 22.0, 100.0 - gap / 2.0];
            lm[upper[1]] = [ox + 38.0, 100.0 - gap / 2.0];
            lm[lower[0]] = [ox + 22.0, 100.0 + gap / 2.0];
            lm[lower[1]] = [ox + 38.0, 100.0 + gap / 2.0];
            if iris {
                let (c, rim) = IRIS_POINTS[k];
                lm[c] = [ox + 30.0, 100.0];
                for (i, p) in rim.iter().enumerate() {
                    let a = i as f32 * std::f32::consts::FRAC_PI_2;
                    lm[*p] = [ox + 30.0 + 10.0 * a.cos(), 100.0 + 10.0 * a.sin()];
                }
            }
        }
        lm
    }

    /// Open, half-closed and closed eyes from their lids; the iris is the yardstick, the eye's
    /// width (a third of it) without one; broken landmarks give no answer.
    #[test]
    fn lids_tell_open_half_and_closed_eyes() {
        for iris in [true, false] {
            assert_eq!(Lids::of(openness(&lids(18.0, iris), 0)), Lids::Open, "iris {iris}");
            assert_eq!(Lids::of(openness(&lids(6.8, iris), 1)), Lids::Half, "iris {iris}");
            assert_eq!(Lids::of(openness(&lids(1.0, iris), 0)), Lids::Closed, "iris {iris}");
        }
        assert!((openness(&lids(20.0, true), 0).unwrap() - 1.0).abs() < 1e-4);
        assert_eq!(openness(&lids(18.0, true)[..100], 0), None);
        assert_eq!(openness(&lids(18.0, true), 5), None);
        assert_eq!(Lids::of(None), Lids::Unknown);
        assert_eq!(Lids::of(Some(f32::NAN)), Lids::Unknown);
        // a narrow gap with the iris hidden is a lowered lid (closed); with some iris under a
        // lowered lid, half closed (the photographer's half-open eyes: 0.33–0.47)
        assert_eq!(lids_from(Some(0.33), Some(0.2)), Lids::Half);
        assert_eq!(lids_from(Some(0.46), Some(0.12)), Lids::Half);
        assert_eq!(lids_from(Some(0.53), Some(0.11)), Lids::Open);
        assert_eq!(lids_from(Some(0.33), Some(0.95)), Lids::Closed);
        assert_eq!(lids_from(Some(0.33), None), Lids::Half);
        assert_eq!(lids_from(Some(0.40), Some(0.95)), Lids::Half);
        assert_eq!(lids_from(Some(0.80), Some(0.95)), Lids::Open, "wide open whatever the iris check says");
        assert_eq!(lids_from(Some(0.10), Some(0.1)), Lids::Closed, "dark lashes over a closed lid");
    }

    /// The largest eight faces, then left to right as the people stand.
    #[test]
    fn people_are_the_largest_faces_left_to_right() {
        let b = |x: f32, s: f32| [x, 0.0, x + s, s];
        let boxes = vec![b(500.0, 50.0), b(100.0, 80.0), b(300.0, 10.0), b(200.0, 120.0)];
        assert_eq!(people_order(&boxes), vec![1, 3, 2, 0]);
        let n = MAX_PEOPLE + 4;
        let many: Vec<[f32; 4]> = (0..n).map(|i| b(i as f32 * 100.0, 10.0 + i as f32)).collect();
        let o = people_order(&many);
        assert_eq!(o.len(), MAX_PEOPLE);
        assert_eq!(o, (4..n).collect::<Vec<_>>(), "the smallest faces are left out, the rest left to right");
        assert!(people_order(&[[f32::NAN, 0.0, 1.0, 1.0]]).is_empty());
    }

    /// Crops stay inside the picture; hostile input never panics.
    #[test]
    fn crops_and_measures_survive_edges_and_junk() {
        let img = eye(0, 0.0);
        let c = crop_at(&img, 0.0, 0.0).unwrap();
        assert_eq!((c.width, c.height), (CROP_W, CROP_H));
        assert_eq!(c.data[0], img.data[0]);
        let tiny = Rgba8::new(3, 2);
        assert_eq!(crop_at(&tiny, 1.0, 1.0).map(|c| (c.width, c.height)), Some((3, 2)));
        assert!(crop_at(&img, f32::NAN, 1.0).is_none());
        assert!(measure_eye(&img, &[]).is_none());
        assert!(measure_eye(&img, &[[f32::NAN, 1.0], [f32::INFINITY, 2.0]]).is_none());
        assert!(measure_eye(&img, &[[-500.0, -500.0], [-400.0, -480.0]]).is_none());
        assert_eq!(sharpness(f32::NAN), 0.0);
        assert_eq!(Focus::of(f32::NAN), Focus::Blurry);
    }

    /// The same eye drawn three times larger (a close portrait on a 45-megapixel frame) scores
    /// about as sharp, not three times softer: the measure follows the eye's size.
    #[test]
    fn the_measure_follows_the_eyes_size() {
        let up3 = |img: &Rgba8| Rgba8 {
            width: img.width * 3,
            height: img.height * 3,
            data: (0..img.width * 3 * img.height * 3).map(|i| img.data[(i / (img.width * 3) / 3) * img.width + (i % (img.width * 3)) / 3]).collect(),
        };
        let small = eye(0, 0.0);
        let big = up3(&small);
        let (a, b) = (measure_eye_at(&small, [100.0, 50.0], 80.0).unwrap(), measure_eye_at(&big, [300.0, 150.0], 240.0).unwrap());
        assert!((b / a - 1.0).abs() < 0.5, "small {a} big {b}");
        // the blurred big eye still reads blurred
        let big_blur = up3(&eye(10, 0.0));
        assert!(measure_eye_at(&big_blur, [300.0, 150.0], 240.0).unwrap() < b * 0.5);
        assert!(measure_eye_at(&small, [f32::NAN, 1.0], 80.0).is_none() && measure_eye_at(&small, [1.0, 1.0], 0.0).is_none());
    }

    fn shot(x: f32, sharpness: f32, lids: Lids, main: bool) -> EyeShot {
        EyeShot {
            face: [x - 0.05, 0.1, x + 0.05, 0.2],
            center: [x, 0.15],
            crop: Arc::new(Rgba8::new(1, 1)),
            sharpness,
            relative: 0.0,
            focus: Focus::of(sharpness),
            both_eyes: true,
            openness: None,
            iris: None,
            contrast: None,
            look: None,
            eye_width: 40.0,
            score: 0.9,
            main,
            landmarks: true,
            lids,
        }
    }

    /// The user's 85 mm f/1.4 frame of five people in one plane read 18, 23, 24, 24 and 29: as
    /// sharp as that lens gets wide open. Within the frame they are all green but the softest,
    /// amber; a frame where every eye is missed stays red; an eye far below the best keeps its own
    /// verdict.
    #[test]
    fn eyes_are_judged_against_the_frames_best() {
        let five = [18.0, 23.0, 24.0, 24.0, 29.0];
        let verdicts: Vec<Focus> = five.iter().map(|s| focus_in_frame(*s, 29.0)).collect();
        assert_eq!(verdicts.iter().filter(|f| **f == Focus::Sharp).count(), 4, "{verdicts:?}");
        assert_eq!(verdicts.iter().filter(|f| **f == Focus::Soft).count(), 1, "{verdicts:?}");
        // a missed frame: the best is blurry, so all stay blurry
        assert!([5.0, 9.0, 12.0].iter().all(|s| focus_in_frame(*s, 12.0) == Focus::Blurry));
        // a missed person in a sharp frame
        assert_eq!(focus_in_frame(6.0, 45.0), Focus::Blurry);
        // never above the best: a soft best makes its peers soft at most
        assert_eq!(focus_in_frame(20.0, 22.0), Focus::Soft);
        assert_eq!(focus_in_frame(f32::NAN, 30.0), Focus::Blurry);
        assert_eq!(focus_in_frame(40.0, f32::NAN), Focus::Sharp);
        // the report's relative scores: the best open eye is 100, closed eyes don't set the bar
        let mut eyes = vec![shot(0.2, 29.0, Lids::Open, true), shot(0.4, 23.0, Lids::Open, true), shot(0.6, 60.0, Lids::Closed, true)];
        judge_relative(&mut eyes);
        assert_eq!(eyes[0].relative, 100.0);
        assert!((eyes[1].relative - 23.0 / 29.0 * 100.0).abs() < 0.01);
        assert_eq!(eyes[1].focus, Focus::Sharp);
        // the verdicts the strip shows
        assert_eq!(eyes[2].verdict(), Verdict::Closed, "closed is closed however sharp");
        assert_eq!(eyes[0].verdict(), Verdict::Sharp);
        assert_eq!(shot(0.1, 5.0, Lids::Open, true).verdict(), Verdict::Blurry);
        assert_eq!(shot(0.1, 50.0, Lids::Hidden, true).verdict(), Verdict::NotVisible);
        assert_eq!(shot(0.1, 50.0, Lids::Half, true).verdict(), Verdict::HalfClosed);
        assert!(Verdict::Closed.is_reject() && Verdict::Blurry.is_reject() && !Verdict::NotVisible.is_reject());
    }

    /// Lids from landmarks: tiny eyes have no verdict (unless shut flat), a turned face whose lid
    /// hides the iris is closed even when FaceMesh draws it open, and a lens is no eye.
    #[test]
    fn lids_of_small_turned_and_hidden_eyes() {
        // a group's back row: FaceMesh's 0.2 gap means nothing at 12 pixels
        assert_eq!(lids_of_eye(Some(0.2), None, Some(0.2), None, 12.0, false), Lids::Unknown);
        assert_eq!(lids_of_eye(Some(0.02), None, Some(0.2), None, 12.0, false), Lids::Closed);
        // the user's kissing frame: side-on closed eyes FaceMesh drew 0.35–0.75 open
        for (o, i) in [(0.47, 0.35), (0.35, 0.40), (0.40, 0.35), (0.75, 0.24), (0.31, 0.21)] {
            assert_eq!(lids_of_eye(Some(o), Some(i), Some(0.05), None, 90.0, true), Lids::Closed, "{o} {i}");
        }
        // the same values on a face seen whole are a squint (the iris shows a little)
        assert_eq!(lids_of_eye(Some(0.75), Some(0.24), Some(0.15), None, 90.0, false), Lids::Open);
        // an open eye of a turned face: the iris shows
        assert_eq!(lids_of_eye(Some(0.56), Some(0.07), Some(0.2), None, 90.0, true), Lids::Open);
        // sunglasses: no iris against whites, a dark socket, the cheek's top behind the lens
        let lens = EyeLook { sclera: 0.0, socket: 0.25, socket_dark: 0.8, under: 0.2 };
        assert_eq!(lids_of_eye(Some(0.5), Some(0.2), Some(0.02), Some(&lens), 60.0, false), Lids::Hidden);
        // closed eyes with dark lashes: as little iris contrast, but skin under the eye
        let lashes = EyeLook { under: 0.55, ..lens };
        assert_eq!(lids_of_eye(Some(0.1), Some(0.9), Some(0.02), Some(&lashes), 60.0, false), Lids::Closed);
        assert!(behind_lens(&lens) && !behind_lens(&lashes));
    }

    /// Where Space zooms without a camera focus point: the main subjects' sharpest open eye, else
    /// the largest face; nothing found, nothing to zoom to.
    #[test]
    fn zoom_target_is_the_sharpest_open_eye_of_the_main_subjects() {
        let r = EyeReport {
            eyes: vec![
                shot(0.2, 30.0, Lids::Open, true),
                shot(0.5, 60.0, Lids::Closed, true),
                shot(0.8, 70.0, Lids::Open, false),
                shot(0.4, 40.0, Lids::Open, true),
            ],
            boxes: vec![[0.0, 0.0, 0.5, 0.5]],
            ..EyeReport::empty()
        };
        assert_eq!(r.zoom_target(), Some([0.4, 0.15]));
        let faces_only = EyeReport { boxes: vec![[0.1, 0.1, 0.2, 0.2], [0.5, 0.5, 0.9, 0.9], [f32::NAN, 0.0, 1.0, 1.0]], ..EyeReport::empty() };
        assert_eq!(faces_only.zoom_target(), Some([0.7, 0.7]));
        assert_eq!(EyeReport::empty().zoom_target(), None);
    }

    /// The eye search lands on the dark of the eye, not on the cheek beside it, and gives up on
    /// skin without one.
    #[test]
    fn the_darkest_spot_finds_the_eye() {
        let img = eye(0, 0.0);
        // a point off to the side and below: the search moves onto the iris
        let c = darkest_spot(&img, [112.0, 60.0], 60.0).unwrap();
        assert!((c[0] - 100.0).abs() < 20.0 && (c[1] - 50.0).abs() < 12.0, "{c:?}");
        let flat = Rgba8 { width: 200, height: 140, data: vec![[150, 120, 100, 255]; 200 * 140] };
        assert!(darkest_spot(&flat, [100.0, 50.0], 60.0).is_none());
        assert!(darkest_spot(&img, [f32::NAN, 50.0], 60.0).is_none());
        assert!(darkest_spot(&img, [100.0, 50.0], f32::INFINITY).is_none());
        // the look of junk positions never panics
        assert!(eye_look(&img, [f32::NAN, 0.0], 40.0, 0.0).is_none());
        assert!(eye_look(&img, [100.0, 50.0], 2.0, 0.0).is_none());
        let _ = eye_look(&img, [199.0, 139.0], 400.0, f32::NAN);
    }

    #[test]
    fn a_pair_of_heavy_lids_is_half_closed_though_one_eye_passes() {
        assert!(both_half_closed(0.42, 0.55), "mean 0.485");
        assert!(!both_half_closed(0.52, 0.60), "mean 0.56: open");
        assert!(!both_half_closed(f32::NAN, 0.1), "an unmeasured eye decides nothing");
    }
}
