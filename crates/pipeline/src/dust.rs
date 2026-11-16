//! Dust-spot detection: sensor dust shows as small, soft, round shadows on smooth areas (sky, walls, sand, a
//! backdrop). A dust particle on the sensor's cover glass casts a shadow that *multiplies* the light under it by
//! the same amount in every colour: so the detector works on log luminance (a shadow is a dip there whatever the
//! brightness) and checks every candidate's colour.
//!
//! 1. **Candidates:** a difference of Gaussians on log luminance at six blob sizes (1.6 to 12 px of sigma at 1600
//!    px, scaled with the image) finds round dips; the local maxima over the threshold are kept.
//! 2. **Measured** on linear RGB: the mean in a disc (1.3 sigma) against the ring around it (2–3.5 sigma), per
//!    colour: the dip (`1 − disc / ring`) and the ring's texture (its luminance spread over its mean).
//! 3. **Kept** when the dip is between the sensitivity's minimum and 40 %, *neutral* (each colour's dip within a
//!    quarter of the mean dip — a pebble, a shell, a mole or a shadow is coloured differently from what's around it,
//!    a dust shadow is not), the ring *smooth* (texture under 40 % of the dip) and bright enough to see a shadow in.
//! 4. **Alone and round:** another dip within 6 sigma (debris on sand, pebbles, footprints come in fields) or an
//!    elongated darkening (a streak, a crease) rules it out.
//! 5. The same spot found at several sizes counts once (the size whose response is strongest).
//!
//! [`DustField`] keeps the analysis of one image so a camera body's known dust positions can be checked on it too
//! ([`DustField::probe`]): is the spot there, and is the place smooth enough to say?

use lcc_raster::{Plane, Rgba8, blur::gaussian};

/// A found spot: centre (0..1 of the image's width / height) and radius (fraction of the long
/// edge), with how much darker it is (0..1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DustSpot {
    pub x: f64,
    pub y: f64,
    pub radius: f64,
    /// How much light it takes away at its centre, relative (`1 − transmission`): the same as `dip`.
    pub strength: f32,
    /// The darkening relative to the surroundings (1 − transmission), mean over the colours.
    pub dip: f32,
    /// The surroundings' texture over the darkening (0 = perfectly smooth; kept below 0.4).
    pub texture: f32,
    /// 0..1: how clearly it reads as dust (smooth surroundings, neutral, a clear dip).
    pub confidence: f32,
}

/// One image as linear RGB and log luminance, at the image's size.
pub struct DustField {
    w: usize,
    h: usize,
    rgb: [Plane; 3],
    log_l: Plane,
    /// Blob sizes (sigma, pixels) at this image size.
    sigmas: Vec<f32>,
}

/// What a known dust position looks like on one image ([`DustField::probe`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Probe {
    /// Darkening at the position relative to its surroundings (0..1), mean over the colours.
    pub dip: f32,
    /// The same (kept for callers comparing with [`DustSpot::strength`]).
    pub dark: f32,
    /// The place is smooth and bright enough that dust there would show (the dip can be trusted either way).
    pub testable: bool,
    /// The surroundings' luminance spread over their mean.
    pub texture: f32,
}

/// The disc / ring measurement of a candidate.
#[derive(Clone, Copy, Debug)]
struct Measure {
    dip: f32,
    /// The largest difference between one colour's dip and the mean.
    chroma: f32,
    texture: f32,
    /// The ring's mean luminance (linear).
    level: f32,
}

const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];
/// Blob sigmas at a 1600 px long edge.
const SIGMAS_1600: [f32; 6] = [1.6, 2.4, 3.6, 5.4, 8.0, 12.0];
/// Largest dip taken for dust (darker: an object). Dust at f/16 can take a third of the light; at the apertures most
/// photos are taken at, under a fifth.
const MAX_DIP: f32 = 0.3;

impl DustField {
    /// The analysis of `img` (`None` below 32 × 32 pixels).
    pub fn new(img: &Rgba8) -> Option<DustField> {
        let (w, h) = (img.width, img.height);
        if w < 32 || h < 32 || img.data.len() != w * h {
            return None;
        }
        let lut: Vec<f32> = (0..256).map(|v| lcc_color::transfer::srgb_to_linear(v as f32 / 255.0)).collect();
        let lin = |v: u8| lut.get(v as usize).copied().unwrap_or(0.0);
        let px = |x: usize, y: usize| img.data.get(y * w + x).copied().unwrap_or([0; 4]);
        let rgb = [0, 1, 2].map(|c| Plane::from_fn(w, h, |x, y| lin(px(x, y)[c])));
        let log_l = Plane::from_fn(w, h, |x, y| {
            let p = px(x, y);
            let l = LUMA[0] * lin(p[0]) + LUMA[1] * lin(p[1]) + LUMA[2] * lin(p[2]);
            l.max(1e-4).ln()
        });
        let k = w.max(h) as f32 / 1600.0;
        let sigmas = SIGMAS_1600.iter().map(|s| s * k).filter(|s| *s >= 0.8).collect();
        Some(DustField { w, h, rgb, log_l, sigmas })
    }

    fn at(p: &Plane, i: usize) -> f32 {
        p.data.get(i).copied().unwrap_or(0.0)
    }

    /// Disc (radius `r`) against ring (`1.55 r` to `2.7 r`) around `(cx, cy)` (pixels). `None` when the ring leaves
    /// the image or there's nothing to measure.
    fn measure(&self, cx: f32, cy: f32, r: f32) -> Option<Measure> {
        let (w, h) = (self.w as f32, self.h as f32);
        let (r0, r1) = (1.55 * r, 2.7 * r);
        if cx - r1 < 0.0 || cy - r1 < 0.0 || cx + r1 >= w || cy + r1 >= h || r.is_nan() || r <= 0.0 {
            return None;
        }
        let (x0, x1) = ((cx - r1).floor() as usize, (cx + r1).ceil() as usize);
        let (y0, y1) = ((cy - r1).floor() as usize, (cy + r1).ceil() as usize);
        let (mut disc, mut ring) = ([0f64; 3], [0f64; 3]);
        let (mut nd, mut nr) = (0usize, 0usize);
        let mut ring_l: Vec<f32> = Vec::new();
        for y in y0..=y1 {
            for x in x0..=x1 {
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                let i = y * self.w + x;
                let v = [0, 1, 2].map(|c| Self::at(&self.rgb[c], i));
                if d <= r {
                    for c in 0..3 {
                        disc[c] += f64::from(v[c]);
                    }
                    nd += 1;
                } else if d >= r0 && d <= r1 {
                    for c in 0..3 {
                        ring[c] += f64::from(v[c]);
                    }
                    ring_l.push(LUMA[0] * v[0] + LUMA[1] * v[1] + LUMA[2] * v[2]);
                    nr += 1;
                }
            }
        }
        if nd == 0 || nr < 8 {
            return None;
        }
        // (a colour clipped around the spot can't show its shadow: it is left out of the dip and the colour check)
        let dips: Vec<f32> = (0..3)
            .filter_map(|c| {
                let (a, b) = (disc[c] / nd as f64, ring[c] / nr as f64);
                (b > 1e-5 && b < 0.9).then(|| (1.0 - a / b) as f32)
            })
            .collect();
        if dips.is_empty() {
            return None;
        }
        let dip = dips.iter().sum::<f32>() / dips.len() as f32;
        let chroma = dips.iter().map(|d| (d - dip).abs()).fold(0.0, f32::max);
        let level = ring_l.iter().sum::<f32>() / nr as f32;
        // the ring's spread over its mean: the median absolute deviation (grain, fine texture) or, when larger, a
        // part of the standard deviation (a few dark specks around — debris on sand — barely move the median)
        let mut dev: Vec<f32> = ring_l.iter().map(|v| (v - level).abs()).collect();
        let std = (dev.iter().map(|d| d * d).sum::<f32>() / dev.len() as f32).sqrt();
        let mid = dev.len() / 2;
        let mad = dev.select_nth_unstable_by(mid, f32::total_cmp).1.to_owned();
        let texture = if level > 1e-5 { (1.4826 * mad).max(0.6 * std) / level } else { f32::INFINITY };
        Some(Measure { dip, chroma, texture, level })
    }

    /// How elongated the darkening around `(cx, cy)` is within `r` pixels: the ratio of the long to the short axis of
    /// its second moments (1 = round). A streak of debris or a crease is long; a dust shadow is round.
    fn elongation(&self, cx: f32, cy: f32, r: f32, level: f32) -> f32 {
        let (x0, x1) = ((cx - r).max(0.0) as usize, ((cx + r).ceil() as usize).min(self.w.saturating_sub(1)));
        let (y0, y1) = ((cy - r).max(0.0) as usize, ((cy + r).ceil() as usize).min(self.h.saturating_sub(1)));
        let (mut sw, mut sx, mut sy, mut sxx, mut syy, mut sxy) = (0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
                if (dx * dx + dy * dy).sqrt() > r {
                    continue;
                }
                let i = y * self.w + x;
                let l = LUMA[0] * Self::at(&self.rgb[0], i) + LUMA[1] * Self::at(&self.rgb[1], i) + LUMA[2] * Self::at(&self.rgb[2], i);
                let wgt = f64::from((level - l).max(0.0));
                let (dx, dy) = (f64::from(dx), f64::from(dy));
                sw += wgt;
                sx += wgt * dx;
                sy += wgt * dy;
                sxx += wgt * dx * dx;
                syy += wgt * dy * dy;
                sxy += wgt * dx * dy;
            }
        }
        if sw <= 1e-9 {
            return 1.0;
        }
        let (mx, my) = (sx / sw, sy / sw);
        let (a, b, c) = (sxx / sw - mx * mx, syy / sw - my * my, sxy / sw - mx * my);
        let (tr, det) = (a + b, a * b - c * c);
        let disc = (tr * tr / 4.0 - det).max(0.0).sqrt();
        let (l1, l2) = (tr / 2.0 + disc, (tr / 2.0 - disc).max(1e-9));
        (l1 / l2).sqrt() as f32
    }

    /// The darkening at `(x, y)` (0..1 of width / height) within `radius` (fraction of the long edge), and whether
    /// the place can tell: smooth and bright enough.
    pub fn probe(&self, x: f64, y: f64, radius: f64) -> Option<Probe> {
        if !(x.is_finite() && y.is_finite() && radius.is_finite()) || !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
            return None;
        }
        let long = self.w.max(self.h) as f64;
        // (a spot's radius covers its soft edge: the disc measuring its dip is a little smaller)
        let r = (radius * long * 0.65).clamp(1.0, 0.03 * long) as f32;
        let (cx, cy) = ((x * self.w as f64) as f32, (y * self.h as f64) as f32);
        // near an edge the ring is cut: measure as large as fits
        let fit = cx.min(cy).min(self.w as f32 - 1.0 - cx).min(self.h as f32 - 1.0 - cy) / 2.75;
        let m = self.measure(cx, cy, r.min(fit))?;
        let dip = m.dip.max(0.0);
        Some(Probe { dip, dark: dip, testable: m.level > 0.02 && m.texture < 0.05, texture: m.texture })
    }

    /// Dust spots; `sensitivity` 0..100 (higher finds fainter spots).
    pub fn detect(&self, sensitivity: f32) -> Vec<DustSpot> {
        let (w, h) = (self.w, self.h);
        let long = w.max(h) as f32;
        let min_dip = 0.045 - 0.035 * (sensitivity / 100.0).clamp(0.0, 1.0);
        let mut found: Vec<(DustSpot, f32, f32)> = Vec::new();
        for &s in &self.sigmas {
            let near = gaussian(&self.log_l, s);
            let far = gaussian(&self.log_l, 2.2 * s);
            let resp: Vec<f32> = far.data.iter().zip(&near.data).map(|(f, n)| f - n).collect();
            let at = |x: usize, y: usize| resp.get(y * w + x).copied().unwrap_or(f32::MIN);
            let edge = (3.0 * s).ceil() as usize + 1;
            // (a DoG peak is about half the disc / ring dip)
            let t = 0.4 * min_dip;
            let mut cands: Vec<(f32, usize, usize)> = Vec::new();
            for y in edge..h.saturating_sub(edge) {
                for x in edge..w.saturating_sub(edge) {
                    let v = at(x, y);
                    if v <= t {
                        continue;
                    }
                    let peak = (y - 1..=y + 1)
                        .all(|yy| (x - 1..=x + 1).all(|xx| (xx, yy) == (x, y) || at(xx, yy) < v || (at(xx, yy) == v && (yy, xx) > (y, x))));
                    if peak {
                        cands.push((v, x, y));
                    }
                }
            }
            cands.sort_by(|a, b| b.0.total_cmp(&a.0));
            // every peak, for the context check: dust is alone; debris, pebbles and footprints come in fields
            let crowd = 6.0 * s;
            let cell = crowd.max(1.0);
            let mut grid: std::collections::HashMap<(i64, i64), Vec<(usize, usize, f32)>> = std::collections::HashMap::new();
            for c in &cands {
                grid.entry(((c.1 as f32 / cell) as i64, (c.2 as f32 / cell) as i64)).or_default().push((c.1, c.2, c.0));
            }
            cands.truncate(600);
            for (v, x, y) in cands {
                let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
                let r = 1.3 * s;
                let Some(m) = self.measure(cx, cy, r) else { continue };
                let neutral = m.chroma <= 0.25 * m.dip + 0.006;
                let smooth = m.texture <= 0.4 * m.dip;
                if m.dip < min_dip || m.dip > MAX_DIP || !neutral || !smooth || m.level < 0.01 {
                    continue;
                }
                let (gx, gy) = ((x as f32 / cell) as i64, (y as f32 / cell) as i64);
                let others = (gy - 1..=gy + 1)
                    .flat_map(|yy| (gx - 1..=gx + 1).map(move |xx| (xx, yy)))
                    .filter_map(|k| grid.get(&k))
                    .flatten()
                    .filter(|(px, py, pv)| {
                        // (others at least half as strong: the faint ripples of a smooth area don't count)
                        (*px, *py) != (x, y) && *pv >= 0.5 * v && ((*px as f32 - cx).powi(2) + (*py as f32 - cy).powi(2)).sqrt() < crowd
                    })
                    .count();
                if others >= 2 || self.elongation(cx, cy, 2.0 * s, m.level * (1.0 - 0.25 * m.dip)) > 2.2 {
                    continue;
                }
                let smooth_c = (1.0 - m.texture / (0.4 * m.dip)).clamp(0.0, 1.0);
                let neutral_c = (1.0 - m.chroma / (0.25 * m.dip + 0.006)).clamp(0.0, 1.0);
                let clear = ((m.dip - min_dip) / (2.0 * min_dip.max(0.01))).clamp(0.0, 1.0);
                let spot = DustSpot {
                    x: f64::from(cx) / w as f64,
                    y: f64::from(cy) / h as f64,
                    // the heal covers the shadow's soft edge
                    radius: f64::from(2.8 * s / long),
                    strength: m.dip,
                    dip: m.dip,
                    texture: m.texture / m.dip,
                    confidence: (0.4 * smooth_c + 0.3 * neutral_c + 0.3 * clear).clamp(0.0, 1.0),
                };
                // the size it is found at is the one whose response is strongest (a difference of Gaussians with a
                // fixed ratio is scale-normalised), less on texture
                let score = v * (0.5 + 0.5 * smooth_c);
                found.push((spot, score, 2.0 * s));
            }
        }
        // one spot per place: the size that fits it best
        found.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut out: Vec<(DustSpot, f32)> = Vec::new();
        for (spot, _, r) in found {
            let (x, y) = (spot.x * w as f64, spot.y * h as f64);
            let taken = out.iter().any(|(o, or)| ((o.x * w as f64 - x).powi(2) + (o.y * h as f64 - y).powi(2)).sqrt() < f64::from(1.2 * r.max(*or)));
            if !taken {
                out.push((spot, r));
            }
        }
        let mut out: Vec<DustSpot> = out.into_iter().map(|(s, _)| s).collect();
        out.sort_by(|a, b| b.strength.total_cmp(&a.strength));
        out.truncate(60);
        out
    }
}

/// Dust spots in a rendered image; `sensitivity` 0..100 (higher finds fainter spots).
pub fn detect(img: &Rgba8, sensitivity: f32) -> Vec<DustSpot> {
    DustField::new(img).map(|f| f.detect(sensitivity)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sky gradient (`w × h`) with soft dark spots `(cx, cy, sigma px, depth)` (depth: the transmission dip, a
    /// multiplication of the light in every colour), sRGB 8-bit.
    pub(crate) fn sky_with(w: usize, h: usize, spots: &[(f32, f32, f32, f32)]) -> Rgba8 {
        let enc = |v: f32| (lcc_color::transfer::linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0).round() as u8;
        let data: Vec<[u8; 4]> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                let base = 0.25 + 0.25 * (y / h as f32) + 0.04 * (x / w as f32);
                let mut t = 1.0;
                for &(cx, cy, r, depth) in spots {
                    let d2 = (x - cx).powi(2) + (y - cy).powi(2);
                    t *= 1.0 - depth * (-d2 / (2.0 * r * r)).exp();
                }
                let v = base * t;
                [enc(v * 0.8), enc(v * 0.95), enc(v * 1.3), 255]
            })
            .collect();
        Rgba8 { width: w, height: h, data }
    }

    /// A smooth sky gradient with three soft dust shadows: all found, nothing else.
    #[test]
    fn finds_spots_on_smooth_areas() {
        let (w, h) = (600usize, 400usize);
        let spots = [(120.0f32, 90.0f32, 4.0f32), (400.0, 200.0, 6.0), (500.0, 330.0, 3.0)];
        let img = sky_with(w, h, &spots.map(|(x, y, r)| (x, y, r, 0.12)));
        let found = detect(&img, 50.0);
        assert_eq!(found.len(), 3, "{found:?}");
        for (cx, cy, _) in spots {
            assert!(
                found.iter().any(|s| (s.x * w as f64 - cx as f64).abs() < 3.0 && (s.y * h as f64 - cy as f64).abs() < 3.0),
                "{cx},{cy}: {found:?}"
            );
        }
        assert!(found.iter().all(|s| s.confidence > 0.5 && s.dip > 0.05 && s.texture < 0.4), "{found:?}");
    }

    /// Coloured dark specks (pebbles on sand, a mole, a shell) are not dust: the shadow of dust is the same in
    /// every colour.
    #[test]
    fn coloured_specks_are_not_dust() {
        let (w, h) = (600usize, 400usize);
        let mut img = sky_with(w, h, &[(300.0, 200.0, 5.0, 0.12)]);
        // a brownish speck: much darker in blue than in red
        for y in 0..h {
            for x in 0..w {
                let d2 = (x as f32 - 150.0).powi(2) + (y as f32 - 120.0).powi(2);
                let k = (-d2 / (2.0 * 16.0)).exp();
                let p = &mut img.data[y * w + x];
                p[1] = (p[1] as f32 * (1.0 - 0.15 * k)) as u8;
                p[2] = (p[2] as f32 * (1.0 - 0.35 * k)) as u8;
            }
        }
        let found = detect(&img, 50.0);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!((found[0].x * w as f64 - 300.0).abs() < 3.0);
    }

    /// The same dust at three apertures: small and dark at f/16, wider and fainter at f/5.6, a faint wide shadow
    /// at f/2.8 that the probe (what a camera body's map checks with) still measures on a smooth sky.
    #[test]
    fn dust_by_aperture() {
        let (w, h) = (800usize, 533usize);
        let f16 = detect(&sky_with(w, h, &[(300.0, 200.0, 3.0, 0.20)]), 50.0);
        let f56 = detect(&sky_with(w, h, &[(300.0, 200.0, 6.0, 0.08)]), 50.0);
        assert_eq!((f16.len(), f56.len()), (1, 1), "{f16:?} {f56:?}");
        assert!(f16[0].radius < f56[0].radius && f16[0].strength > f56[0].strength);
        let wide = sky_with(w, h, &[(300.0, 200.0, 10.0, 0.03)]);
        let field = DustField::new(&wide).unwrap();
        let p = field.probe(300.0 / w as f64, 200.0 / h as f64, 0.02).unwrap();
        assert!(p.testable && p.dip > 0.012, "{p:?}");
        // nothing at a clean place of the same image, and it can tell
        let q = field.probe(600.0 / w as f64, 400.0 / h as f64, 0.02).unwrap();
        assert!(q.testable && q.dip < p.dip / 3.0, "{q:?} vs {p:?}");
    }

    #[test]
    fn textured_scenes_give_few_false_spots() {
        for scene in lcc_scenes::demo_library().iter().take(6) {
            let img = scene.render(600, 400);
            let data: Vec<[u8; 4]> = img
                .data
                .iter()
                .map(|c| {
                    let e = |v: f32| (lcc_color::transfer::linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0) as u8;
                    [e(c[0]), e(c[1]), e(c[2]), 255]
                })
                .collect();
            let img = Rgba8 { width: 600, height: 400, data };
            let found = detect(&img, 50.0);
            assert!(found.len() <= 2, "{}: {} false spots {found:?}", scene.id, found.len());
        }
        // and probes over texture say they can't tell
        let img = lcc_scenes::demo_library()[2].render(600, 400);
        let data: Vec<[u8; 4]> =
            img.data.iter().map(|c| [0, 1, 2].map(|k| (c[k].clamp(0.0, 1.0).powf(0.45) * 255.0) as u8)).map(|p| [p[0], p[1], p[2], 255]).collect();
        let field = DustField::new(&Rgba8 { width: 600, height: 400, data }).unwrap();
        let textured = (0..20).filter_map(|k| field.probe(0.05 + 0.045 * k as f64, 0.5, 0.01)).filter(|p| !p.testable).count();
        assert!(textured > 0);
    }

    #[test]
    fn probes_outside_or_on_tiny_images_are_none() {
        assert!(DustField::new(&Rgba8 { width: 8, height: 8, data: vec![[0; 4]; 64] }).is_none());
        let f = DustField::new(&sky_with(64, 64, &[])).unwrap();
        assert!(f.probe(-0.1, 0.5, 0.01).is_none() && f.probe(0.5, f64::NAN, 0.01).is_none());
        assert!(f.probe(0.5, 0.5, 1e9).is_some());
        assert!(f.probe(0.0, 0.0, 0.01).is_none(), "no ring at a corner");
    }
}
