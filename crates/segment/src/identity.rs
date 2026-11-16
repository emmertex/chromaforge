//! Telling people apart by how they look, with the open (Tier A) MediaPipe models only: no face
//! recognition network. Every face BlazeFace and FaceMesh find gets an appearance [`Signature`]:
//!
//! - the colours of the person's clothes, hair and face skin (Selfie Multiclass, inside the part
//!   of the photo that belongs to that face);
//! - the face's proportions (24 FaceMesh landmarks, eyes level, scaled by the eye distance);
//! - a coarse colour layout of the face box (glasses, beard, make-up).
//!
//! Within one shoot this separates the bride (white dress) from the groom (dark suit) and the
//! guests well; across shoots clothes change and it doesn't, which is why matching only ever
//! suggests and the photographer confirms. An identity embedding from a face-recognition model
//! the user installed can be attached ([`Signature::embedding`]) and then dominates the score.
//!
//! Ported from OneSlips' fork (own design there).

use serde::{Deserialize, Serialize};

use crate::people::{Face, PeopleMap};

/// FaceMesh landmarks used for the face's proportions: eye corners, brows, nose, mouth, chin, jaw,
/// forehead.
const GEOMETRY: [usize; 24] = [33, 133, 362, 263, 70, 105, 300, 334, 168, 6, 1, 2, 98, 327, 61, 291, 0, 17, 152, 234, 454, 172, 397, 10];

/// Cells of the face box's colour layout per side.
const LAYOUT: usize = 4;

/// A face's appearance (all parts optional: a profile has no landmarks, a head shot no clothes).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Signature {
    /// Landmark positions (x, y pairs) in eye-distance units around the eyes' midpoint.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub geometry: Vec<f32>,
    /// Mean colour (opponent L, a, b; 0..1-ish) of the clothes, hair and face skin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clothes: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hair: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skin: Option<[f32; 3]>,
    /// The face box's colour layout ([`LAYOUT`]² cells × a, b, relative L).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layout: Vec<f32>,
    /// An identity embedding from an optional face-recognition model (L2-normalized).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
    /// SAM 2.1 image features pooled over the person (body: their part of the photo where Selfie
    /// Multiclass sees a person; face: the face box), L2-normalized: appearance beyond colour
    /// (cut, fabric, beard, hair style).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Vec<f32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub face: Option<Vec<f32>>,
}

fn unit(v: Vec<f32>) -> Option<Vec<f32>> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    (n.is_finite() && n > 1e-9).then(|| v.into_iter().map(|x| x / n).collect())
}

/// Pool a channel-last image embedding (`side² × dim` over the stretched photo) over
/// `faces[index]`'s body and face: `(body, face)` features for [`Signature::body`] /
/// [`Signature::face`]. `w × h` are the photo's pixels the faces are in.
pub fn features(
    emb: &[f32],
    side: usize,
    dim: usize,
    faces: &[Face],
    index: usize,
    map: Option<&PeopleMap>,
    w: usize,
    h: usize,
) -> (Option<Vec<f32>>, Option<Vec<f32>>) {
    let Some(face) = faces.get(index) else { return (None, None) };
    if side == 0 || dim == 0 || emb.len() != side * side * dim || w == 0 || h == 0 {
        return (None, None);
    }
    let d = &face.detection;
    let (fx0, fy0, fx1, fy1) = (d.x0 / w as f32, d.y0 / h as f32, d.x1 / w as f32, d.y1 / h as f32);
    let (mut body, mut fac) = (vec![0f32; dim], vec![0f32; dim]);
    let (mut nb, mut nf) = (0usize, 0usize);
    for k in 0..side * side {
        let (u, v) = (((k % side) as f32 + 0.5) / side as f32, ((k / side) as f32 + 0.5) / side as f32);
        let Some(cell) = emb.get(k * dim..(k + 1) * dim) else { continue };
        let n = cell.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
        let in_face = u >= fx0 && u <= fx1 && v >= fy0 && v <= fy1;
        if in_face {
            fac.iter_mut().zip(cell).for_each(|(a, b)| *a += b / n);
            nf += 1;
        }
        // the body: below the face, the person's own part of the photo, a person per the map
        let below = v > fy1 && v < fy1 + 5.0 * (fy1 - fy0) && (u - (fx0 + fx1) / 2.0).abs() < 2.5 * (fx1 - fx0);
        let person = map.map_or(below, |m| m.at(&[1, 2, 3, 4, 5], u, v) > 0.5);
        if below && person && owner(faces, w, h, u, v) == Some(index) {
            body.iter_mut().zip(cell).for_each(|(a, b)| *a += b / n);
            nb += 1;
        }
    }
    let body = if nb >= 2 { unit(body) } else { None };
    let fac = if nf >= 1 { unit(fac) } else { None };
    (body, fac)
}

fn cosine(a: &Option<Vec<f32>>, b: &Option<Vec<f32>>) -> Option<f32> {
    match (a, b) {
        (Some(x), Some(y)) if x.len() == y.len() && !x.is_empty() => Some(x.iter().zip(y).map(|(p, q)| p * q).sum()),
        _ => None,
    }
}

/// sRGB 8-bit to a simple opponent space: lightness, red–green, yellow–blue (0..1-ish).
fn opponent(r: f32, g: f32, b: f32) -> [f32; 3] {
    let (r, g, b) = (r / 255.0, g / 255.0, b / 255.0);
    [(r + g + b) / 3.0, r - g, (r + g) / 2.0 - b]
}

fn pixel(rgb: &[u8], w: usize, h: usize, u: f32, v: f32) -> Option<[f32; 3]> {
    if w == 0 || h == 0 || !u.is_finite() || !v.is_finite() {
        return None;
    }
    let x = ((u * w as f32) as isize).clamp(0, w as isize - 1) as usize;
    let y = ((v * h as f32) as isize).clamp(0, h as isize - 1) as usize;
    let i = (y * w + x).checked_mul(3)?;
    let p = rgb.get(i..i + 3)?;
    Some(opponent(f32::from(p[0]), f32::from(p[1]), f32::from(p[2])))
}

/// Which face (index into `faces`) owns normalized point `(u, v)`: the nearest by its centre, in
/// photo proportions (the same rule People masks use).
pub fn owner(faces: &[Face], w: usize, h: usize, u: f32, v: f32) -> Option<usize> {
    if w == 0 || h == 0 {
        return None;
    }
    let aspect = w as f32 / h as f32;
    faces
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let [cx, cy] = f.detection.center();
            let (dx, dy) = ((u - cx / w as f32) * aspect, (v - cy / h as f32) * 0.6);
            (i, dx * dx + dy * dy)
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

fn geometry(lm: &[[f32; 2]]) -> Vec<f32> {
    let p = |i: usize| lm.get(i).copied();
    let (Some(a0), Some(a1), Some(b0), Some(b1)) = (p(33), p(133), p(362), p(263)) else { return Vec::new() };
    let e1 = [(a0[0] + a1[0]) / 2.0, (a0[1] + a1[1]) / 2.0];
    let e2 = [(b0[0] + b1[0]) / 2.0, (b0[1] + b1[1]) / 2.0];
    let (dx, dy) = (e2[0] - e1[0], e2[1] - e1[1]);
    let d = (dx * dx + dy * dy).sqrt();
    if !(d.is_finite() && d > 1e-3) {
        return Vec::new();
    }
    let (c, s) = (dx / d, dy / d);
    let mid = [(e1[0] + e2[0]) / 2.0, (e1[1] + e2[1]) / 2.0];
    let mut out = Vec::with_capacity(GEOMETRY.len() * 2);
    for i in GEOMETRY {
        let Some(q) = p(i) else { return Vec::new() };
        let (x, y) = (q[0] - mid[0], q[1] - mid[1]);
        // rotate so the eyes are level, scale by their distance
        out.push((x * c + y * s) / d);
        out.push((-x * s + y * c) / d);
    }
    out
}

/// The appearance signature of `faces[index]` in an 8-bit RGB photo `w × h` (the faces in photo
/// pixels, as [`crate::people::FaceModel::faces`] gives them), with the photo's Selfie Multiclass
/// map when there is one. `None` for an index out of range.
pub fn signature(faces: &[Face], index: usize, map: Option<&PeopleMap>, rgb: &[u8], w: usize, h: usize) -> Option<Signature> {
    let face = faces.get(index)?;
    if w == 0 || h == 0 {
        return None;
    }
    let det = &face.detection;
    let (fw, fh) = (det.width().max(1.0) / w as f32, det.height().max(1.0) / h as f32);
    let [cx, cy] = det.center();
    let (cx, cy) = (cx / w as f32, cy / h as f32);
    let mut sig = Signature { geometry: face.landmarks.as_deref().map(geometry).unwrap_or_default(), ..Default::default() };
    if let Some(map) = map {
        // weighted mean colour per class over the person's part of the photo, near the face
        let mut acc = [[0f32; 4]; 3]; // clothes, hair, skin: sum L, a, b, weight
        let classes = [4usize, 1, 3];
        let step = 1.0 / map.side.max(1) as f32;
        let (x0, x1) = ((cx - 2.5 * fw).max(0.0), (cx + 2.5 * fw).min(1.0));
        let (y0, y1) = ((cy - 1.5 * fh).max(0.0), (cy + 5.0 * fh).min(1.0));
        let mut v = y0 + step / 2.0;
        while v < y1 {
            let mut u = x0 + step / 2.0;
            while u < x1 {
                if owner(faces, w, h, u, v) == Some(index)
                    && let Some(px) = pixel(rgb, w, h, u, v)
                {
                    for (k, c) in classes.iter().enumerate() {
                        // clothes only below the face; hair and skin around it
                        if k == 0 && v < cy + 0.6 * fh {
                            continue;
                        }
                        let p = map.at(&[*c], u, v);
                        if p > 0.5
                            && let Some(a) = acc.get_mut(k)
                        {
                            a[0] += p * px[0];
                            a[1] += p * px[1];
                            a[2] += p * px[2];
                            a[3] += p;
                        }
                    }
                }
                u += step;
            }
            v += step;
        }
        let mean = |a: [f32; 4], min: f32| (a[3] > min).then(|| [a[0] / a[3], a[1] / a[3], a[2] / a[3]]);
        // at least a few cells' worth, so a sliver of a sleeve doesn't count as the clothes
        let cells = |n: f32| n * (map.side as f32 / 256.0).powi(2);
        sig.clothes = mean(acc[0], cells(12.0));
        sig.hair = mean(acc[1], cells(4.0));
        sig.skin = mean(acc[2], cells(4.0));
    }
    // the face box's layout: mean colour per cell, lightness relative to the face's
    let mut cells = vec![[0f32; 4]; LAYOUT * LAYOUT];
    let samples = 6;
    for (k, cell) in cells.iter_mut().enumerate() {
        let (gx, gy) = ((k % LAYOUT) as f32, (k / LAYOUT) as f32);
        for j in 0..samples {
            for i in 0..samples {
                let u = (det.x0 + (gx + (i as f32 + 0.5) / samples as f32) / LAYOUT as f32 * det.width()) / w as f32;
                let v = (det.y0 + (gy + (j as f32 + 0.5) / samples as f32) / LAYOUT as f32 * det.height()) / h as f32;
                if let Some(px) = pixel(rgb, w, h, u, v) {
                    cell[0] += px[0];
                    cell[1] += px[1];
                    cell[2] += px[2];
                    cell[3] += 1.0;
                }
            }
        }
    }
    let total_l: f32 = cells.iter().map(|c| c[0]).sum::<f32>() / cells.iter().map(|c| c[3]).sum::<f32>().max(1.0);
    if total_l > 0.0 && cells.iter().all(|c| c[3] > 0.0) {
        sig.layout = cells.iter().flat_map(|c| [c[0] / c[3] / total_l.max(1e-3) - 1.0, c[1] / c[3], c[2] / c[3]]).collect();
    }
    Some(sig)
}

fn dist3(a: [f32; 3], b: [f32; 3], l_weight: f32) -> f32 {
    (l_weight * (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn rms(a: &[f32], b: &[f32]) -> Option<f32> {
    (a.len() == b.len() && !a.is_empty()).then(|| (a.iter().zip(b).map(|(x, y)| (x - y).powi(2)).sum::<f32>() / a.len() as f32).sqrt())
}

/// How alike two signatures are, 0..1 (1: the same look). With identity embeddings on both, their
/// cosine similarity carries most of the weight.
pub fn similarity(a: &Signature, b: &Signature) -> f32 {
    // A logistic model over the cues' distances, fitted on pairs of faces from three weddings
    // (same person or not, within one shoot): (distance, weight, mean, spread). The colours of the
    // clothes and hair and SAM 2.1's features of the body carry it; face proportions, skin colour
    // and the face's own features hardly told people apart there and are left out.
    let mut z = -0.84f32;
    let mut any = false;
    let mut add = |d: Option<f32>, w: f32, mean: f32, spread: f32| {
        if let Some(d) = d.filter(|d| d.is_finite()) {
            z += w * (d - mean) / spread;
            any = true;
        }
    };
    let pair3 = |x: Option<[f32; 3]>, y: Option<[f32; 3]>, lw: f32| match (x, y) {
        (Some(x), Some(y)) => Some(dist3(x, y, lw)),
        _ => None,
    };
    add(pair3(a.clothes, b.clothes, 1.5), -0.51, 0.28, 0.256);
    add(pair3(a.hair, b.hair, 1.0), -0.378, 0.127, 0.097);
    add(cosine(&a.body, &b.body).map(|c| 1.0 - c), -0.662, 0.203, 0.084);
    add(rms(&a.layout, &b.layout), -0.096, 0.299, 0.126);
    let look = if any { 1.0 / (1.0 + (-z.clamp(-30.0, 30.0)).exp()) } else { 0.0 };
    match (&a.embedding, &b.embedding) {
        (Some(x), Some(y)) if x.len() == y.len() && !x.is_empty() => {
            let cos: f32 = x.iter().zip(y).map(|(p, q)| p * q).sum();
            // cosine 0.3..0.7 is the usual "maybe"..."same person" band of face embedders
            let id = ((cos - 0.25) / 0.45).clamp(0.0, 1.0);
            0.75 * id + 0.25 * look
        }
        _ => look,
    }
}

/// The best match for `probe` among `known` (label, signatures): `(index into known, score)`
/// when it scores at least `threshold` and leads the runner-up label by `margin`.
pub fn best_match(probe: &Signature, known: &[(String, Vec<Signature>)], threshold: f32, margin: f32) -> Option<(usize, f32)> {
    let mut scores: Vec<(usize, f32)> = known
        .iter()
        .enumerate()
        .filter_map(|(i, (_, sigs))| {
            // a person's score: the mean of their two closest examples (robust to one odd photo)
            let mut s: Vec<f32> = sigs.iter().map(|q| similarity(probe, q)).collect();
            s.sort_by(|a, b| b.total_cmp(a));
            let top: Vec<f32> = s.into_iter().take(2).collect();
            (!top.is_empty()).then(|| (i, top.iter().sum::<f32>() / top.len() as f32))
        })
        .collect();
    scores.sort_by(|a, b| b.1.total_cmp(&a.1));
    let first = *scores.first()?;
    let second = scores.get(1).map_or(0.0, |s| s.1);
    (first.1 >= threshold && first.1 - second >= margin).then_some(first)
}

/// Most faces [`cluster`] groups at once (its cost grows with the cube of the count).
pub const MAX_CLUSTERED: usize = 400;

/// Group signatures into clusters of the same look (average linkage, merging while the two closest
/// clusters' mean similarity is at least `threshold`). Returns clusters of indices, largest first.
/// At most [`MAX_CLUSTERED`] signatures are clustered (the rest are left out).
pub fn cluster(sigs: &[Signature], threshold: f32) -> Vec<Vec<usize>> {
    let n = sigs.len().min(MAX_CLUSTERED);
    let mut clusters: Vec<Vec<usize>> = (0..n).map(|i| vec![i]).collect();
    // pairwise similarities
    let sim: Vec<f32> = (0..n * n)
        .map(|k| match (sigs.get(k / n), sigs.get(k % n)) {
            _ if k / n == k % n => 1.0,
            (Some(a), Some(b)) => similarity(a, b),
            _ => 0.0,
        })
        .collect();
    let link = |a: &[usize], b: &[usize]| -> f32 {
        let mut s = 0.0;
        for i in a {
            for j in b {
                s += sim.get(i * n + j).copied().unwrap_or(0.0);
            }
        }
        s / (a.len() * b.len()).max(1) as f32
    };
    loop {
        let mut best: Option<(usize, usize, f32)> = None;
        for (i, a) in clusters.iter().enumerate() {
            for (j, b) in clusters.iter().enumerate().skip(i + 1) {
                let l = link(a, b);
                if l >= threshold && best.is_none_or(|b| l > b.2) {
                    best = Some((i, j, l));
                }
            }
        }
        let Some((i, j, _)) = best else { break };
        let moved = clusters.remove(j);
        if let Some(c) = clusters.get_mut(i) {
            c.extend(moved);
        }
    }
    for c in &mut clusters {
        c.sort_unstable();
    }
    clusters.sort_by(|a, b| b.len().cmp(&a.len()).then(a.first().cmp(&b.first())));
    clusters
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mediapipe::Detection;

    fn face(x: f32) -> Face {
        Face { detection: Detection { x0: x - 5.0, y0: 10.0, x1: x + 5.0, y1: 20.0, score: 1.0, keypoints: [[0.0; 2]; 6] }, landmarks: None }
    }

    /// 100 × 100: left half white (dress), right half navy (suit); hair dark everywhere at the top.
    fn photo() -> (Vec<u8>, PeopleMap) {
        let (w, h) = (100usize, 100usize);
        let mut rgb = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let c = if y < 10 {
                    [40, 30, 20]
                } else if x < 50 {
                    [245, 245, 240]
                } else {
                    [20, 30, 90]
                };
                rgb[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&c);
            }
        }
        let side = 64;
        let mut probs = vec![0f32; side * side * 6];
        for y in 0..side {
            for x in 0..side {
                let v = (y as f32 + 0.5) / side as f32;
                let c = if v < 0.1 {
                    1
                } else if v > 0.25 {
                    4
                } else {
                    3
                };
                probs[(y * side + x) * 6 + c] = 1.0;
            }
        }
        (rgb, PeopleMap { side, probs })
    }

    #[test]
    fn clothes_tell_the_bride_from_the_groom() {
        let (rgb, map) = photo();
        let faces = [face(25.0), face(75.0)];
        let bride = signature(&faces, 0, Some(&map), &rgb, 100, 100).unwrap();
        let groom = signature(&faces, 1, Some(&map), &rgb, 100, 100).unwrap();
        let c = bride.clothes.unwrap();
        assert!(c[0] > 0.9, "white dress: {c:?}");
        assert!(groom.clothes.unwrap()[2] < -0.1, "navy suit is blue: {:?}", groom.clothes);
        assert!(similarity(&bride, &bride) > 0.55, "{}", similarity(&bride, &bride));
        assert!(similarity(&bride, &groom) < 0.3, "{}", similarity(&bride, &groom));
        let known = vec![("Groom".to_string(), vec![groom.clone()]), ("Bride".to_string(), vec![bride.clone()])];
        assert_eq!(best_match(&bride, &known, 0.5, 0.1).map(|m| m.0), Some(1));
        let clusters = cluster(&[bride.clone(), groom.clone(), bride.clone(), groom], 0.5);
        assert_eq!(clusters, vec![vec![0, 2], vec![1, 3]]);
        assert!(signature(&faces, 5, Some(&map), &rgb, 100, 100).is_none());
    }

    #[test]
    fn hostile_input_stays_finite() {
        let faces = [face(25.0)];
        assert!(signature(&faces, 0, None, &[], 100, 100).is_some_and(|s| s.layout.is_empty()));
        assert!(signature(&faces, 0, None, &[1, 2, 3], 0, 0).is_none());
        let mut f = face(25.0);
        f.landmarks = Some(vec![[f32::NAN, 0.0]; 478]);
        let s = signature(&[f], 0, None, &[0; 300], 10, 10).unwrap();
        assert!(s.geometry.iter().all(|v| !v.is_finite()) || s.geometry.is_empty());
        assert_eq!(similarity(&Signature::default(), &Signature::default()), 0.0);
        let e = Signature { embedding: Some(vec![1.0, 0.0]), ..Default::default() };
        let other = Signature { embedding: Some(vec![1.0]), ..Default::default() };
        assert_eq!(similarity(&e, &other), 0.0, "embedding sizes differ: ignored");
        assert!(best_match(&e, &[], 0.1, 0.0).is_none());
        assert!(cluster(&[], 0.5).is_empty());
    }
}
