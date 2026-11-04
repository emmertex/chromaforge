use super::*;
use crate::{BlackLevel, ColorData, Metadata, OpcodeLists, Orientation, RawFormat};

const BLACK: f32 = 512.0;
const WHITE: f32 = 16383.0;

/// Deterministic, roughly normal noise (sum of 4 uniforms).
struct Noise(u64);
impl Noise {
    fn next(&mut self) -> f32 {
        let mut s = 0.0;
        for _ in 0..4 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            s += ((self.0 >> 40) as f32 / (1u64 << 24) as f32) - 0.5;
        }
        s * 1.732
    }
}

/// A `w × h` mosaic of `scene` (linear RGB, 0..1) through `cfa`, with noise `sigma` (normalised).
fn mosaic(w: usize, h: usize, cfa: Option<Cfa>, cpp: usize, sigma: f32, scene: impl Fn(f32, f32) -> [f32; 3]) -> RawImage {
    let mut n = Noise(7);
    let range = WHITE - BLACK;
    let mut data = Vec::with_capacity(w * h * cpp);
    for y in 0..h {
        for x in 0..w {
            let rgb = scene(x as f32 + 0.5, y as f32 + 0.5);
            for s in 0..cpp {
                let c = match (&cfa, cpp) {
                    (Some(c), _) => c.color_at(x, y) as usize,
                    (None, 3) => s,
                    _ => 1,
                };
                let v = (rgb[c] + sigma * n.next()).clamp(0.0, 1.0);
                data.push((BLACK + v * range).round() as u16);
            }
        }
    }
    RawImage {
        format: RawFormat::Dng,
        width: w,
        height: h,
        cpp,
        data: RawData::U16(data),
        cfa,
        bits: 14,
        black: BlackLevel::uniform(BLACK),
        white: vec![WHITE],
        active_area: Rect::new(0, 0, w, h),
        crop: Rect::default(),
        orientation: Orientation::Normal,
        color: ColorData::default(),
        wb_multipliers: None,
        linearized: false,
        opcodes: OpcodeLists::default(),
        metadata: Metadata::default(),
        lens_metadata: Default::default(),
        pixel_aspect: 1.0,
    }
}

fn set(raw: &mut RawImage, x: usize, y: usize, s: usize, v: f32) {
    let i = (y * raw.width + x) * raw.cpp + s;
    if let RawData::U16(d) = &mut raw.data {
        d[i] = (BLACK + v.clamp(0.0, 1.0) * (WHITE - BLACK)).round() as u16;
    }
}

fn get(raw: &RawImage, x: usize, y: usize, s: usize) -> f32 {
    (raw.data.get((y * raw.width + x) * raw.cpp + s) - BLACK) / (WHITE - BLACK)
}

/// A smooth sky-like gradient.
fn sky(x: f32, y: f32) -> [f32; 3] {
    let t = y / 200.0;
    [0.10 + 0.05 * t, 0.18 + 0.06 * t + 0.0002 * x, 0.30 + 0.04 * t]
}

fn has(d: &Detection, x: usize, y: usize) -> Option<DefectKind> {
    d.defects.iter().find(|q| q.x as usize == x && q.y as usize == y).map(|q| q.kind)
}

#[test]
fn bayer_hot_dead_and_stuck_pixels_are_found_and_corrected() {
    let truth = mosaic(160, 120, Cfa::bayer("RGGB"), 1, 0.004, sky);
    let mut raw = truth.clone();
    {
        let v = get(&raw, 40, 30, 0) + 0.25;
        set(&mut raw, 40, 30, 0, v);
    } // hot red
    {
        let v = get(&raw, 81, 50, 0) + 0.12;
        set(&mut raw, 81, 50, 0, v);
    } // mild hot green
    set(&mut raw, 101, 71, 0, 0.0); // dead blue
    set(&mut raw, 60, 91, 0, 1.0); // stuck green
    let d = detect(&raw, &Thresholds::CORRECT);
    assert_eq!(has(&d, 40, 30), Some(DefectKind::Hot), "{d:?}");
    assert_eq!(has(&d, 81, 50), Some(DefectKind::Hot), "{d:?}");
    assert_eq!(has(&d, 101, 71), Some(DefectKind::Dead), "{d:?}");
    assert_eq!(has(&d, 60, 91), Some(DefectKind::Stuck), "{d:?}");
    assert_eq!(d.defects.len(), 4, "no false positives on a smooth noisy sky: {d:?}");
    // one loose pass refined gives what the strict pass gives
    let loose = detect(&raw, &Thresholds::LEARN);
    assert!(loose.defects.len() >= 4);
    assert_eq!(refine(&raw, &loose, &Thresholds::CORRECT), d.defects);
    let fixes: Vec<Fix> = d.defects.iter().map(Fix::from).collect();
    assert_eq!(correct(&mut raw, &fixes), 4);
    for (x, y) in [(40, 30), (81, 50), (101, 71), (60, 91)] {
        assert!((get(&raw, x, y, 0) - get(&truth, x, y, 0)).abs() < 0.02, "({x}, {y}) restored");
    }
    assert!(detect(&raw, &Thresholds::CORRECT).defects.is_empty());
}

/// A star field (point sources through optics with a low-pass filter: Gaussian blur of 0.5–0.8 px) and a portrait
/// catchlight: nothing is a defect, at the learning threshold either.
#[test]
fn stars_and_catchlights_are_not_defects() {
    let stars: Vec<(f32, f32, f32, f32)> = (0..60)
        .map(|i| {
            let f = i as f32;
            let x = 8.0 + (f * 37.3) % 180.0;
            let y = 8.0 + (f * 23.7) % 130.0;
            // brightness 0.05..1.5 (bright ones clip), sharpness 0.5..0.8 px, centred anywhere in a pixel
            (x + (f * 0.37) % 1.0, y + (f * 0.61) % 1.0, 0.05 + (f * 0.173) % 1.45, 0.5 + (f * 0.071) % 0.3)
        })
        .collect();
    let night = |x: f32, y: f32| {
        let mut v = 0.01f32;
        for &(sx, sy, a, s) in &stars {
            v += a * (-((x - sx).powi(2) + (y - sy).powi(2)) / (2.0 * s * s)).exp();
        }
        [v * 0.9, v, v * 1.1]
    };
    let raw = mosaic(200, 150, Cfa::bayer("RGGB"), 1, 0.002, night);
    let d = detect(&raw, &Thresholds::CORRECT);
    assert!(d.defects.is_empty(), "stars corrected: {:?}", d.defects);
    // the learning threshold may note the very sharpest faint ones (a body map confirms a pixel only when it is
    // there in photos of different scenes); a little softer, none
    let soft = |x: f32, y: f32| {
        let mut v = 0.01f32;
        for &(sx, sy, a, s) in &stars {
            v += a * (-((x - sx).powi(2) + (y - sy).powi(2)) / (2.0 * (s + 0.1).powi(2))).exp();
        }
        [v * 0.9, v, v * 1.1]
    };
    let d = detect(&mosaic(200, 150, Cfa::bayer("RGGB"), 1, 0.002, soft), &Thresholds::LEARN);
    assert!(d.defects.is_empty(), "stars learnt: {:?}", d.defects);
    // a catchlight: a small bright disc (radius 1.5 px) on a dark iris
    let eye = |x: f32, y: f32| {
        let d = ((x - 50.3).powi(2) + (y - 40.6).powi(2)).sqrt();
        let v = if d < 1.5 {
            0.95
        } else if d < 2.3 {
            0.95 * (2.3 - d) / 0.8
        } else if d < 12.0 {
            0.03
        } else {
            0.25
        };
        [v, v * 0.95, v * 0.9]
    };
    let raw = mosaic(100, 80, Cfa::bayer("GRBG"), 1, 0.003, eye);
    assert!(detect(&raw, &Thresholds::LEARN).defects.is_empty());
    // small hard-edged highlights two to four pixels across, with a brighter core pixel (jewellery, a glint on a
    // button): their samples all stand out from the background
    let blobs = |x: f32, y: f32| {
        let mut v = 0.2f32;
        for (bx, by, rx, ry) in [(30.2f32, 30.7f32, 1.6f32, 1.1f32), (70.5, 50.5, 2.0, 1.5), (50.9, 20.1, 1.2, 1.2)] {
            let d = ((x - bx) / rx).powi(2) + ((y - by) / ry).powi(2);
            if d < 1.0 {
                v += 0.25 + 0.35 * (1.0 - d);
            }
        }
        [v, v * 0.95, v * 0.9]
    };
    let d = detect(&mosaic(100, 80, Cfa::bayer("RGGB"), 1, 0.003, blobs), &Thresholds::LEARN);
    assert!(d.defects.is_empty(), "{d:?}");
}

/// Sun glints on sand or water: a field of single-pixel sparkles of every colour. None is a defect, while a hot pixel
/// away from them still is.
#[test]
fn glint_fields_are_not_defects() {
    let mut raw = mosaic(200, 150, Cfa::bayer("RGGB"), 1, 0.003, |_, _| [0.35, 0.3, 0.25]);
    let mut n = Noise(99);
    for k in 0..60 {
        let x = 30 + ((n.next().abs() * 40.0) as usize + k * 3) % 50;
        let y = 30 + ((n.next().abs() * 40.0) as usize + k * 7) % 40;
        set(&mut raw, x, y, 0, 0.6 + 0.1 * n.next().abs());
    }
    set(&mut raw, 170, 130, 0, 0.9);
    let d = detect(&raw, &Thresholds::CORRECT);
    assert_eq!(d.defects.iter().map(|q| (q.x, q.y)).collect::<Vec<_>>(), vec![(170, 130)], "{d:?}");
}

#[test]
fn fine_texture_gives_no_false_positives() {
    // fabric-like texture at one to three pixel scales
    let cloth = |x: f32, y: f32| {
        let v = 0.3 + 0.12 * (x * 2.1).sin() * (y * 1.7).cos() + 0.06 * ((x + y) * 3.1).sin();
        [v, v * 0.9, v * 0.8]
    };
    let raw = mosaic(200, 150, Cfa::bayer("RGGB"), 1, 0.006, cloth);
    assert!(detect(&raw, &Thresholds::CORRECT).defects.is_empty());
    assert!(detect(&raw, &Thresholds::LEARN).defects.len() <= 2);
}

#[test]
fn pairs_of_adjacent_defects_are_both_found() {
    let mut raw = mosaic(100, 80, Cfa::bayer("RGGB"), 1, 0.003, sky);
    set(&mut raw, 30, 30, 0, 0.9);
    set(&mut raw, 31, 30, 0, 0.85);
    let d = detect(&raw, &Thresholds::CORRECT);
    assert!(has(&d, 30, 30).is_some() && has(&d, 31, 30).is_some(), "{d:?}");
    let fixes: Vec<Fix> = d.defects.iter().map(Fix::from).collect();
    correct(&mut raw, &fixes);
    assert!(get(&raw, 30, 30, 0) < 0.3 && get(&raw, 31, 30, 0) < 0.3);
}

#[test]
fn thresholds_follow_the_noise() {
    // ISO 12800-like noise: a +0.06 sample is ordinary there, and nothing is corrected
    let mut raw = mosaic(160, 120, Cfa::bayer("RGGB"), 1, 0.02, sky);
    {
        let v = get(&raw, 50, 50, 0) + 0.06;
        set(&mut raw, 50, 50, 0, v);
    }
    let d = detect(&raw, &Thresholds::CORRECT);
    assert!(d.defects.is_empty(), "{d:?}");
    assert!(d.noise[1] > 0.012 && d.noise[1] < 0.03, "{:?}", d.noise);
    // a hot pixel well above that noise is still found
    set(&mut raw, 50, 50, 0, 0.9);
    assert!(has(&detect(&raw, &Thresholds::CORRECT), 50, 50).is_some());
}

#[test]
fn xtrans_defects_are_found_and_stars_are_not() {
    let truth = mosaic(144, 120, Some(Cfa::xtrans()), 1, 0.004, sky);
    let mut raw = truth.clone();
    // one of each colour
    let cfa = Cfa::xtrans();
    let spot = |c: u8, from: usize| (from..from + 6).flat_map(|y| (20..26).map(move |x| (x, y))).find(|&(x, y)| cfa.color_at(x, y) == c);
    let picks: Vec<(usize, usize)> = [(0u8, 20usize), (1, 50), (2, 80)].iter().filter_map(|&(c, y)| spot(c, y)).collect();
    assert_eq!(picks.len(), 3);
    for &(x, y) in &picks {
        set(&mut raw, x, y, 0, 0.8);
    }
    let d = detect(&raw, &Thresholds::CORRECT);
    for &(x, y) in &picks {
        assert_eq!(has(&d, x, y), Some(DefectKind::Hot), "({x}, {y}) {d:?}");
    }
    assert_eq!(d.defects.len(), 3, "{d:?}");
    let fixes: Vec<Fix> = d.defects.iter().map(Fix::from).collect();
    assert_eq!(correct(&mut raw, &fixes), 3);
    for &(x, y) in &picks {
        assert!((get(&raw, x, y, 0) - get(&truth, x, y, 0)).abs() < 0.03);
    }
    let star = |x: f32, y: f32| {
        let v = 0.01 + 0.8 * (-((x - 70.4).powi(2) + (y - 60.7).powi(2)) / (2.0 * 0.6f32 * 0.6)).exp();
        [v, v, v]
    };
    assert!(detect(&mosaic(144, 120, Some(Cfa::xtrans()), 1, 0.002, star), &Thresholds::LEARN).defects.is_empty());
}

#[test]
fn linear_and_monochrome_data_work_too() {
    // three samples a pixel (a linear DNG): a stuck channel
    let mut lin = mosaic(80, 60, None, 3, 0.003, sky);
    set(&mut lin, 20, 20, 2, 1.0);
    let d = detect(&lin, &Thresholds::CORRECT);
    assert_eq!(d.defects.len(), 1, "{d:?}");
    assert_eq!((d.defects[0].x, d.defects[0].y, d.defects[0].s, d.defects[0].kind), (20, 20, 2, DefectKind::Stuck));
    let fixes: Vec<Fix> = d.defects.iter().map(Fix::from).collect();
    assert_eq!(correct(&mut lin, &fixes), 1);
    assert!(get(&lin, 20, 20, 2) < 0.4);
    // a white dot in all three channels of one pixel is not a stuck channel… but it is a hot pixel
    let mut lin = mosaic(80, 60, None, 3, 0.003, sky);
    for s in 0..3 {
        set(&mut lin, 30, 30, s, 0.9);
    }
    assert!(detect(&lin, &Thresholds::CORRECT).defects.is_empty(), "other channels lit: treated as light");
    // monochrome
    let mut mono = mosaic(80, 60, None, 1, 0.003, sky);
    set(&mut mono, 40, 30, 0, 0.9);
    let d = detect(&mono, &Thresholds::CORRECT);
    assert_eq!(d.defects.len(), 1, "{d:?}");
    // a demosaiced linear DNG with the default settings decodes as before when nothing is wrong
    let clean = mosaic(80, 60, None, 3, 0.003, sky);
    assert!(detect(&clean, &Thresholds::CORRECT).defects.is_empty());
}

#[test]
fn hostile_inputs_never_panic() {
    // tiny, empty-ish, active area at the edge, a wrong data length, NaN floats
    let mut r = mosaic(6, 6, Cfa::bayer("RGGB"), 1, 0.0, sky);
    assert!(detect(&r, &Thresholds::CORRECT).defects.is_empty());
    assert_eq!(correct(&mut r, &[Fix { x: 3, y: 3, s: 0 }, Fix { x: 99, y: 99, s: 4 }]), 0);
    let mut r = mosaic(40, 40, Cfa::bayer("RGGB"), 1, 0.0, sky);
    r.data = RawData::U16(vec![0; 10]);
    assert!(detect(&r, &Thresholds::CORRECT).defects.is_empty());
    assert_eq!(correct(&mut r, &[Fix { x: 20, y: 20, s: 0 }]), 0);
    let mut r = mosaic(40, 40, None, 1, 0.0, sky);
    r.data = RawData::F32(vec![f32::NAN; 1600]);
    let _ = detect(&r, &Thresholds::LEARN);
    let _ = correct(&mut r, &[Fix { x: 20, y: 20, s: 0 }]);
    let _ = sample_fingerprint(&r);
}

#[test]
fn dark_frames_list_hot_pixels_and_noise() {
    let mut raw = mosaic(200, 150, Cfa::bayer("RGGB"), 1, 0.0015, |_, _| [0.006, 0.006, 0.006]);
    let hot = [(10usize, 10usize, 0.03f32), (77, 41, 0.3), (150, 120, 1.0)];
    for &(x, y, v) in &hot {
        set(&mut raw, x, y, 0, v);
    }
    let df = crate::darkframe::analyze(&raw).unwrap();
    assert_eq!(df.hot.len(), 3, "{:?}", df.hot);
    assert_eq!(df.hot[0].kind, DefectKind::Stuck);
    assert!((df.noise[1] - 0.0015).abs() < 0.0006, "{:?}", df.noise);
    assert!((df.offset[1] - 0.006).abs() < 0.0006);
    // the record round-trips as JSON
    let j = serde_json::to_string(&df).unwrap();
    assert_eq!(serde_json::from_str::<crate::darkframe::DarkFrame>(&j).unwrap(), df);
    // a lit photo is refused
    assert!(crate::darkframe::analyze(&mosaic(100, 80, Cfa::bayer("RGGB"), 1, 0.003, sky)).is_err());
}

/// A cluster of three hot photosites (an older sensor): each fails the isolation guard alone, but grown from the one
/// a camera body's map knows, all three are corrected.
#[test]
fn clusters_grow_from_a_known_pixel() {
    let mut raw = mosaic(100, 80, Cfa::bayer("RGGB"), 1, 0.003, sky);
    for (x, y) in [(40usize, 30usize), (41, 30), (40, 31)] {
        set(&mut raw, x, y, 0, 0.8);
    }
    let d = detect(&raw, &Thresholds::CORRECT);
    assert!(d.defects.is_empty(), "a cluster is not a single-photo defect: {d:?}");
    let seed = [Fix { x: 40, y: 30, s: 0 }];
    let more = grow(&raw, &seed);
    assert_eq!(more.len(), 2, "{more:?}");
    let all: Vec<Fix> = seed.iter().copied().chain(more).collect();
    correct(&mut raw, &all);
    for (x, y) in [(40usize, 30usize), (41, 30), (40, 31)] {
        assert!(get(&raw, x, y, 0) < 0.4, "({x}, {y}) {}", get(&raw, x, y, 0));
    }
    // around a seed with nothing wrong next to it, nothing grows
    assert!(grow(&mosaic(100, 80, Cfa::bayer("RGGB"), 1, 0.003, sky), &seed).is_empty());
}
