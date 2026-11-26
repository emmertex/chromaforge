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

/// A candidate right inside the margin of an active area that touches the frame's edge: the checks around it read
/// further out than the margin (the isolation guard's ring is 4 pixels wide), and must skip what is outside the frame
/// or the area instead of forming an index past it.
#[test]
fn candidates_at_the_edge_of_the_frame_never_panic() {
    let layouts: [(Option<Cfa>, usize); 5] = [(Cfa::bayer("RGGB"), 1), (Cfa::bayer("GBRG"), 1), (Some(Cfa::xtrans()), 1), (None, 3), (None, 1)];
    for (cfa, cpp) in layouts {
        for inset in [0usize, 1, 2] {
            let (w, h) = (48 + 2 * inset, 36 + 2 * inset);
            let mut raw = mosaic(w, h, cfa.clone(), cpp, 0.002, sky);
            raw.active_area = Rect::new(inset, inset, w - 2 * inset, h - 2 * inset);
            let (lo, hix, hiy) = (inset + MARGIN, w - inset - MARGIN - 1, h - inset - MARGIN - 1);
            let corners = [(lo, lo), (hix, lo), (lo, hiy), (hix, hiy), (lo, h / 2), (w / 2, lo)];
            for &(x, y) in &corners {
                set(&mut raw, x, y, 0, 1.0);
            }
            let found = detect(&raw, &Thresholds::LEARN);
            let _ = detect(&raw, &Thresholds::CORRECT);
            let _ = refine(&raw, &found, &Thresholds::CORRECT);
            let seeds: Vec<Fix> = corners.iter().map(|&(x, y)| Fix { x: x as u32, y: y as u32, s: 0 }).collect();
            let _ = grow(&raw, &seeds);
            let edge = [Fix { x: 0, y: 0, s: 0 }, Fix { x: u32::MAX, y: u32::MAX, s: 0 }, Fix { x: w as u32, y: 0, s: 0 }];
            let _ = correct(&mut raw, &[seeds.as_slice(), &edge].concat());
        }
    }
    // the smallest area the checks take: one pixel past twice the margin each way
    let mut raw = mosaic(2 * MARGIN + 2, 2 * MARGIN + 2, Cfa::bayer("RGGB"), 1, 0.0, sky);
    set(&mut raw, MARGIN, MARGIN, 0, 1.0);
    let _ = detect(&raw, &Thresholds::LEARN);
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

/// The per-sample loop `detect` used for every layout before its fast loop: the oracle for it.
fn detect_plain(raw: &RawImage, t: &Thresholds) -> Detection {
    let Some(view) = View::new(raw) else { return Detection::default() };
    let noise = view.noise();
    let a = view.area;
    let (y0, y1) = (a.y + MARGIN, a.y + a.height - MARGIN);
    let (x0, x1) = (a.x + MARGIN, a.x + a.width - MARGIN);
    let black = raw.black.mean();
    let floor_raw: Vec<f32> = view.scale.iter().map(|s| t.floor / s).collect();
    let mut found: Vec<Defect> = Vec::new();
    let mut buf = Vec::with_capacity(8);
    for y in y0..y1 {
        for x in x0..x1 {
            let near = view.offsets(x, y);
            for s in 0..view.cpp {
                let v = view.raw_at(x, y, s);
                let (mut mx, mut mn) = (f32::MIN, f32::MAX);
                for &(dx, dy) in near.iter().take(4) {
                    let Some((nx, ny)) = view.step(x, y, dx, dy) else { continue };
                    let n = view.raw_at(nx, ny, s);
                    mx = mx.max(n);
                    mn = mn.min(n);
                }
                let fr = floor_raw.get(s).copied().unwrap_or(0.0);
                let bright = v > mx + fr;
                let dark = v - black < 0.5 * (mn - black) && mn - black > 0.0;
                if (bright || dark)
                    && let Some(d) = view.judge(x, y, s, t, &noise, &mut buf)
                {
                    found.push(d);
                }
            }
        }
    }
    if found.len() > t.max {
        return Detection { defects: Vec::new(), noise, overflow: true };
    }
    Detection { defects: found, noise, overflow: false }
}

/// Defects of every kind and size on a loose grid over `raw` (far enough apart for the context guard to leave them alone),
/// at sizes either side of the floor and of the learning and correcting thresholds; some come as a side-by-side pair. Deterministic.
fn sprinkle(raw: &mut RawImage, seed: u64) {
    let mut n = Noise(seed);
    let mut r = move || ((n.next() * 1e4).abs() as usize) ^ ((n.next() * 1e6).abs() as usize);
    let (w, h) = (raw.width, raw.height);
    for gy in (8..h.saturating_sub(8)).step_by(17) {
        for gx in (8..w.saturating_sub(8)).step_by(17) {
            let (x, y) = (gx + r() % 5, gy + r() % 5);
            if x + 1 >= w || y >= h {
                continue;
            }
            let here = get(raw, x, y, 0);
            // (the sizes within a few noise sigmas of the floor are the ones that decide on the margin of each test)
            let near_floor = here + 0.004 + 0.016 * (r() % 1000) as f32 / 1000.0;
            let v = match r() % 8 {
                0..=3 => near_floor,
                4 => here + 0.1,
                5 => 1.0,
                6 => 0.0,
                _ => here * 0.05,
            };
            set(raw, x, y, 0, v);
            if r() % 6 == 0 {
                set(raw, x + 1, y, 0, v);
            }
        }
    }
}

#[test]
fn the_fast_detect_loop_matches_the_per_sample_loop() {
    let layouts = [Cfa::bayer("RGGB"), Cfa::bayer("BGGR"), Cfa::bayer("GRBG"), Some(Cfa::xtrans())];
    let scenes: [(&str, fn(f32, f32) -> [f32; 3], f32); 4] = [
        ("sky", sky, 0.004),
        ("dark", |_, _| [0.01, 0.012, 0.015], 0.002),
        ("bright", |x, _| [0.6 + 0.0004 * x, 0.7, 0.55], 0.006),
        ("black", |_, _| [0.0, 0.0, 0.0], 0.0),
    ];
    // black levels: uniform; a 2 × 2 pattern whose phases differ by hundreds of counts; with row and column deltas
    let blacks = [
        BlackLevel::uniform(BLACK),
        BlackLevel {
            repeat_rows: 2,
            repeat_cols: 2,
            values: vec![BLACK - 300.0, BLACK + 120.0, BLACK + 200.0, BLACK - 90.0],
            delta_h: vec![],
            delta_v: vec![],
        },
        BlackLevel {
            repeat_rows: 1,
            repeat_cols: 1,
            values: vec![BLACK],
            delta_h: (0..80).map(|i| (i % 7) as f32 * 20.0 - 60.0).collect(),
            delta_v: (0..70).map(|i| (i % 5) as f32 * 30.0 - 60.0).collect(),
        },
    ];
    let mut compared = 0;
    for (li, cfa) in layouts.into_iter().flatten().enumerate() {
        for (name, scene, sigma) in scenes {
            for (bi, black) in blacks.iter().enumerate() {
                for (area_i, area) in [Rect::new(0, 0, 80, 70), Rect::new(7, 5, 61, 53)].into_iter().enumerate() {
                    let mut raw = mosaic(80, 70, Some(cfa.clone()), 1, sigma, scene);
                    raw.active_area = area;
                    raw.black = black.clone();
                    for round in 0..4u64 {
                        sprinkle(&mut raw, 11 + li as u64 * 7 + bi as u64 + round * 101);
                        for (tn, t) in [("LEARN", &Thresholds::LEARN), ("CORRECT", &Thresholds::CORRECT)] {
                            let want = detect_plain(&raw, t);
                            let got = detect(&raw, t);
                            assert_eq!(got, want, "layout {li}, scene {name}, black {bi}, area {area_i}, round {round}, {tn}");
                            compared += want.defects.len();
                        }
                    }
                }
            }
        }
    }
    // the comparison only means something if defects were found
    assert!(compared > 300, "only {compared} defects compared");
}

#[test]
fn detect_on_other_layouts_still_matches_the_per_sample_loop() {
    // float data and three samples a photosite take the general loop; with a CFA and u16 data but a window of the mosaic
    // too narrow for the fast loop's phases, too
    let mut f32_raw = mosaic(70, 60, Cfa::bayer("RGGB"), 1, 0.004, sky);
    f32_raw.data = match &f32_raw.data {
        RawData::U16(d) => RawData::F32(d.iter().map(|&v| f32::from(v)).collect()),
        other => other.clone(),
    };
    sprinkle(&mut f32_raw, 3);
    let mut rgb = mosaic(70, 60, None, 3, 0.004, sky);
    sprinkle(&mut rgb, 5);
    let mut narrow = mosaic(9, 60, Cfa::bayer("RGGB"), 1, 0.004, sky);
    sprinkle(&mut narrow, 9);
    for (name, raw) in [("f32", &f32_raw), ("three samples", &rgb), ("narrow", &narrow)] {
        for t in [&Thresholds::LEARN, &Thresholds::CORRECT] {
            assert_eq!(detect(raw, t), detect_plain(raw, t), "{name}");
        }
    }
}

/// Black levels that differ across positions by about as much as the detection floor: where the stored-count tests of the
/// fast loop need their slack. Many small random photos, judged both ways.
#[test]
fn the_fast_detect_loop_matches_when_the_black_level_varies_with_position() {
    let mut n = Noise(99);
    let mut rnd = move |amp: f32| n.next() * amp;
    let mut compared = 0;
    for round in 0..400u64 {
        let cfa = if round % 5 == 4 { Cfa::xtrans() } else { Cfa::bayer(["RGGB", "BGGR", "GRBG", "GBRG"][(round % 4) as usize]).expect("bayer") };
        let level = 0.01 + 0.002 * (round % 40) as f32;
        let mut raw = mosaic(56, 56, Some(cfa), 1, 0.0015, |_, _| [level, level * 1.1, level * 0.9]);
        raw.black = BlackLevel {
            repeat_rows: 2,
            repeat_cols: 2,
            values: (0..4).map(|_| BLACK + rnd(60.0)).collect(),
            delta_h: (0..56).map(|_| rnd(60.0)).collect(),
            delta_v: (0..56).map(|_| rnd(60.0)).collect(),
        };
        sprinkle(&mut raw, round * 13 + 1);
        for t in [&Thresholds::LEARN, &Thresholds::CORRECT] {
            let want = detect_plain(&raw, t);
            assert_eq!(detect(&raw, t), want, "round {round}");
            compared += want.defects.len();
        }
    }
    assert!(compared > 300, "only {compared} defects compared");
}

/// Rewrites `raw`'s samples so that each sits `black.at(..)` above what `mosaic` put at the uniform black level.
fn with_position_black(raw: &mut RawImage, black: BlackLevel) {
    let (w, cpp) = (raw.width, raw.cpp);
    if let RawData::U16(d) = &mut raw.data {
        for (i, v) in d.iter_mut().enumerate() {
            let (x, y) = ((i / cpp) % w, (i / cpp) / w);
            *v = (f32::from(*v) - BLACK + black.at(x, y, i % cpp, cpp)).round().clamp(0.0, 65535.0) as u16;
        }
    }
    raw.black = black;
}

/// A hot sample whose brightest same-colour neighbour is a diagonal one that sits on a higher black level (stored counts
/// higher, normalised value the same): the tighter stored-count test must keep it within the slack the black spread gives.
fn diagonal_black_scene(delta_h: Vec<f32>, delta_v: Vec<f32>, x: usize, y: usize, e: f32) -> RawImage {
    let mut raw = mosaic(80, 70, Cfa::bayer("RGGB"), 1, 0.0, |_, _| [0.002, 0.002, 0.002]);
    let black = BlackLevel { repeat_rows: 1, repeat_cols: 1, values: vec![BLACK], delta_h, delta_v };
    with_position_black(&mut raw, black);
    // the candidate: e counts above its flat surroundings, on its own black
    let i = y * raw.width + x;
    if let RawData::U16(d) = &mut raw.data {
        d[i] += e as u16;
    }
    raw
}

#[test]
fn spread_slack_keeps_a_defect_whose_brightest_neighbour_is_on_a_higher_black() {
    // column x-2 and row y-2 are 100 counts higher: the diagonal (-2,-2) is 200 above its value, the orthogonal ones 100
    let (x, y) = (40usize, 30usize);
    let mut dh = vec![0.0f32; 80];
    let mut dv = vec![0.0f32; 70];
    dh[x - 2] = 100.0;
    dv[y - 2] = 100.0;
    let raw = diagonal_black_scene(dh, dv, x, y, 280.0);
    let t = Thresholds::LEARN;
    let want = detect_plain(&raw, &t);
    assert!(has(&want, x, y).is_some(), "the oracle must see the defect: {want:?}");
    assert_eq!(detect(&raw, &t), want);
}

/// F01: BlackLevel::at adds 0 past the end of delta_h / delta_v, but black_spread only looks at the stored entries.
#[test]
fn a_black_delta_shorter_than_the_image_is_not_a_slack_of_zero() {
    let (x, y) = (40usize, 30usize);
    // equal entries (a spread of 0 over what is stored), ending just before the candidate's own column / row
    let dh = vec![100.0f32; x - 1];
    let dv = vec![100.0f32; y - 1];
    let raw = diagonal_black_scene(dh, dv, x, y, 280.0);
    let t = Thresholds::LEARN;
    let want = detect_plain(&raw, &t);
    assert!(has(&want, x, y).is_some(), "the oracle must see the defect: {want:?}");
    assert_eq!(detect(&raw, &t), want);
}

/// A flat `level` counts above black everywhere (RGGB, no noise), on the position-dependent `black`, with the sample at
/// `(x, y)` replaced by `cand` counts above its own black.
fn black_scene(black: BlackLevel, level: u16, x: usize, y: usize, cand: u16) -> RawImage {
    let mut raw = mosaic(80, 70, Cfa::bayer("RGGB"), 1, 0.0, |_, _| [f32::from(level) / (WHITE - BLACK); 3]);
    with_position_black(&mut raw, black);
    let own = raw.black.at(x, y, 0, 1);
    let i = y * raw.width + x;
    if let RawData::U16(d) = &mut raw.data {
        d[i] = (own + f32::from(cand)).round().clamp(0.0, 65535.0) as u16;
    }
    raw
}

fn delta_black(dh: Vec<f32>, dv: Vec<f32>) -> BlackLevel {
    BlackLevel { repeat_rows: 1, repeat_cols: 1, values: vec![BLACK], delta_h: dh, delta_v: dv }
}

/// `detect` (with the tighter pre-filter) and `detect_plain` (without it) agree on a scene, and the scene does hold `want`.
#[track_caller]
fn assert_prefilter_is_transparent(raw: &RawImage, t: &Thresholds, at: (usize, usize), want: Option<DefectKind>) {
    let plain = detect_plain(raw, t);
    assert_eq!(has(&plain, at.0, at.1), want, "the oracle must see what the scene holds: {plain:?}");
    assert_eq!(detect(raw, t), plain);
}

/// The first whole count of excess `judge` accepts over a flat dark field: `excess > floor` in normalised values, and the
/// noise term (1e-5 on a noiseless field) and `ratio * median` (0 at black) are far below the floor.
fn first_hot_count(floor: f32) -> u16 {
    (floor * (WHITE - BLACK)).floor() as u16 + 1
}

/// T03/T18: a hot sample exactly at, one count below and one count above the floor, on a flat dark field. `judge` takes
/// `excess > floor`; the pre-filter's `0.999 * floor` margin must keep everything `judge` takes, at either threshold set.
#[test]
fn a_hot_sample_at_the_floor_boundary_is_judged_as_the_unfiltered_loop_judges_it() {
    for t in [&Thresholds::LEARN, &Thresholds::CORRECT] {
        let first = first_hot_count(t.floor);
        for e in first - 2..=first + 2 {
            let mut raw = mosaic(40, 40, Cfa::bayer("RGGB"), 1, 0.0, |_, _| [0.0; 3]);
            let i = 20 * raw.width + 20;
            if let RawData::U16(d) = &mut raw.data {
                d[i] += e;
            }
            let want = (e >= first).then_some(DefectKind::Hot);
            assert_prefilter_is_transparent(&raw, t, (20, 20), want);
        }
    }
}

/// T04: a dead sample whose neighbours' median is just above `dead_min`, at depths from a deep dip to the shallowest
/// `judge` takes (`v < dead_ratio * median`) and past it: `lo - v` stays above `0.5 * dead_min` only because the median is
/// above it, so a bound of `0.9 * dead_min` or `dead_min` would drop the shallower dips `judge` takes.
#[test]
fn a_dead_sample_beside_neighbours_just_above_the_dead_minimum_is_judged_as_the_unfiltered_loop_judges_it() {
    let t = &Thresholds::LEARN;
    let level = (t.dead_min * (WHITE - BLACK)).ceil() as u16 + 6; // the neighbours' median, a few counts above dead_min
    let shallowest = (t.dead_ratio * f32::from(level)).ceil() as u16; // v < dead_ratio * median: counts of dip height at which it stops
    let mut found = 0;
    for dip in [0, 40, 100, 160, shallowest - 3, shallowest - 1, shallowest, shallowest + 1, shallowest + 40] {
        let raw = black_scene(BlackLevel::uniform(BLACK), level, 40, 30, dip);
        let plain = detect_plain(&raw, t);
        assert_eq!(detect(&raw, t), plain, "dip {dip}");
        found += usize::from(has(&plain, 40, 30) == Some(DefectKind::Dead));
    }
    // (the sweep must contain dips `judge` takes, or it compares nothing)
    assert!(found >= 5, "only {found} dead samples seen");
}

/// T04, the `- spread` term of the dead bound: the darkest neighbour sits on a lower black (stored counts lower, the
/// normalised value the same), which shrinks `lo - v` in stored counts by up to the spread.
#[test]
fn a_dead_sample_whose_darkest_neighbour_is_on_a_lower_black_is_kept() {
    let (x, y) = (40usize, 30usize);
    let mut dh = vec![0.0f32; 80];
    let mut dv = vec![0.0f32; 70];
    dh[x - 2] = -150.0;
    dv[y - 2] = -150.0;
    let raw = black_scene(delta_black(dh, dv), 800, x, y, 160);
    assert_prefilter_is_transparent(&raw, &Thresholds::LEARN, (x, y), Some(DefectKind::Dead));
}

/// T07: a `BlackLevel` with a non-finite entry, a `values` table shorter than its repeat pattern or empty, and a white level
/// at or under the black one (scale falls back to 1): the pre-filter must let through everything `detect_plain` takes.
#[test]
fn odd_black_and_white_levels_do_not_make_the_prefilter_drop_defects() {
    let nan_cols = {
        let mut dh = vec![0.0f32; 80];
        dh[10] = f32::NAN;
        dh
    };
    let inf_rows = {
        let mut dv = vec![0.0f32; 70];
        dv[12] = f32::INFINITY;
        dv
    };
    let blacks = [
        (
            "nan in values",
            BlackLevel { repeat_rows: 2, repeat_cols: 2, values: vec![BLACK, f32::NAN, BLACK, BLACK], delta_h: vec![], delta_v: vec![] },
        ),
        ("nan in delta_h", delta_black(nan_cols, vec![])),
        ("inf in delta_v", delta_black(vec![], inf_rows)),
        (
            "values shorter than the repeat",
            BlackLevel { repeat_rows: 2, repeat_cols: 2, values: vec![BLACK, BLACK + 80.0], delta_h: vec![], delta_v: vec![] },
        ),
        ("empty values", BlackLevel { repeat_rows: 1, repeat_cols: 1, values: vec![], delta_h: vec![30.0; 20], delta_v: vec![] }),
    ];
    for (name, black) in blacks {
        let mut raw = mosaic(80, 70, Cfa::bayer("RGGB"), 1, 0.002, sky);
        raw.black = black;
        for round in 0..3 {
            sprinkle(&mut raw, 41 + round);
            for t in [&Thresholds::LEARN, &Thresholds::CORRECT] {
                assert_eq!(detect(&raw, t), detect_plain(&raw, t), "{name}, round {round}");
            }
        }
    }
    for white in [BLACK, BLACK - 100.0, 0.0] {
        let mut raw = mosaic(80, 70, Cfa::bayer("RGGB"), 1, 0.002, sky);
        raw.white = vec![white];
        sprinkle(&mut raw, 77);
        for t in [&Thresholds::LEARN, &Thresholds::CORRECT] {
            assert_eq!(detect(&raw, t), detect_plain(&raw, t), "white {white}");
        }
    }
}

/// F01/T07: `black_spread` over exactly the entries `BlackLevel::at` can read for an area, a missing one counting as 0.
#[test]
fn black_spread_covers_what_the_black_level_can_read_in_the_area() {
    let area = Rect::new(0, 0, 10, 8);
    let spread = |b: &BlackLevel| black_spread(b, area);
    let with = |values: Vec<f32>, dh: Vec<f32>, dv: Vec<f32>| BlackLevel { repeat_rows: 2, repeat_cols: 2, values, delta_h: dh, delta_v: dv };
    // nothing varies
    assert_eq!(spread(&BlackLevel::uniform(BLACK)), 0.0);
    assert_eq!(spread(&with(vec![], vec![], vec![])), 0.0);
    // the repeat pattern, the column and the row deltas add
    assert_eq!(spread(&with(vec![10.0, 25.0, 12.0, 20.0], vec![], vec![])), 15.0);
    assert_eq!(spread(&with(vec![], vec![0.0, 5.0, 3.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], vec![])), 5.0);
    assert_eq!(spread(&with(vec![10.0, 25.0], vec![-4.0; 10], vec![1.0, 3.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0])), 15.0 + 0.0 + 2.0);
    // a table shorter than the area: the positions past its end read 0, so a table of equal non-zero entries is a spread
    assert_eq!(spread(&with(vec![10.0], vec![100.0; 9], vec![])), 100.0);
    assert_eq!(spread(&with(vec![10.0], vec![], vec![-30.0; 7])), 30.0);
    assert_eq!(spread(&with(vec![10.0], vec![-7.0; 3], vec![])), 7.0);
    // ... and one of exactly the area's length does not
    assert_eq!(spread(&with(vec![10.0], vec![100.0; 10], vec![100.0; 8])), 0.0);
    // entries past the area are never read
    let mut long = vec![0.0f32; 10];
    long.extend([1000.0, -1000.0]);
    assert_eq!(spread(&with(vec![10.0], long.clone(), long)), 0.0);
    // a non-finite entry anywhere (read or not) is an infinite spread: min/max would otherwise swallow it
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut long = vec![0.0f32; 12];
        long[11] = bad;
        assert_eq!(spread(&with(vec![10.0, bad], vec![], vec![])), f32::INFINITY, "{bad} in values");
        assert_eq!(spread(&with(vec![10.0], vec![0.0, bad], vec![])), f32::INFINITY, "{bad} in delta_h");
        assert_eq!(spread(&with(vec![10.0], vec![], vec![bad])), f32::INFINITY, "{bad} in delta_v");
        assert_eq!(spread(&with(vec![10.0], long, vec![])), f32::INFINITY, "{bad} past the area");
    }
    // finite entries whose spread overflows
    assert_eq!(spread(&with(vec![f32::MAX, -f32::MAX], vec![], vec![])), f32::INFINITY);
}
