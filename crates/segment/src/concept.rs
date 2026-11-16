//! Remembered objects ("concepts"): the photographer selects a thing on a few photos (a wedding
//! dress, the groom's suit, the bouquet) and names it; the app finds it on other photos. All
//! local and few-shot, on SAM 2.1's image encoder (Apache-2.0):
//!
//! 1. [`learn`]: the encoder's 64 × 64 × 256 embedding is pooled into prototypes — up to
//!    [`POSITIVES`] inside the selection, [`NEGATIVES`] outside it (k-means on unit vectors) — plus
//!    the selection's mean colour and spread. One [`Example`] per taught photo.
//! 2. [`score_map`]: on a new photo every cell scores "closest positive minus closest negative"
//!    (cosine), plus a small colour term (the best over the examples, so a white and a black dress
//!    can both be taught).
//! 3. [`find`]: the best cells become SAM 2.1 clicks; the mask SAM draws from them is kept only
//!    where the score map agrees (dilated), so it doesn't spill into the person wearing the dress.
//!
//! SAM's encoder features are trained for segmentation, not recognition: they match appearance
//! (white satin with folds, a dark tailored jacket) rather than "this exact dress".
//! Own design.

use serde::{Deserialize, Serialize};

use crate::{Error, Result, Sam2, Sam2Encoded};

/// Prototypes kept from inside a taught selection.
pub const POSITIVES: usize = 8;
/// Prototypes kept from outside it.
pub const NEGATIVES: usize = 16;
/// Examples a concept keeps (the oldest go first).
pub const MAX_EXAMPLES: usize = 12;
/// Below this best cell score nothing is found.
pub const FOUND: f32 = 0.15;

/// What one taught photo contributed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Example {
    /// Unit vectors (`dim` each), base64 f32 little-endian.
    #[serde(with = "b64")]
    pub positives: Vec<Vec<f32>>,
    #[serde(with = "b64")]
    pub negatives: Vec<Vec<f32>>,
    /// Mean colour inside the selection (opponent L, a, b) and its spread.
    pub color: [f32; 3],
    pub spread: [f32; 3],
    /// The selection's share of the photo.
    pub area: f32,
}

/// Base64 (standard alphabet) of f32 vectors, one string per vector.
mod b64 {
    use serde::{Deserialize, Deserializer, Serializer};

    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn encode(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for c in bytes.chunks(3) {
            let b = [c.first().copied().unwrap_or(0), c.get(1).copied().unwrap_or(0), c.get(2).copied().unwrap_or(0)];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            for (i, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
                if i <= c.len() {
                    out.push(char::from(A.get(((n >> shift) & 63) as usize).copied().unwrap_or(b'A')));
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    pub fn decode(s: &str) -> Option<Vec<u8>> {
        let val = |c: u8| A.iter().position(|a| *a == c).map(|p| p as u32);
        let s = s.trim_end_matches('=').as_bytes();
        let mut out = Vec::with_capacity(s.len() * 3 / 4);
        for c in s.chunks(4) {
            let mut n = 0u32;
            for (i, ch) in c.iter().enumerate() {
                n |= val(*ch)? << (18 - 6 * i as u32);
            }
            let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
            out.extend(bytes.iter().take(c.len().saturating_sub(1)));
        }
        Some(out)
    }

    pub fn serialize<S: Serializer>(v: &[Vec<f32>], s: S) -> Result<S::Ok, S::Error> {
        let strings: Vec<String> = v.iter().map(|x| encode(&x.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>())).collect();
        serde::Serialize::serialize(&strings, s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Vec<f32>>, D::Error> {
        let strings: Vec<String> = Vec::deserialize(d)?;
        // (a damaged vector is dropped, not an error: the concept still works with the rest)
        Ok(strings
            .iter()
            .take(64)
            .filter_map(|s| decode(s))
            .filter(|b| b.len() % 4 == 0 && b.len() <= 4 * 4096)
            .map(|b| b.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect())
            .collect())
    }
}

fn opponent(p: &[u8]) -> [f32; 3] {
    let (r, g, b) = (
        f32::from(p.first().copied().unwrap_or(0)) / 255.0,
        f32::from(p.get(1).copied().unwrap_or(0)) / 255.0,
        f32::from(p.get(2).copied().unwrap_or(0)) / 255.0,
    );
    [(r + g + b) / 3.0, r - g, (r + g) / 2.0 - b]
}

/// The mean colour of each of the `side²` cells of the (stretched) image.
fn cell_colors(rgb: &[u8], w: usize, h: usize, side: usize) -> Vec<[f32; 3]> {
    let mut out = vec![[0f32; 3]; side * side];
    if w == 0 || h == 0 || side == 0 {
        return out;
    }
    for (k, c) in out.iter_mut().enumerate() {
        let (cx, cy) = (k % side, k / side);
        let (x0, x1) = (cx * w / side, ((cx + 1) * w / side).max(cx * w / side + 1).min(w));
        let (y0, y1) = (cy * h / side, ((cy + 1) * h / side).max(cy * h / side + 1).min(h));
        let mut acc = [0f32; 4];
        // a few samples per cell are plenty for a mean
        let (sx, sy) = (((x1 - x0) / 4).max(1), ((y1 - y0) / 4).max(1));
        for y in (y0..y1).step_by(sy) {
            for x in (x0..x1).step_by(sx) {
                if let Some(p) = rgb.get((y * w + x) * 3..(y * w + x) * 3 + 3) {
                    let o = opponent(p);
                    acc = [acc[0] + o[0], acc[1] + o[1], acc[2] + o[2], acc[3] + 1.0];
                }
            }
        }
        if acc[3] > 0.0 {
            *c = [acc[0] / acc[3], acc[1] / acc[3], acc[2] / acc[3]];
        }
    }
    out
}

fn unit(v: &[f32]) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n.is_finite() && n > 1e-12 { v.iter().map(|x| x / n).collect() } else { vec![0.0; v.len()] }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Spherical k-means: up to `k` unit prototypes of `vectors` (farthest-point start, 8 rounds).
fn prototypes(vectors: &[Vec<f32>], k: usize) -> Vec<Vec<f32>> {
    let Some(first) = vectors.first() else { return Vec::new() };
    let mut centres = vec![first.clone()];
    while centres.len() < k.min(vectors.len()) {
        let far = vectors
            .iter()
            .map(|v| (v, centres.iter().map(|c| dot(v, c)).fold(f32::MIN, f32::max)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(v, _)| v.clone());
        match far {
            Some(v) if !centres.contains(&v) => centres.push(v),
            _ => break,
        }
    }
    let dim = first.len();
    for _ in 0..8 {
        let mut sums = vec![vec![0f32; dim]; centres.len()];
        for v in vectors {
            let best = centres.iter().enumerate().map(|(i, c)| (i, dot(v, c))).max_by(|a, b| a.1.total_cmp(&b.1)).map(|b| b.0);
            if let Some(s) = best.and_then(|i| sums.get_mut(i)) {
                s.iter_mut().zip(v).for_each(|(a, b)| *a += b);
            }
        }
        for (c, s) in centres.iter_mut().zip(&sums) {
            if s.iter().any(|x| *x != 0.0) {
                *c = unit(s);
            }
        }
    }
    centres
}

/// Every `stride`-th cell's unit vector where `keep(cell)` holds.
fn cell_vectors(emb: &[f32], side: usize, dim: usize, keep: impl Fn(usize) -> bool, max: usize) -> Vec<Vec<f32>> {
    let cells: Vec<usize> = (0..side * side).filter(|k| keep(*k)).collect();
    let stride = cells.len().div_ceil(max.max(1)).max(1);
    cells.iter().step_by(stride).filter_map(|k| emb.get(k * dim..(k + 1) * dim)).map(unit).collect()
}

/// Learn an example from a photo's embedding (`side² × dim`, channel-last) and the taught
/// selection: `alpha(u, v)` its strength at normalized photo point (u, v); the photo's 8-bit RGB
/// (`w × h`) for colours. `None` when the selection is (nearly) empty or the embedding is damaged.
pub fn learn(emb: &[f32], side: usize, dim: usize, alpha: impl Fn(f32, f32) -> f32, rgb: &[u8], w: usize, h: usize) -> Option<Example> {
    if side == 0 || dim == 0 || emb.len() != side * side * dim {
        return None;
    }
    let a: Vec<f32> = (0..side * side).map(|k| alpha(((k % side) as f32 + 0.5) / side as f32, ((k / side) as f32 + 0.5) / side as f32)).collect();
    let inside = |k: usize| a.get(k).is_some_and(|v| *v > 0.5);
    let outside = |k: usize| a.get(k).is_some_and(|v| *v < 0.1);
    let n_in = (0..side * side).filter(|k| inside(*k)).count();
    if n_in < 3 {
        return None;
    }
    let pos = prototypes(&cell_vectors(emb, side, dim, inside, 1024), POSITIVES);
    let neg = prototypes(&cell_vectors(emb, side, dim, outside, 2048), NEGATIVES);
    let colors = cell_colors(rgb, w, h, side);
    let ins: Vec<[f32; 3]> = (0..side * side).filter(|k| inside(*k)).filter_map(|k| colors.get(k).copied()).collect();
    let n = ins.len().max(1) as f32;
    let mean = ins.iter().fold([0f32; 3], |m, c| [m[0] + c[0] / n, m[1] + c[1] / n, m[2] + c[2] / n]);
    let var = ins
        .iter()
        .fold([0f32; 3], |m, c| [m[0] + (c[0] - mean[0]).powi(2) / n, m[1] + (c[1] - mean[1]).powi(2) / n, m[2] + (c[2] - mean[2]).powi(2) / n]);
    Some(Example { positives: pos, negatives: neg, color: mean, spread: var.map(f32::sqrt), area: n_in as f32 / (side * side) as f32 })
}

/// How much each cell of a photo's embedding looks like the concept (about −1..1; above
/// [`FOUND`]: likely it).
pub fn score_map(emb: &[f32], side: usize, dim: usize, examples: &[Example], rgb: &[u8], w: usize, h: usize) -> Vec<f32> {
    let cells = side * side;
    if dim == 0 || emb.len() != cells * dim {
        return vec![-1.0; cells];
    }
    if !examples.iter().any(|e| e.positives.iter().any(|p| p.len() == dim)) {
        return vec![-1.0; cells];
    }
    let colors = cell_colors(rgb, w, h, side);
    (0..cells)
        .map(|k| {
            let Some(v) = emb.get(k * dim..(k + 1) * dim) else { return -1.0 };
            let v = unit(v);
            let c = colors.get(k).copied().unwrap_or_default();
            // each example on its own terms: like what it taught, unlike what was around it there
            // (a black dress's example knows the groom's black suit was not it), its colours
            examples
                .iter()
                .filter(|e| e.positives.iter().any(|p| p.len() == dim))
                .map(|e| {
                    let p = e.positives.iter().filter(|q| q.len() == dim).map(|q| dot(&v, q)).fold(f32::MIN, f32::max);
                    let n = e.negatives.iter().filter(|q| q.len() == dim).map(|q| dot(&v, q)).fold(0.0f32, f32::max);
                    let d: f32 = (0..3).map(|i| ((c[i] - e.color[i]) / (2.0 * e.spread[i].max(0.04))).powi(2)).sum();
                    p - n + 0.15 * ((-d / 3.0).exp() - 0.5)
                })
                .fold(-1.0f32, f32::max)
        })
        .collect()
}

/// Up to `k` peak cells of `score` (at least `min_gap` cells apart, each at least 70 % of the best
/// and above `threshold`), as normalized points.
pub fn seeds(score: &[f32], side: usize, threshold: f32, k: usize, min_gap: usize) -> Vec<(f32, f32)> {
    let mut order: Vec<usize> = (0..score.len().min(side * side)).collect();
    order.sort_by(|a, b| score.get(*b).copied().unwrap_or(f32::MIN).total_cmp(&score.get(*a).copied().unwrap_or(f32::MIN)));
    let Some(best) = order.first().and_then(|i| score.get(*i)).copied() else { return Vec::new() };
    if best < threshold {
        return Vec::new();
    }
    let mut picked: Vec<usize> = Vec::new();
    for i in order {
        let s = score.get(i).copied().unwrap_or(f32::MIN);
        if picked.len() >= k || s < threshold.max(0.7 * best) {
            break;
        }
        let (x, y) = (i % side, i / side);
        if picked.iter().all(|p| (p % side).abs_diff(x).max((p / side).abs_diff(y)) >= min_gap) {
            picked.push(i);
        }
    }
    picked.iter().map(|i| (((i % side) as f32 + 0.5) / side as f32, ((i / side) as f32 + 0.5) / side as f32)).collect()
}

/// `score` (`side²`) dilated by `r` cells and sampled at normalized `(u, v)` (nearest).
fn dilated_at(dil: &[f32], side: usize, u: f32, v: f32) -> f32 {
    let x = ((u * side as f32) as usize).min(side.saturating_sub(1));
    let y = ((v * side as f32) as usize).min(side.saturating_sub(1));
    dil.get(y * side + x).copied().unwrap_or(-1.0)
}

fn dilate(score: &[f32], side: usize, r: usize) -> Vec<f32> {
    (0..side * side)
        .map(|k| {
            let (x, y) = (k % side, k / side);
            let mut m = f32::MIN;
            for yy in y.saturating_sub(r)..=(y + r).min(side.saturating_sub(1)) {
                for xx in x.saturating_sub(r)..=(x + r).min(side.saturating_sub(1)) {
                    m = m.max(score.get(yy * side + xx).copied().unwrap_or(f32::MIN));
                }
            }
            m
        })
        .collect()
}

/// What [`find`] found: the logits (`mask_side²` over the stretched photo) and how sure it is.
pub struct Found {
    pub logits: Vec<f32>,
    pub mask_side: usize,
    /// The best cell's score.
    pub score: f32,
    pub seeds: Vec<(f32, f32)>,
}

/// Find the concept on an encoded photo (`rgb`, `w × h` is the image SAM 2.1 encoded). `None`
/// when nothing scores above [`FOUND`].
pub fn find(sam: &Sam2, enc: &Sam2Encoded, examples: &[Example], rgb: &[u8], w: usize, h: usize) -> Result<Option<Found>> {
    if examples.is_empty() {
        return Err(Error::Message("the object has no examples yet".into()));
    }
    let (emb, side, dim) = enc.embedding()?;
    let score = score_map(&emb, side, dim, examples, rgb, w, h);
    let best = score.iter().copied().fold(f32::MIN, f32::max);
    let picks = seeds(&score, side, FOUND, 3, (side / 10).max(2));
    if picks.is_empty() {
        return Ok(None);
    }
    let clicks: Vec<crate::Click> = picks.iter().map(|(x, y)| crate::Click { x: *x, y: *y, positive: true }).collect();
    let p = sam.segment_clicks(enc, &clicks)?;
    let mask_side = sam.mask_side();
    if p.logits.len() != mask_side * mask_side {
        return Err(Error::Message("SAM 2.1 gave an unexpected mask size".into()));
    }
    // keep SAM's mask only where the concept's map agrees (dilated by two cells)
    let dil = dilate(&score, side, 2);
    let gate = (0.5 * best).max(FOUND * 0.5);
    let logits = p
        .logits
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let (u, v) = (((i % mask_side) as f32 + 0.5) / mask_side as f32, ((i / mask_side) as f32 + 0.5) / mask_side as f32);
            if dilated_at(&dil, side, u, v) < gate { l.min(-4.0) } else { *l }
        })
        .collect::<Vec<f32>>();
    if logits.iter().all(|l| *l <= 0.0) {
        return Ok(None);
    }
    // SAM selects whole things: when what it drew is mostly not like the concept (it grew from a
    // stray seed into a person or a wall), it isn't the concept
    let (mut inside, mut n) = (0.0f32, 0usize);
    for (i, l) in logits.iter().enumerate() {
        if *l > 0.0 {
            let (u, v) = (((i % mask_side) as f32 + 0.5) / mask_side as f32, ((i / mask_side) as f32 + 0.5) / mask_side as f32);
            inside += dilated_at(&score, side, u, v);
            n += 1;
        }
    }
    if n == 0 || inside / (n as f32) < 0.45 * best {
        return Ok(None);
    }
    Ok(Some(Found { logits, mask_side, score: best, seeds: picks }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 16 × 16 embedding of 4 channels: "dress" cells point one way, the rest another.
    fn emb(dress: impl Fn(usize, usize) -> bool) -> Vec<f32> {
        let side = 16;
        (0..side * side).flat_map(|k| if dress(k % side, k / side) { [1.0, 0.1, 0.0, 0.0] } else { [0.0, 0.0, 1.0, 0.2] }).collect()
    }

    #[test]
    fn a_taught_object_is_found_elsewhere_and_absent_objects_are_not() {
        let side = 16;
        let taught = emb(|x, y| (4..8).contains(&x) && (4..12).contains(&y));
        let alpha = |u: f32, v: f32| if (0.25..0.5).contains(&u) && (0.25..0.75).contains(&v) { 1.0 } else { 0.0 };
        let ex = learn(&taught, side, 4, alpha, &[], 0, 0).unwrap();
        assert!(!ex.positives.is_empty() && !ex.negatives.is_empty());
        assert!((ex.area - 32.0 / 256.0).abs() < 1e-6);
        // the same thing elsewhere on another photo
        let other = emb(|x, y| (10..14).contains(&x) && (2..6).contains(&y));
        let s = score_map(&other, side, 4, std::slice::from_ref(&ex), &[], 0, 0);
        let picks = seeds(&s, side, FOUND, 3, 2);
        assert!(!picks.is_empty());
        assert!(picks.iter().all(|(u, v)| (0.6..0.9).contains(u) && (0.1..0.4).contains(v)), "{picks:?}");
        // a photo without it
        let none = emb(|_, _| false);
        assert!(seeds(&score_map(&none, side, 4, std::slice::from_ref(&ex), &[], 0, 0), side, FOUND, 3, 2).is_empty());
        // round trip through JSON (base64 vectors)
        let json = serde_json::to_string(&ex).unwrap();
        let back: Example = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ex);
    }

    #[test]
    fn hostile_input_is_harmless() {
        assert!(learn(&[1.0; 10], 16, 4, |_, _| 1.0, &[], 0, 0).is_none(), "wrong embedding size");
        assert!(learn(&emb(|_, _| true), 16, 4, |_, _| 0.0, &[], 0, 0).is_none(), "empty selection");
        assert_eq!(score_map(&[0.0; 3], 16, 4, &[], &[], 0, 0).len(), 256);
        assert!(seeds(&[], 16, 0.1, 3, 2).is_empty());
        let bad: std::result::Result<Example, _> =
            serde_json::from_str(r#"{"positives":["!!!", "AAAA"],"negatives":[],"color":[0,0,0],"spread":[0,0,0],"area":0}"#);
        assert!(bad.is_ok_and(|e| e.positives.len() <= 1));
        assert_eq!(b64::decode(&b64::encode(b"hello")).unwrap(), b"hello");
        assert_eq!(b64::decode(&b64::encode(b"hi")).unwrap(), b"hi");
    }
}
