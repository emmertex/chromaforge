//! Synthetic quality and speed checks for content-aware completion.
//!
//! The regular tests assert that structure crossing a hole is continued and that excluded pixels are
//! never copied. `report` (ignored; run with `--ignored --nocapture`) prints hole RMSE and timings for
//! every case and, when `INPAINT_OUT` names a directory, writes `clean | input | result` PNGs there.

use std::time::Instant;

use lcc_inpaint::{CompleteParams, Guide, complete_guided};

const CH: usize = 3;

/// Deterministic hash noise in `[-1, 1)`.
fn hash(x: usize, y: usize, s: u64) -> f32 {
    let mut z = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ s;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z % 2_000_000) as f32 / 1_000_000.0 - 1.0
}

/// Smooth value noise (bilinear between hash lattice points `cell` pixels apart).
fn value_noise(x: usize, y: usize, cell: usize, s: u64) -> f32 {
    let (fx, fy) = (x as f32 / cell as f32, y as f32 / cell as f32);
    let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let (tx, ty) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
    let a = hash(x0, y0, s) + (hash(x0 + 1, y0, s) - hash(x0, y0, s)) * tx;
    let b = hash(x0, y0 + 1, s) + (hash(x0 + 1, y0 + 1, s) - hash(x0, y0 + 1, s)) * tx;
    a + (b - a) * ty
}

fn disc(w: usize, h: usize, cx: f32, cy: f32, r: f32) -> Vec<bool> {
    (0..w * h).map(|i| ((i % w) as f32 + 0.5 - cx).hypot((i / w) as f32 + 0.5 - cy) < r).collect()
}

/// Anti-aliased coverage of the half-plane `n·(p - c) > 0` at pixel `(x, y)`.
fn half_plane(x: usize, y: usize, c: (f32, f32), angle_deg: f32) -> f32 {
    let a = angle_deg.to_radians();
    let n = (-a.sin(), a.cos());
    let d = (x as f32 + 0.5 - c.0) * n.0 + (y as f32 + 0.5 - c.1) * n.1;
    (d + 0.5).clamp(0.0, 1.0)
}

/// Distance from pixel centre to the line through `c` at `angle_deg`.
fn line_dist(x: usize, y: usize, c: (f32, f32), angle_deg: f32) -> f32 {
    let a = angle_deg.to_radians();
    let n = (-a.sin(), a.cos());
    ((x as f32 + 0.5 - c.0) * n.0 + (y as f32 + 0.5 - c.1) * n.1).abs()
}

struct Case {
    name: &'static str,
    w: usize,
    h: usize,
    clean: Vec<f32>,
    hole: Vec<bool>,
    exclude: Option<Vec<bool>>,
}

fn build(w: usize, h: usize, f: impl Fn(usize, usize) -> [f32; 3]) -> Vec<f32> {
    (0..w * h).flat_map(|i| f(i % w, i / w)).collect()
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// (a) a diagonal straight edge between two flat colours.
fn case_edge(s: usize, r: f32) -> Case {
    let c = (s as f32 / 2.0, s as f32 / 2.0);
    let clean = build(s, s, |x, y| lerp3([0.55, 0.32, 0.18], [0.08, 0.16, 0.45], half_plane(x, y, (c.0 + 0.3 * r, c.1), 33.0)));
    Case { name: "a_diagonal_edge", w: s, h: s, clean, hole: disc(s, s, c.0, c.1, r), exclude: None }
}

/// (b) a thin (2 px) dark line on a sky gradient.
fn case_wire(s: usize, r: f32) -> Case {
    let c = (s as f32 / 2.0, s as f32 / 2.0);
    let clean = build(s, s, |x, y| {
        let t = y as f32 / s as f32;
        let sky = [0.25 + 0.25 * t, 0.42 + 0.2 * t, 0.85 - 0.1 * t];
        let cov = (1.5 - line_dist(x, y, (c.0, c.1 - 0.25 * r), 18.0)).clamp(0.0, 1.0);
        lerp3(sky, [0.03, 0.03, 0.035], cov)
    });
    Case { name: "b_power_line", w: s, h: s, clean, hole: disc(s, s, c.0, c.1, r), exclude: None }
}

/// (c) a slightly tilted horizon: textured sky above, rougher ground below.
fn case_horizon(s: usize, r: f32) -> Case {
    let c = (s as f32 / 2.0, s as f32 / 2.0);
    let clean = build(s, s, |x, y| {
        let sky = 0.04 * value_noise(x, y, 12, 3) + 0.01 * hash(x, y, 4);
        let sky = [0.45 + sky, 0.6 + sky, 0.9 + sky];
        let g = 0.08 * value_noise(x, y, 5, 5) + 0.04 * hash(x, y, 6);
        let ground = [0.22 + g, 0.3 + g, 0.1 + 0.5 * g];
        lerp3(sky, ground, half_plane(x, y, (c.0, c.1 + 0.1 * r), 4.0))
    });
    Case { name: "c_horizon", w: s, h: s, clean, hole: disc(s, s, c.0, c.1, r), exclude: None }
}

/// (d) vertical stripes.
fn case_stripes(s: usize, r: f32) -> Case {
    let clean = build(s, s, |x, _| if (x / 4) % 2 == 0 { [0.2; 3] } else { [0.8; 3] });
    Case { name: "d_stripes", w: s, h: s, clean, hole: disc(s, s, s as f32 / 2.0, s as f32 / 2.0, r), exclude: None }
}

/// (e) a noisy, blotchy texture (gravel / foliage stand-in).
fn case_noise(s: usize, r: f32) -> Case {
    let clean = build(s, s, |x, y| {
        let v = 0.12 * value_noise(x, y, 6, 8) + 0.06 * value_noise(x, y, 2, 9) + 0.03 * hash(x, y, 10);
        [0.35 + v, 0.3 + 0.8 * v, 0.2 + 0.5 * v]
    });
    Case { name: "e_noise", w: s, h: s, clean, hole: disc(s, s, s as f32 / 2.0, s as f32 / 2.0, r), exclude: None }
}

/// (f) grey with a bright red block covering the left half of the hole's surroundings; the block is
/// excluded, so the fill must be grey (the truth inside the hole is grey).
fn case_exclude(s: usize, r: f32) -> Case {
    let c = (s as f32 / 2.0, s as f32 / 2.0);
    // A red block whose right edge runs through the hole centre: without exclusion the fill continues
    // the red/grey edge into the hole.
    let in_sq = |x: usize, y: usize| (x as f32) < c.0 && (y as f32 - c.1).abs() < 1.6 * r && (x as f32) > c.0 - 3.0 * r;
    let grey = |x: usize, y: usize| {
        let g = 0.4 + 0.01 * hash(x, y, 11);
        [g, g, g]
    };
    let clean = build(s, s, grey); // truth: grey everywhere
    let hole = disc(s, s, c.0, c.1, r);
    let mut img_with_red = clean.clone();
    let mut exclude = vec![false; s * s];
    for y in 0..s {
        for x in 0..s {
            if in_sq(x, y) && !hole[y * s + x] {
                img_with_red[(y * s + x) * CH..(y * s + x + 1) * CH].copy_from_slice(&[0.9, 0.05, 0.05]);
                exclude[y * s + x] = true;
            }
        }
    }
    Case { name: "f_exclusion", w: s, h: s, clean: img_with_red, hole, exclude: Some(exclude) }
}

fn all_cases() -> Vec<Case> {
    vec![case_edge(200, 32.0), case_wire(200, 32.0), case_horizon(200, 32.0), case_stripes(200, 32.0), case_noise(200, 32.0), case_exclude(200, 32.0)]
}

/// The input: hole pixels painted a flat mid grey (as the real caller hands over whatever was there).
fn dirty(c: &Case) -> Vec<f32> {
    let mut d = c.clean.clone();
    for (i, h) in c.hole.iter().enumerate() {
        if *h {
            d[i * CH..(i + 1) * CH].copy_from_slice(&[0.5, 0.5, 0.5]);
        }
    }
    d
}

/// Truth inside the hole (for the exclusion case: grey, not red).
fn truth(c: &Case) -> Vec<f32> {
    if c.exclude.is_some() {
        let mut t = c.clean.clone();
        for (i, h) in c.hole.iter().enumerate() {
            if *h {
                let g = 0.4 + 0.01 * hash(i % c.w, i / c.w, 11);
                t[i * CH..(i + 1) * CH].copy_from_slice(&[g, g, g]);
            }
        }
        t
    } else {
        c.clean.clone()
    }
}

fn hole_rmse(a: &[f32], b: &[f32], hole: &[bool]) -> f32 {
    let (mut e, mut n) = (0.0f64, 0usize);
    for (i, h) in hole.iter().enumerate() {
        if *h {
            for c in 0..CH {
                e += f64::from(a[i * CH + c] - b[i * CH + c]).powi(2);
                n += 1;
            }
        }
    }
    (e / n.max(1) as f64).sqrt() as f32
}

/// Hole pixels that came out clearly red (`R - G > 0.2`).
fn red_pixels(out: &[f32], hole: &[bool]) -> usize {
    hole.iter().enumerate().filter(|(i, h)| **h && out[i * CH] - out[i * CH + 1] > 0.2).count()
}

fn run(c: &Case, img: &[f32], p: &CompleteParams) -> Vec<f32> {
    let guide = Guide { exclude: c.exclude.as_deref() };
    complete_guided(c.w, c.h, CH, img, &c.hole, &guide, p, &|| false).ok().flatten().unwrap_or_else(|| img.to_vec())
}

fn write_png(path: &std::path::Path, w: usize, h: usize, panels: &[&[f32]]) {
    let tw = w * panels.len() + 4 * (panels.len() - 1);
    let mut data = vec![255u8; tw * h * 3];
    for (k, p) in panels.iter().enumerate() {
        for y in 0..h {
            for x in 0..w {
                for c in 0..3 {
                    let v = p[(y * w + x) * CH + c].clamp(0.0, 1.0);
                    let s = if v <= 0.0031308 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
                    data[(y * tw + k * (w + 4) + x) * 3 + c] = (s * 255.0 + 0.5) as u8;
                }
            }
        }
    }
    let f = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), tw as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&data).unwrap();
}

#[test]
fn diagonal_edge_is_continued() {
    let c = case_edge(120, 20.0);
    let out = run(&c, &dirty(&c), &CompleteParams::default());
    let e = hole_rmse(&out, &truth(&c), &c.hole);
    assert!(e < 0.04, "rmse {e}");
}

#[test]
fn power_line_is_continued() {
    let c = case_wire(120, 20.0);
    let out = run(&c, &dirty(&c), &CompleteParams::default());
    // The dark line must cross the hole: along the true line, inside the hole, pixels stay dark.
    let cc = (60.0f32, 60.0 - 5.0);
    let (mut on, mut dark) = (0, 0);
    for y in 0..c.h {
        for x in 0..c.w {
            if c.hole[y * c.w + x] && line_dist(x, y, cc, 18.0) < 0.5 {
                on += 1;
                if out[(y * c.w + x) * CH + 2] < 0.4 {
                    dark += 1;
                }
            }
        }
    }
    assert!(on > 10 && dark * 10 >= on * 8, "{dark} of {on} line pixels dark");
}

#[test]
fn excluded_pixels_are_never_copied() {
    let c = case_exclude(120, 18.0);
    let input = dirty(&c);
    let out = run(&c, &input, &CompleteParams::default());
    assert_eq!(red_pixels(&out, &c.hole), 0);
    // Excluded and other known pixels are unchanged.
    for (i, h) in c.hole.iter().enumerate() {
        if !*h {
            assert_eq!(out[i * CH..(i + 1) * CH], input[i * CH..(i + 1) * CH]);
        }
    }
}

#[test]
fn exclusion_of_everything_falls_back() {
    let (w, h) = (40, 40);
    let img = vec![0.5f32; w * h * CH];
    let hole = disc(w, h, 20.0, 20.0, 6.0);
    let ex: Vec<bool> = hole.iter().map(|h| !h).collect();
    let g = Guide { exclude: Some(&ex) };
    assert_eq!(complete_guided(w, h, CH, &img, &hole, &g, &CompleteParams::default(), &|| false), Ok(None));
    let bad = vec![false; 7];
    let g = Guide { exclude: Some(&bad) };
    assert_eq!(complete_guided(w, h, CH, &img, &hole, &g, &CompleteParams::default(), &|| false), Ok(None));
}

#[test]
#[ignore = "report: prints numbers, writes PNGs to $INPAINT_OUT"]
fn report() {
    let out_dir = std::env::var_os("INPAINT_OUT").map(std::path::PathBuf::from);
    let tag = std::env::var("INPAINT_TAG").unwrap_or_else(|_| "after".into());
    let p = CompleteParams::default();
    println!("| case | input RMSE | result RMSE | red px | ms |");
    for c in all_cases() {
        let input = dirty(&c);
        let t = Instant::now();
        let out = run(&c, &input, &p);
        let ms = t.elapsed().as_secs_f64() * 1e3;
        let tr = truth(&c);
        println!(
            "| {} | {:.4} | {:.4} | {} | {:.0} |",
            c.name,
            hole_rmse(&input, &tr, &c.hole),
            hole_rmse(&out, &tr, &c.hole),
            if c.exclude.is_some() { red_pixels(&out, &c.hole) } else { 0 },
            ms
        );
        if let Some(d) = &out_dir {
            write_png(&d.join(format!("{}_{tag}.png", c.name)), c.w, c.h, &[&tr, &input, &out]);
        }
    }
    for (s, r) in [(400usize, 60.0f32), (1200, 200.0)] {
        let mut c = case_horizon(s, r);
        // Add a wire too, so the timing covers the structure pass.
        let w2 = case_wire(s, r);
        for (i, v) in c.clean.iter_mut().enumerate() {
            if w2.clean[i / CH * CH + 2] < 0.1 {
                *v = w2.clean[i];
            }
        }
        let input = dirty(&c);
        let mut out = Vec::new();
        for (label, pp) in [("structure off", CompleteParams { structure: 0.0, ..p.clone() }), ("structure on", p.clone())] {
            let mut times = Vec::new();
            for _ in 0..5 {
                let t = Instant::now();
                out = run(&c, &input, &pp);
                times.push(t.elapsed().as_secs_f64() * 1e3);
            }
            times.sort_by(f64::total_cmp);
            println!(
                "timing {s}x{s} hole r={r} ({label}): min {:.0} ms, median {:.0} ms; rmse {:.4}",
                times[0],
                times[2],
                hole_rmse(&out, &c.clean, &c.hole)
            );
        }
        if let Some(d) = &out_dir {
            write_png(&d.join(format!("timing_{s}_{tag}.png")), c.w, c.h, &[&c.clean, &out]);
        }
    }
}

#[test]
fn guided_result_is_independent_of_thread_count() {
    // 200×200 is above the parallel threshold: bands run on the pool.
    let mut c = case_horizon(200, 32.0);
    c.exclude = Some((0..200 * 200).map(|i| i % 200 < 12 && i / 200 > 150).collect());
    let input = dirty(&c);
    let p = CompleteParams::default();
    let many = run(&c, &input, &p);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    let one = pool.install(|| run(&c, &input, &p));
    assert_eq!(many, one);
}

#[test]
fn guided_completion_can_be_cancelled() {
    let c = case_wire(120, 20.0);
    let ex = vec![false; 120 * 120];
    let g = Guide { exclude: Some(&ex) };
    let r = complete_guided(c.w, c.h, CH, &dirty(&c), &c.hole, &g, &CompleteParams::default(), &|| true);
    assert_eq!(r, Err(lcc_inpaint::Cancelled));
}

/// A beach like a real one out of focus: soft transitions, a sea with darker and lighter streaks, a
/// bright surf band broken into foam blobs, and wet sand; a tall stroke (a person and their
/// reflection) crosses every band, wider than the coarse-to-fine refinement extent at full size.
fn case_surf(w: usize, h: usize) -> Case {
    let (hf, wf) = (h as f32, w as f32);
    let soft = |y: f32, at: f32, width: f32| ((y - at) / width).clamp(-1.0, 1.0) * 0.5 + 0.5;
    let clean = build(w, h, |x, y| {
        let yf = y as f32;
        let t = 0.012 * hash(x, y, 22);
        let sky = [0.55, 0.68, 0.85];
        let streak = 0.06 * value_noise(x / 3, y, 18, 24);
        let sea = [0.12 + streak, 0.22 + streak, 0.35 + streak];
        let foam = 0.3 * value_noise(x, y / 2, 30, 23).max(-0.2);
        let surf = [0.62 + foam, 0.68 + foam, 0.78 + foam];
        let sand = [0.4, 0.48, 0.6];
        let a = lerp3(sky, sea, soft(yf, 0.3 * hf, 1.0));
        let b = lerp3(a, surf, soft(yf, 0.52 * hf, 10.0));
        let c = lerp3(b, sand, soft(yf, 0.62 * hf, 8.0));
        [c[0] + t, c[1] + t, c[2] + t]
    });
    let (cx, w2) = (wf * 0.55, wf * 0.12);
    let hole: Vec<bool> = (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
            let yy = y.clamp(hf * 0.2, hf * 0.85);
            (x - cx).hypot(y - yy) < w2
        })
        .collect();
    Case { name: "g_surf", w, h, clean, hole, exclude: None }
}

/// The largest jump between neighbouring columns of the surf band's mean brightness inside the hole,
/// over the same measure on the clean photo: a hard, straight seam (a block copied with an abrupt
/// end) shows as a large jump.
fn surf_seam(c: &Case, out: &[f32]) -> f32 {
    let (y0, y1) = ((c.h as f32 * 0.52) as usize, (c.h as f32 * 0.6) as usize);
    let col = |img: &[f32], x: usize| (y0..y1).map(|y| img[(y * c.w + x) * CH + 1]).sum::<f32>() / (y1 - y0) as f32;
    // the hole's columns and one known column on each side (a seam sits where the fill meets the photo)
    let mut inside: Vec<usize> = (0..c.w).filter(|x| (y0..y1).all(|y| c.hole[y * c.w + x])).collect();
    if let (Some(&a), Some(&b)) = (inside.first(), inside.last()) {
        inside.insert(0, a.saturating_sub(2));
        inside.push((b + 2).min(c.w - 1));
    }
    let step = |img: &[f32]| inside.windows(2).map(|p| (col(img, p[1]) - col(img, p[0])).abs()).fold(0.0f32, f32::max);
    step(out) / step(&c.clean).max(1e-3)
}

// Given parallel lines meeting a tall hole, the fill has no hard straight seams across them
#[test]
fn parallel_lines_leave_no_blocky_seams() {
    let c = case_surf(560, 700);
    let out = run(&c, &dirty(&c), &CompleteParams::default());
    let seam = surf_seam(&c, &out);
    eprintln!("surf seam ratio {seam}");
    if let Some(dir) = std::env::var_os("INPAINT_OUT") {
        write_png(&std::path::Path::new(&dir).join("g_surf.png"), c.w, c.h, &[&c.clean, &dirty(&c), &out]);
    }
    assert!(seam < 2.5, "a jump {seam}× the photo's own across the surf line");
}
