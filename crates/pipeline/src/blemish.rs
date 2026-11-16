//! Blemish detection on skin: small, round spots that are darker or redder than the skin around
//! them. Classical, in the spirit of [`crate::dust`]: a blurred copy gives the surrounding skin,
//! a spot has to stand out from both the surroundings and the local texture, and each candidate
//! blob must be small and roughly round.
//!
//! Where to look comes from the open MediaPipe models (in `lcc-segment`, run by the
//! engine): the skin they found (a coverage plane: never hair, lips or the background), each
//! face's outline from its FaceMesh landmarks, and the eyes, brows, nostrils and mouth cut out by
//! the landmark polygons around them, so lashes, brows, nostril edges and lips are never taken
//! for blemishes. A face without landmarks (a strong profile) is left alone. With body skin
//! included, the skin outside the faces (shoulders, arms, back) is searched too, at the scale of
//! the nearest face.
//!
//! Deliberately conservative: a face (or a body) with many candidates has stubble, freckles,
//! moles or pores, not blemishes, and gets none; the rest are capped per face. Removing freckles
//! is a different feature and never a default.

use lcc_develop::SegMask;
use lcc_raster::{Plane, Rgba8, blur::gaussian};

/// FaceMesh landmark indices the detector cuts features out by (data from MediaPipe's
/// `face_landmarks_connections`, Copyright 2023 The MediaPipe Authors, Apache License 2.0).
pub mod mesh {
    /// The face oval (closed).
    pub const FACE_OVAL: [usize; 36] = [
        10, 338, 297, 332, 284, 251, 389, 356, 454, 323, 361, 288, 397, 365, 379, 378, 400, 377, 152, 148, 176, 149, 150, 136, 172, 58, 132, 93, 234,
        127, 162, 21, 54, 103, 67, 109,
    ];
    /// The eye openings (the image's left eye first).
    pub const EYES: [[usize; 16]; 2] = [
        [33, 7, 163, 144, 145, 153, 154, 155, 133, 173, 157, 158, 159, 160, 161, 246],
        [263, 249, 390, 373, 374, 380, 381, 382, 362, 398, 384, 385, 386, 387, 388, 466],
    ];
    /// The brows: upper line then lower line, per brow.
    pub const BROWS: [([usize; 5], [usize; 5]); 2] =
        [([70, 63, 105, 66, 107], [46, 53, 52, 65, 55]), ([300, 293, 334, 296, 336], [276, 283, 282, 295, 285])];
    /// The outer lip line (closed).
    pub const LIPS_OUTER: [usize; 20] = [61, 146, 91, 181, 84, 17, 314, 405, 321, 375, 291, 409, 270, 269, 267, 0, 37, 39, 40, 185];
    /// The lower nose: tip, columella, nostrils and the wings (their convex hull is cut out).
    pub const NOSE: [usize; 22] = [1, 2, 4, 19, 94, 97, 98, 326, 327, 64, 294, 48, 278, 129, 358, 102, 331, 49, 279, 115, 344, 5];
    /// Landmarks at least this many: FaceMesh-V2 (with irises: 478).
    pub const POINTS: usize = 468;
}

/// A face in fractions of the image (0..1, y down): its box, its eyes, nose tip and mouth centre
/// (BlazeFace's keypoints: the right eye, on the image's left, first), and FaceMesh's landmarks
/// when it placed them.
#[derive(Clone, Debug, PartialEq)]
pub struct FaceBox {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
    pub keypoints: [(f64, f64); 4],
    pub mesh: Option<Vec<(f64, f64)>>,
}

/// A found blemish: which face, centre (0..1 of the image's width / height), radius (fraction
/// of the long edge) and how much it stands out (0..1). `body`: on the skin outside the faces
/// (of the person whose face is `face`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blemish {
    pub face: usize,
    pub x: f64,
    pub y: f64,
    pub radius: f64,
    pub strength: f32,
    pub body: bool,
}

/// What to look for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// 0..100: higher finds fainter spots.
    pub sensitivity: f32,
    /// Most kept per face (and per body), strongest first.
    pub max_per_face: usize,
    /// Also search the skin outside the faces.
    pub body: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { sensitivity: 50.0, max_per_face: MAX_PER_FACE, body: false }
    }
}

/// Faces narrower than this (in pixels of the analysed image) are left alone: a blemish is not
/// resolved below it, and what is found there is lash shadow and nostril edges.
pub const MIN_FACE_PX: f64 = 160.0;
/// More candidates than this on one face means texture (stubble, freckles), not blemishes.
pub const MAX_CANDIDATES_PER_FACE: usize = 14;
/// More candidates than this on one body means freckles or moles, not blemishes.
pub const MAX_CANDIDATES_PER_BODY: usize = 10;
/// Most blemishes kept per face, strongest first.
pub const MAX_PER_FACE: usize = 6;
/// Skin coverage below this is not skin to search (the soft edges of the skin mask).
const SKIN_CORE: f32 = 0.85;

/// The coverage (0..1) of a stored skin segmentation at each pixel of a `w × h` image of the
/// whole, uncropped photo it was made for; `None` when the data is damaged.
pub fn skin_plane(seg: &SegMask, w: usize, h: usize) -> Option<Plane> {
    let grid = crate::masks::SegGrid::new(seg)?;
    Some(Plane::from_fn(w, h, |x, y| {
        let n = lcc_geom::Point::new((x as f64 + 0.5) / w.max(1) as f64, (y as f64 + 0.5) / h.max(1) as f64);
        1.0 / (1.0 + (-grid.logit(n)).exp())
    }))
}

/// Whether `p` lies inside the closed polygon `poly` (even–odd rule).
fn inside(poly: &[(f64, f64)], p: (f64, f64)) -> bool {
    let mut c = false;
    let n = poly.len();
    for i in 0..n {
        let (Some(a), Some(b)) = (poly.get(i), poly.get((i + n - 1) % n)) else { continue };
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            c = !c;
        }
    }
    c
}

/// `poly` scaled about its centroid by `sx` horizontally and `sy` vertically.
fn grown(poly: &[(f64, f64)], sx: f64, sy: f64) -> Vec<(f64, f64)> {
    let n = poly.len().max(1) as f64;
    let (cx, cy) = poly.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0 / n, a.1 + p.1 / n));
    poly.iter().map(|p| (cx + (p.0 - cx) * sx, cy + (p.1 - cy) * sy)).collect()
}

/// The convex hull of `pts` (Andrew's monotone chain), for the nose.
fn hull(mut pts: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    pts.retain(|p| p.0.is_finite() && p.1.is_finite());
    pts.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: (f64, f64), a: (f64, f64), b: (f64, f64)| (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0);
    let chain = |pts: &mut dyn Iterator<Item = (f64, f64)>| {
        let mut c: Vec<(f64, f64)> = Vec::new();
        for p in pts {
            while let [.., a, b] = c[..] {
                if cross(a, b, p) > 0.0 {
                    break;
                }
                c.pop();
            }
            c.push(p);
        }
        c.pop();
        c
    };
    let mut lower = chain(&mut pts.iter().copied());
    lower.extend(chain(&mut pts.iter().rev().copied()));
    lower
}

/// A face's geometry in pixels of the analysed image: its outline and the features to cut out.
struct Features {
    oval: Vec<(f64, f64)>,
    cuts: Vec<Vec<(f64, f64)>>,
    /// Width of the face (pixels).
    width: f64,
}

impl Features {
    /// From FaceMesh's landmarks (normalized), in pixels of a `w × h` image; `None` without them.
    fn of(f: &FaceBox, w: f64, h: f64) -> Option<Features> {
        let lm = f.mesh.as_ref().filter(|m| m.len() >= mesh::POINTS)?;
        if !lm.iter().all(|p| p.0.is_finite() && p.1.is_finite()) {
            return None;
        }
        let px = |idx: &[usize]| -> Vec<(f64, f64)> { idx.iter().filter_map(|i| lm.get(*i)).map(|p| (p.0 * w, p.1 * h)).collect() };
        let oval = px(&mesh::FACE_OVAL);
        let (x0, x1) = oval.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
        let mut cuts = Vec::new();
        for eye in &mesh::EYES {
            // the opening grown for the lashes and lash line, and over the shadow beneath
            cuts.push(grown(&px(eye), 1.35, 2.3));
        }
        for (upper, lower) in mesh::BROWS {
            let mut poly = px(&upper);
            poly.extend(px(&lower).into_iter().rev());
            cuts.push(grown(&poly, 1.2, 1.9));
        }
        cuts.push(grown(&px(&mesh::LIPS_OUTER), 1.12, 1.3));
        cuts.push(grown(&hull(px(&mesh::NOSE)), 1.15, 1.12));
        Some(Features { oval, cuts, width: x1 - x0 })
    }

    /// Whether pixel centre `p` is face skin to search: inside the outline, outside every cut.
    fn searchable(&self, p: (f64, f64)) -> bool {
        inside(&self.oval, p) && !self.cuts.iter().any(|c| inside(c, p))
    }

    /// The outline's bounding box `[x0, y0, x1, y1]`.
    fn bounds(&self) -> [f64; 4] {
        let (x0, x1) = self.oval.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
        let (y0, y1) = self.oval.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
        [x0, y0, x1, y1]
    }

    fn centre(&self) -> (f64, f64) {
        let n = self.oval.len().max(1) as f64;
        self.oval.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0 / n, a.1 + p.1 / n))
    }
}

/// Blemishes on the given faces of a rendered image of the whole photo, in its skin `skin` (the
/// coverage of [`skin_plane`] at the image's size; `None`: anything skin-coloured counts).
/// Faces without landmarks, or narrower than [`MIN_FACE_PX`], get none; a face with more than
/// [`MAX_CANDIDATES_PER_FACE`] candidates gets none; at most `opts.max_per_face` per face.
pub fn detect(img: &Rgba8, faces: &[FaceBox], skin: Option<&Plane>, opts: &Options) -> Vec<Blemish> {
    let (w, h) = (img.width, img.height);
    if w < 32 || h < 32 {
        return Vec::new();
    }
    let skin = skin.filter(|s| s.width == w && s.height == h);
    let features: Vec<Option<Features>> = faces.iter().map(|f| Features::of(f, w as f64, h as f64)).collect();
    let mut out = Vec::new();
    for (fi, ft) in features.iter().enumerate() {
        let Some(ft) = ft else { continue };
        if !(ft.width.is_finite() && ft.width >= MIN_FACE_PX) {
            continue;
        }
        let found = search(img, skin, ft.bounds(), ft.width, opts, |p| ft.searchable(p), fi, false);
        if found.len() > MAX_CANDIDATES_PER_FACE {
            continue; // texture, not blemishes
        }
        out.extend(strongest(found, opts.max_per_face));
    }
    if opts.body
        && let Some(skin) = skin
    {
        out.extend(body(img, skin, &features, opts));
    }
    out
}

/// The `n` strongest of `found`.
fn strongest(mut found: Vec<Blemish>, n: usize) -> Vec<Blemish> {
    found.sort_by(|a, b| b.strength.total_cmp(&a.strength));
    found.truncate(n);
    found
}

/// Blemishes on the skin outside the faces, each body measured by its nearest face.
fn body(img: &Rgba8, skin: &Plane, features: &[Option<Features>], opts: &Options) -> Vec<Blemish> {
    let (w, h) = (img.width as f64, img.height as f64);
    let faces: Vec<(usize, &Features, (f64, f64))> = features
        .iter()
        .enumerate()
        .filter_map(|(i, f)| f.as_ref().map(|f| (i, f)))
        .filter(|(_, f)| f.width.is_finite() && f.width >= MIN_FACE_PX)
        .map(|(i, f)| (i, f, f.centre()))
        .collect();
    let ovals: Vec<Vec<(f64, f64)>> = features.iter().flatten().map(|f| grown(&f.oval, 1.1, 1.1)).collect();
    let in_a_face = |p: (f64, f64)| ovals.iter().any(|o| inside(o, p));
    let mut out = Vec::new();
    for (fi, f, c) in &faces {
        // this person's body: the skin nearer to this face (in its own size) than to any other, within reach
        let reach = 6.0 * f.width;
        let region = [(c.0 - reach).max(0.0), (c.1 - 0.5 * f.width).max(0.0), (c.0 + reach).min(w), (c.1 + 2.0 * reach).min(h)];
        let nearest = |p: (f64, f64)| {
            faces
                .iter()
                .map(|(i, g, gc)| (*i, ((p.0 - gc.0).powi(2) + (p.1 - gc.1).powi(2)).sqrt() / g.width))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
        };
        let found = search(img, Some(skin), region, f.width, opts, |p| nearest(p) == Some(*fi) && !in_a_face(p), *fi, true);
        if found.len() > MAX_CANDIDATES_PER_BODY {
            continue; // freckles or moles
        }
        out.extend(strongest(found, opts.max_per_face));
    }
    out
}

/// Candidate blemishes in `region` (`[x0, y0, x1, y1]`, pixels) of `img`, at the scale of a face
/// `fw` pixels wide, where `allowed` and the skin say so.
#[allow(clippy::too_many_arguments)]
fn search(
    img: &Rgba8,
    skin: Option<&Plane>,
    region: [f64; 4],
    fw: f64,
    opts: &Options,
    allowed: impl Fn((f64, f64)) -> bool,
    face: usize,
    body: bool,
) -> Vec<Blemish> {
    let (w, h) = (img.width, img.height);
    let long = w.max(h) as f64;
    let thr = 0.09 - 0.05 * (opts.sensitivity / 100.0).clamp(0.0, 1.0);
    // the region with a margin, clipped to the image
    let clamp = |v: f64, hi: usize| if v.is_finite() { (v.max(0.0) as usize).min(hi) } else { 0 };
    let m = fw * 0.1;
    let (x0, y0, x1, y1) = (clamp(region[0] - m, w), clamp(region[1] - m, h), clamp(region[2] + m, w), clamp(region[3] + m, h));
    let (cw, ch) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
    if cw < 16 || ch < 16 {
        return Vec::new();
    }
    let px = |xx: usize, yy: usize| img.data.get((y0 + yy) * w + x0 + xx).copied().unwrap_or([0; 4]);
    let sigma = (0.035 * fw as f32).max(2.0);
    // the skin, away from its soft edges (the face against the background, the hairline)
    let core = skin.map(|s| {
        let crop = Plane::from_fn(cw, ch, |xx, yy| s.data.get((y0 + yy) * w + x0 + xx).copied().unwrap_or(0.0));
        gaussian(&crop, sigma)
    });
    let mask: Vec<bool> = (0..cw * ch)
        .map(|i| {
            let (xx, yy) = (i % cw, i / cw);
            let p = ((x0 + xx) as f64 + 0.5, (y0 + yy) as f64 + 0.5);
            let in_skin = match &core {
                Some(c) => c.data.get(i).is_some_and(|v| *v >= SKIN_CORE),
                None => {
                    let q = px(xx, yy);
                    let (r, g, b) = (f64::from(q[0]), f64::from(q[1]), f64::from(q[2]));
                    let cr = 128.0 + 0.5 * r - 0.4187 * g - 0.0813 * b;
                    let cb = 128.0 - 0.1687 * r - 0.3313 * g + 0.5 * b;
                    (133.0..180.0).contains(&cr) && (77.0..130.0).contains(&cb)
                }
            };
            in_skin && allowed(p)
        })
        .collect();
    if !mask.iter().any(|v| *v) {
        return Vec::new();
    }
    let lum = Plane::from_fn(cw, ch, |xx, yy| {
        let p = px(xx, yy);
        (0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2])) / 255.0
    });
    // redness: how much red exceeds green, which a pimple does and even skin does not
    let red = Plane::from_fn(cw, ch, |xx, yy| {
        let p = px(xx, yy);
        (f32::from(p[0]) - f32::from(p[1])) / 255.0
    });
    let around_l = gaussian(&lum, sigma);
    let around_r = gaussian(&red, sigma);
    let detail = Plane::from_fn(cw, ch, |xx, yy| {
        let i = yy * cw + xx;
        (lum.data.get(i).copied().unwrap_or(0.0) - around_l.data.get(i).copied().unwrap_or(0.0)).abs()
    });
    let texture = gaussian(&detail, (0.06 * fw as f32).max(3.0));
    let resp: Vec<f32> = (0..cw * ch)
        .map(|i| {
            let dark = around_l.data.get(i).copied().unwrap_or(0.0) - lum.data.get(i).copied().unwrap_or(0.0);
            let redder = red.data.get(i).copied().unwrap_or(0.0) - around_r.data.get(i).copied().unwrap_or(0.0);
            dark.max(redder).max(0.0)
        })
        .collect();
    let candidate = |i: usize| {
        mask.get(i).copied().unwrap_or(false)
            && resp.get(i).copied().unwrap_or(0.0) > thr
            && resp.get(i).copied().unwrap_or(0.0) > 2.5 * texture.data.get(i).copied().unwrap_or(0.0)
    };
    // blobs of candidate pixels (4-connected), small and roundish
    let mut seen = vec![false; cw * ch];
    let mut found: Vec<Blemish> = Vec::new();
    let (rmin, rmax) = ((0.004 * fw).max(1.5), 0.05 * fw);
    for start in 0..cw * ch {
        if seen.get(start).copied().unwrap_or(true) || !candidate(start) {
            continue;
        }
        let mut stack = vec![start];
        if let Some(s) = seen.get_mut(start) {
            *s = true;
        }
        let (mut n, mut sx, mut sy, mut peak) = (0usize, 0.0f64, 0.0f64, 0.0f32);
        let (mut bx0, mut bx1, mut by0, mut by1) = (cw, 0, ch, 0);
        while let Some(i) = stack.pop() {
            let (xx, yy) = (i % cw, i / cw);
            n += 1;
            sx += xx as f64;
            sy += yy as f64;
            peak = peak.max(resp.get(i).copied().unwrap_or(0.0));
            (bx0, bx1, by0, by1) = (bx0.min(xx), bx1.max(xx), by0.min(yy), by1.max(yy));
            for j in [i.wrapping_sub(1), i + 1, i.wrapping_sub(cw), i + cw] {
                let ok = j < cw * ch && !seen.get(j).copied().unwrap_or(true) && (j % cw).abs_diff(xx) <= 1;
                if ok && candidate(j) {
                    if let Some(s) = seen.get_mut(j) {
                        *s = true;
                    }
                    stack.push(j);
                }
            }
        }
        // a blob too big to be a blemish is a shadow or a feature
        let (bw, bh) = ((bx1 - bx0 + 1) as f64, (by1 - by0 + 1) as f64);
        let r = bw.max(bh) / 2.0;
        let round = bw.max(bh) / bw.min(bh) < 3.0 && n as f64 >= 0.35 * bw * bh && n >= 3;
        if r >= rmin && r <= rmax && round {
            found.push(Blemish {
                face,
                x: (x0 as f64 + sx / n as f64 + 0.5) / w as f64,
                y: (y0 as f64 + sy / n as f64 + 0.5) / h as f64,
                radius: (r / long * 1.6).max(fw * 0.012 / long),
                strength: peak,
                body,
            });
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grey frame with one skin-coloured face ellipse `fw` wide at (`cx`, `cy`), a FaceMesh-like
    /// set of landmarks where a face has its features, and `dots` dark spots (in face sizes).
    fn scene_at(w: usize, h: usize, cx: f64, cy: f64, fw: f64, dots: &[(f64, f64)]) -> (Rgba8, FaceBox) {
        let fh = fw * 1.3;
        let mut img = Rgba8::filled(w, h, [120, 120, 120, 255]);
        for y in 0..h {
            for x in 0..w {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                if ((px - cx) / (fw * 0.5)).powi(2) + ((py - cy) / (fh * 0.5)).powi(2) <= 1.0 {
                    img.data[y * w + x] = [222, 180, 160, 255]; // skin
                }
            }
        }
        for &(dx, dy) in dots {
            spot(&mut img, cx + dx * fw, cy + dy * fh, (0.012 * fw).max(1.5));
        }
        let n = |x: f64, y: f64| (x / w as f64, y / h as f64);
        let mut lm = vec![n(cx, cy); 478];
        let ring = |idx: &[usize], ex: f64, ey: f64, rx: f64, ry: f64, lm: &mut Vec<(f64, f64)>| {
            for (k, i) in idx.iter().enumerate() {
                let a = k as f64 / idx.len() as f64 * std::f64::consts::TAU;
                lm[*i] = n(ex + rx * a.cos(), ey + ry * a.sin());
            }
        };
        ring(&mesh::FACE_OVAL, cx, cy, fw * 0.48, fh * 0.48, &mut lm);
        ring(&mesh::EYES[0], cx - fw * 0.2, cy - fh * 0.12, fw * 0.08, fh * 0.025, &mut lm);
        ring(&mesh::EYES[1], cx + fw * 0.2, cy - fh * 0.12, fw * 0.08, fh * 0.025, &mut lm);
        for (b, sx) in [(0usize, -1.0), (1, 1.0)] {
            let (upper, lower) = mesh::BROWS[b];
            for (k, i) in upper.iter().enumerate() {
                lm[*i] = n(cx + sx * fw * (0.1 + 0.05 * k as f64), cy - fh * 0.2);
            }
            for (k, i) in lower.iter().enumerate() {
                lm[*i] = n(cx + sx * fw * (0.1 + 0.05 * k as f64), cy - fh * 0.18);
            }
        }
        ring(&mesh::LIPS_OUTER, cx, cy + fh * 0.28, fw * 0.15, fh * 0.05, &mut lm);
        ring(&mesh::NOSE, cx, cy + fh * 0.06, fw * 0.08, fh * 0.06, &mut lm);
        let face = FaceBox {
            x0: (cx - fw / 2.0) / w as f64,
            y0: (cy - fh / 2.0) / h as f64,
            x1: (cx + fw / 2.0) / w as f64,
            y1: (cy + fh / 2.0) / h as f64,
            keypoints: [n(cx - fw * 0.2, cy - fh * 0.12), n(cx + fw * 0.2, cy - fh * 0.12), n(cx, cy + fh * 0.08), n(cx, cy + fh * 0.28)],
            mesh: Some(lm),
        };
        (img, face)
    }

    fn scene(w: usize, h: usize, fw: f64, dots: &[(f64, f64)]) -> (Rgba8, FaceBox) {
        scene_at(w, h, w as f64 / 2.0, h as f64 / 2.0, fw, dots)
    }

    /// A dark red spot of radius `r` at (`sx`, `sy`).
    fn spot(img: &mut Rgba8, sx: f64, sy: f64, r: f64) {
        let w = img.width;
        for y in 0..img.height {
            for x in 0..w {
                if (x as f64 + 0.5 - sx).powi(2) + (y as f64 + 0.5 - sy).powi(2) <= r * r {
                    img.data[y * w + x] = [150, 90, 85, 255];
                }
            }
        }
    }

    /// The skin of a scene as a coverage plane: everything skin-coloured (spots included).
    fn skin_of(img: &Rgba8) -> Plane {
        Plane::from_fn(img.width, img.height, |x, y| if matches!(img.data[y * img.width + x][0], 222 | 150) { 1.0 } else { 0.0 })
    }

    #[test]
    fn finds_spots_on_the_cheeks_and_nothing_else() {
        let dots = [(-0.28, 0.08), (0.3, 0.12), (-0.2, 0.3)];
        let (img, face) = scene(800, 600, 300.0, &dots);
        let skin = skin_of(&img);
        for skin in [None, Some(&skin)] {
            let found = detect(&img, std::slice::from_ref(&face), skin, &Options::default());
            assert_eq!(found.len(), 3, "{found:?}");
            for (dx, dy) in dots {
                let (ex, ey) = (0.5 + dx * 300.0 / 800.0, 0.5 + dy * 390.0 / 600.0);
                assert!(found.iter().any(|b| (b.x - ex).abs() < 0.01 && (b.y - ey).abs() < 0.01), "{found:?} lacks {ex},{ey}");
            }
            assert!(found.iter().all(|b| b.face == 0 && !b.body && b.radius > 0.0 && b.radius < 0.02 && b.strength > 0.0));
        }
        let (clean, face) = scene(800, 600, 300.0, &[]);
        assert!(detect(&clean, &[face], None, &Options::default()).is_empty(), "a clean face has no blemishes");
    }

    #[test]
    fn features_small_faces_and_faces_without_landmarks_are_left_alone() {
        // spots on the eyes, the brows, the nose and the mouth: all inside the cut-outs
        let dots = [(-0.2, -0.12), (0.2, -0.12), (0.18, -0.19), (0.0, 0.06), (0.03, 0.1), (0.0, 0.28), (0.1, 0.28)];
        let (img, face) = scene(800, 600, 300.0, &dots);
        let opts = Options { sensitivity: 80.0, ..Options::default() };
        let found = detect(&img, std::slice::from_ref(&face), None, &opts);
        assert!(found.is_empty(), "{found:?}");
        // the same cheek spots on a face below the size floor
        let (img, face) = scene(800, 600, 120.0, &[(-0.28, 0.08), (0.3, 0.12)]);
        assert!(detect(&img, &[face], None, &Options::default()).is_empty());
        // or on a face whose landmarks weren't found (a profile)
        let (img, mut face) = scene(800, 600, 300.0, &[(-0.28, 0.08), (0.3, 0.12)]);
        face.mesh = None;
        assert!(detect(&img, &[face], None, &Options::default()).is_empty());
    }

    #[test]
    fn only_the_skin_the_models_found_is_searched() {
        // the right half of the face is not skin to the models (hair over it, a hand)
        let (img, face) = scene(800, 600, 300.0, &[(-0.28, 0.08), (0.3, 0.12)]);
        let skin = Plane::from_fn(800, 600, |x, _| if x < 400 { 1.0 } else { 0.0 });
        let found = detect(&img, &[face], Some(&skin), &Options::default());
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].x < 0.5);
    }

    #[test]
    fn a_speckled_face_is_texture_not_blemishes() {
        // 24 spots on the cheeks, all outside the eye, nose and mouth cut-outs
        let dots: Vec<(f64, f64)> =
            [-0.42, -0.32, -0.22, 0.22, 0.32, 0.42].iter().flat_map(|&dx| (0..4).map(move |j| (dx, 0.0 + 0.08 * j as f64))).collect();
        let (img, face) = scene(800, 600, 300.0, &dots);
        assert!(
            detect(&img, &[face], None, &Options::default()).is_empty(),
            "more than {MAX_CANDIDATES_PER_FACE} candidates means stubble or freckles"
        );
    }

    #[test]
    fn body_skin_is_searched_only_when_asked() {
        // a face at the top, a shoulder of skin below it with two spots
        let (w, h) = (800, 1000);
        let (mut img, mut face) = scene_at(w, h, 400.0, 250.0, 240.0, &[]);
        for y in 650..1000 {
            for x in 100..700 {
                img.data[y * w + x] = [222, 180, 160, 255];
            }
        }
        spot(&mut img, 250.0, 850.0, 4.0);
        spot(&mut img, 550.0, 800.0, 4.0);
        let skin = skin_of(&img);
        assert!(detect(&img, std::slice::from_ref(&face), Some(&skin), &Options::default()).is_empty());
        let found = detect(&img, std::slice::from_ref(&face), Some(&skin), &Options { body: true, ..Options::default() });
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found.iter().all(|b| b.body && b.face == 0 && b.y > 0.75));
        // no landmarks: no face to measure a body by
        face.mesh = None;
        assert!(detect(&img, &[face], Some(&skin), &Options { body: true, ..Options::default() }).is_empty());
    }

    #[test]
    fn hostile_faces_do_not_panic() {
        let (img, _) = scene(64, 48, 20.0, &[]);
        let weird =
            FaceBox { x0: -2.0, y0: 0.5, x1: 9.0, y1: f64::NAN, keypoints: [(f64::INFINITY, 0.0); 4], mesh: Some(vec![(f64::NAN, 1e300); 478]) };
        let inverted = FaceBox { x0: 0.9, y0: 0.9, x1: 0.1, y1: 0.1, keypoints: [(0.5, 0.5); 4], mesh: Some(vec![(0.5, 0.5); 10]) };
        let huge = FaceBox { x0: 0.0, y0: 0.0, x1: 1.0, y1: 1.0, keypoints: [(0.5, 0.5); 4], mesh: Some(vec![(1e9, -1e9); 478]) };
        let opts = Options { body: true, sensitivity: 100.0, max_per_face: 20 };
        assert!(detect(&img, &[weird, inverted.clone(), huge], Some(&Plane::new(64, 48)), &opts).is_empty());
        assert!(detect(&Rgba8::new(0, 0), &[inverted], None, &opts).is_empty());
        // a skin plane of the wrong size is ignored, not indexed
        let (img, face) = scene(800, 600, 300.0, &[(-0.28, 0.08)]);
        assert_eq!(detect(&img, &[face], Some(&Plane::new(3, 3)), &Options::default()).len(), 1);
        assert!(skin_plane(&SegMask { side: 4, data: "bad".into(), rect: None }, 10, 10).is_none());
        assert_eq!(hull(vec![(0.0, 0.0)]).len(), 1);
        let square = hull(vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.5, 0.5)]);
        assert_eq!(square.len(), 4, "{square:?}");
    }
}
