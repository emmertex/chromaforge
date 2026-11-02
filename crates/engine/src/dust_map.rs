//! Remove Dust: the dust spots of one photo, from its own pixels and from its camera body's dust map, turned into
//! heal spots tagged *auto dust* — and what the photo teaches the body's map.
//!
//! The photo is analysed as rendered uncropped (≈1600 px, without its auto-dust spots: they'd hide the dust) by
//! `lcc_pipeline::dust`. Positions on it are turned into sensor positions (the developed area before the
//! file's and the user's rotation; crops never move spots, which live in the uncropped frame) to be matched with
//! the body's map, and back. Each spot is healed, or evened out (a flat-field lift that keeps the texture inside)
//! when that leaves it closer to its surroundings — the usual case for large, faint shadows at wide apertures.

use lcc_develop::{AutoSpot, AutoSpotKind, Spot, SpotMode};
use lcc_geom::{Orientation, Point};
use lcc_pipeline::dust::{DustField, DustSpot};
use lcc_raster::{Rgb32f, Rgba8};

use crate::sensor_maps::{self, DustCheck, DustSeen, Shot};

/// Detections less sure than this are not added.
pub const MIN_CONFIDENCE: f32 = 0.55;
/// The long edge the photo is analysed at.
pub const EDGE: usize = 1600;

/// Where a photo sits on its sensor, and what it is.
#[derive(Clone, Debug, Default)]
pub struct PhotoContext {
    /// The camera body (`None`: the photo alone, no map).
    pub body: Option<String>,
    pub model: String,
    pub sensor: [usize; 2],
    /// The file's orientation and the user's on top of it.
    pub exif: Orientation,
    pub user: Orientation,
    /// The developed area (before rotation), for the map's aspect.
    pub developed: [usize; 2],
    pub shot: Shot,
}

impl PhotoContext {
    /// A position on the photo (0..1 of the uncropped, rotated frame) on the sensor.
    pub fn to_sensor(&self, p: (f64, f64)) -> (f64, f64) {
        let (x, y) = self.user.inverse().map(p.0, p.1, 1.0, 1.0);
        self.exif.inverse().map(x, y, 1.0, 1.0)
    }

    /// A sensor position on the photo.
    pub fn to_photo(&self, p: (f64, f64)) -> (f64, f64) {
        let (x, y) = self.exif.map(p.0, p.1, 1.0, 1.0);
        self.user.map(x, y, 1.0, 1.0)
    }
}

/// What [`analyse`] found.
#[derive(Clone, Debug, Default)]
pub struct PhotoDust {
    /// The spots to add (each tagged auto dust).
    pub spots: Vec<Spot>,
    /// Found on this photo alone, and how many of those the body's map has.
    pub found: usize,
    pub confirmed: usize,
    /// Added from the body's map though not found here (the place is smooth, the spot faint).
    pub from_map: usize,
    /// Map spots left alone because the place is textured (subject detail).
    pub skipped: usize,
    /// The photo was added to the body's dust map.
    pub learnt: bool,
}

/// The image as linear RGB (for the spot tools).
fn linear(img: &Rgba8) -> Rgb32f {
    let lin = |v: u8| lcc_color::transfer::srgb_to_linear(f32::from(v) / 255.0);
    Rgb32f { width: img.width, height: img.height, data: img.data.iter().map(|p| [lin(p[0]), lin(p[1]), lin(p[2])]).collect() }
}

/// Heal or Even Out for a spot at `c` (pixels) of radius `r` (pixels): the one whose result is closer to the ring
/// around it (mean and spread), trying both on a window of `img`. Heal also gets its source offset (pixels).
fn choose(img: &Rgb32f, c: (f32, f32), r: f32) -> (SpotMode, Option<(f32, f32)>) {
    let src = lcc_pipeline::spots::ranked_sources(img, c, r).first().copied();
    let pad = (7.0 * r).ceil() as usize;
    let (cx, cy) = (c.0.max(0.0) as usize, c.1.max(0.0) as usize);
    let (x0, y0) = (cx.saturating_sub(pad), cy.saturating_sub(pad));
    let (x1, y1) = ((cx + pad).min(img.width), (cy + pad).min(img.height));
    if x1 <= x0 + 4 || y1 <= y0 + 4 {
        return (SpotMode::Heal, src);
    }
    let (w, h) = (x1 - x0, y1 - y0);
    let window = Rgb32f::from_fn(w, h, |x, y| img.get(x0 + x, y0 + y));
    let long = w.max(h) as f64;
    let frame = lcc_pipeline::geometry::Frame::new(w, h, &lcc_develop::DevelopSettings::default(), true);
    let at = Point::new((f64::from(c.0) - x0 as f64) / w as f64, (f64::from(c.1) - y0 as f64) / h as f64);
    let score = |mode: SpotMode, offset: Option<(f32, f32)>| {
        let mut out = window.clone();
        let spot = Spot {
            mode,
            points: vec![at],
            size: f64::from(r) / long,
            source_offset: offset.map(|o| Point::new(f64::from(o.0) / w as f64, f64::from(o.1) / h as f64)),
            ..Default::default()
        };
        lcc_pipeline::spots::apply(&mut out, &[spot], &frame, frame.px_per_long(w));
        // luminance statistics inside the spot and on the ring around it
        let (mut a, mut b) = (Vec::new(), Vec::new());
        let (sx, sy) = (f64::from(c.0) - x0 as f64, f64::from(c.1) - y0 as f64);
        for y in 0..h {
            for x in 0..w {
                let d = ((x as f64 + 0.5 - sx).powi(2) + (y as f64 + 0.5 - sy).powi(2)).sqrt() / f64::from(r);
                let p = out.get(x, y);
                let l = 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
                if d < 0.7 {
                    a.push(l);
                } else if (1.3..2.5).contains(&d) {
                    b.push(l);
                }
            }
        }
        let stats = |v: &[f32]| {
            let n = v.len().max(1) as f32;
            let m = v.iter().sum::<f32>() / n;
            (m, (v.iter().map(|x| (x - m).powi(2)).sum::<f32>() / n).sqrt())
        };
        let ((ma, sa), (mb, sb)) = (stats(&a), stats(&b));
        (ma - mb).abs() / mb.max(1e-4) + (sa - sb).abs() / mb.max(1e-4)
    };
    let heal = score(SpotMode::Heal, src);
    let flat = score(SpotMode::Flat, None);
    // Even Out keeps what is under the shadow (dividing by its transmission is what removes dust); Heal copies other
    // pixels in, which can bring a pebble or a ripple along: it is taken only when Even Out is clearly worse
    if flat <= heal * 1.5 { (SpotMode::Flat, None) } else { (SpotMode::Heal, src) }
}

/// The spot for a dust shadow at `(x, y)` (0..1 of the photo) of radius `radius` (fraction of the long edge).
fn make_spot(img: &Rgb32f, x: f64, y: f64, radius: f64, auto: AutoSpot) -> Spot {
    let (w, h) = (img.width as f64, img.height as f64);
    let long = w.max(h);
    let r = (radius * long).max(1.0) as f32;
    let (mode, offset) = choose(img, ((x * w) as f32, (y * h) as f32), r);
    Spot {
        mode,
        points: vec![Point::new(x, y)],
        size: radius.clamp(crate::cmd::SPOT_SIZE.0, crate::cmd::SPOT_SIZE.1),
        feather: 60.0,
        opacity: 100.0,
        source_offset: offset.map(|o| Point::new(f64::from(o.0) / w, f64::from(o.1) / h)),
        generated: None,
        auto: Some(auto),
    }
}

/// The dust of one photo (`img`: rendered uncropped, without auto-dust spots). With `use_map`, the body's map adds
/// its spots; with `learn`, the photo is added to the map (once: shots are remembered).
pub fn analyse(img: &Rgba8, ctx: &PhotoContext, sensitivity: f32, use_map: bool, learn: bool) -> PhotoDust {
    let mut out = PhotoDust::default();
    let Some(field) = DustField::new(img) else { return out };
    let lin = linear(img);
    let found: Vec<DustSpot> = field.detect(sensitivity).into_iter().filter(|d| d.confidence >= MIN_CONFIDENCE).collect();
    out.found = found.len();
    let body = ctx.body.as_deref().filter(|_| ctx.sensor != [0, 0]);
    // what the photo teaches the map
    if learn && let Some(b) = body {
        let seen: Vec<DustSeen> = found
            .iter()
            .map(|d| {
                let (x, y) = ctx.to_sensor((d.x, d.y));
                DustSeen { x, y, radius: d.radius, strength: d.strength, dip: d.dip }
            })
            .collect();
        let check = |x: f64, y: f64, r: f64| {
            let (px, py) = ctx.to_photo((x, y));
            field.probe(px, py, r).map(|p| DustCheck { dip: p.dip, testable: p.testable })
        };
        let aspect = if ctx.developed[1] > 0 { ctx.developed[0] as f64 / ctx.developed[1] as f64 } else { 1.5 };
        let r = sensor_maps::update(b, "", &ctx.model, ctx.sensor, |m| {
            if !(m.aspect.is_finite() && m.aspect > 0.0) {
                m.aspect = aspect;
            }
            m.record_dust(&ctx.shot, &seen, check)
        });
        out.learnt = r.unwrap_or(false);
    }
    // the map's spots for this photo's time and aperture
    let mut map_spots: Vec<(f64, f64, f64, f32, bool)> = Vec::new();
    if use_map
        && let Some(b) = body
        && let Some(m) = sensor_maps::get(b).filter(|m| m.fits(&ctx.model, ctx.sensor))
    {
        map_spots = m
            .dust_for(ctx.shot.time(), ctx.shot.f_number)
            .into_iter()
            .map(|(x, y, r, strength, confirmed)| {
                let (px, py) = ctx.to_photo((x, y));
                (px, py, r, strength, confirmed)
            })
            .collect();
    }
    let long = img.width.max(img.height) as f64;
    let (w, h) = (img.width as f64, img.height as f64);
    let near =
        |a: (f64, f64), b: (f64, f64), r: f64| (((a.0 - b.0) * w).powi(2) + ((a.1 - b.1) * h).powi(2)).sqrt() <= (1.5 * r * long).max(0.004 * long);
    let mut used = vec![false; map_spots.len()];
    for d in &found {
        let matched = map_spots.iter().position(|m| near((d.x, d.y), (m.0, m.1), d.radius.max(m.2)));
        let confirmed = matched.and_then(|i| map_spots.get(i)).is_some_and(|m| m.4);
        if let Some(u) = matched.and_then(|i| used.get_mut(i)) {
            *u = true;
        }
        out.confirmed += usize::from(confirmed);
        let confidence = if confirmed { d.confidence.max(0.9) } else { d.confidence };
        out.spots.push(make_spot(&lin, d.x, d.y, d.radius, AutoSpot { kind: AutoSpotKind::Dust, confirmed, confidence }));
    }
    for (m, u) in map_spots.iter().zip(&used) {
        if *u {
            continue;
        }
        let (x, y, r, strength, confirmed) = *m;
        let Some(p) = field.probe(x, y, r) else { continue };
        // never on subject detail; a spot only seen twice must show here
        if !p.testable {
            out.skipped += 1;
            continue;
        }
        if !confirmed && p.dark < 0.4 * strength {
            continue;
        }
        out.from_map += 1;
        let confidence = if p.dark >= 0.4 * strength { 0.8 } else { 0.6 };
        out.spots.push(make_spot(&lin, x, y, r, AutoSpot { kind: AutoSpotKind::Dust, confirmed, confidence }));
    }
    out
}

/// Settings without their auto-dust spots (the ones a user moved or resized are theirs, untagged, and stay).
pub fn without_auto_dust(s: &lcc_develop::DevelopSettings) -> lcc_develop::DevelopSettings {
    let mut s = s.clone();
    s.spots.retain(|sp| !sp.auto.is_some_and(|a| a.kind == AutoSpotKind::Dust));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensor_and_photo_positions_map_both_ways_under_rotation() {
        for exif in [Orientation::Normal, Orientation::Rotate90, Orientation::Rotate270, Orientation::Rotate180, Orientation::FlipH] {
            for user in [Orientation::Normal, Orientation::Rotate90, Orientation::Transpose] {
                let ctx = PhotoContext { exif, user, ..Default::default() };
                let p = (0.23, 0.71);
                let s = ctx.to_sensor(p);
                let back = ctx.to_photo(s);
                assert!((back.0 - p.0).abs() < 1e-12 && (back.1 - p.1).abs() < 1e-12, "{exif:?} {user:?}");
            }
        }
        // a portrait shot (the camera turned clockwise: the file says Rotate90) puts the sensor's top-left corner at
        // the photo's top-right
        let ctx = PhotoContext { exif: Orientation::Rotate90, ..Default::default() };
        let (x, y) = ctx.to_photo((0.0, 0.0));
        assert!((x - 1.0).abs() < 1e-12 && y.abs() < 1e-12);
        // and the user's Rotate Left on top undoes it
        let ctx = PhotoContext { exif: Orientation::Rotate90, user: Orientation::Rotate270, ..Default::default() };
        let (x, y) = ctx.to_photo((0.2, 0.3));
        assert!((x - 0.2).abs() < 1e-12 && (y - 0.3).abs() < 1e-12);
    }
}
