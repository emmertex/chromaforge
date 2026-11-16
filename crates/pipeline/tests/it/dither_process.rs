//! Process V8 dithers 8-bit output with an 8×8 ordered pattern (adapted from upstream #3509); V1–V7
//! keep plain rounding (`docs/process-versions.md`).

use lcc_develop::{DevelopSettings, Process};
use lcc_pipeline::{DeepSamples, OutputDepth, PixelWindow, RenderRequest, SourceInfo, render};
use lcc_raster::{Rgb32f, Rgba8};

const W: usize = 512;
const H: usize = 64;

/// A slow horizontal ramp: a dozen 8-bit levels over the width, so rounding leaves wide flat bands.
fn ramp() -> Rgb32f {
    Rgb32f::from_fn(W, H, |x, _| {
        let v = 0.16 + 0.03 * x as f32 / (W - 1) as f32;
        [v, v, v]
    })
}

fn at(process: Process) -> DevelopSettings {
    DevelopSettings { process: process.version(), ..DevelopSettings::default() }
}

fn image(s: &DevelopSettings, req: &RenderRequest) -> Rgba8 {
    render(&ramp(), &SourceInfo::default(), s, req).image
}

/// The green channel in 8-bit levels without rounding: the 16-bit render, scaled.
fn reference(s: &DevelopSettings) -> Vec<f64> {
    let r = render(&ramp(), &SourceInfo::default(), s, &RenderRequest { depth: OutputDepth::U16, ..RenderRequest::fit(W, H) });
    match r.deep.expect("a 16-bit render").samples {
        DeepSamples::U16(v) => v.as_chunks::<3>().0.iter().map(|p| f64::from(p[1]) / 257.0).collect(),
        _ => panic!("16-bit samples"),
    }
}

/// The worst difference, over the 8×8 tiles, between the tile's mean 8-bit level and its exact mean.
fn worst_tile_error(img: &Rgba8, exact: &[f64]) -> f64 {
    let mut worst = 0.0f64;
    for ty in 0..H / 8 {
        for tx in 0..W / 8 {
            let (mut got, mut want) = (0.0, 0.0);
            for y in ty * 8..ty * 8 + 8 {
                for x in tx * 8..tx * 8 + 8 {
                    got += f64::from(img.data[y * W + x][1]);
                    want += exact[y * W + x];
                }
            }
            worst = worst.max((got - want).abs() / 64.0);
        }
    }
    worst
}

#[test]
fn v8_dithers_a_gradient_so_each_tile_keeps_its_exact_tone() {
    let req = RenderRequest::fit(W, H);
    let (v7, v8) = (at(Process::V7), at(Process::V8));
    let (e7, e8) = (worst_tile_error(&image(&v7, &req), &reference(&v7)), worst_tile_error(&image(&v8, &req), &reference(&v8)));
    eprintln!("worst 8×8 tile error: V7 {e7:.3} levels, V8 {e8:.3} levels");
    assert!(e7 > 0.4, "rounding bands: a tile is off by up to half a level ({e7})");
    assert!(e8 < 0.1, "dithered tiles keep their tone ({e8})");
}

#[test]
fn processes_before_v8_round_without_dither() {
    let req = RenderRequest::fit(W, H);
    for p in [Process::V1, Process::V5, Process::V7] {
        let img = image(&at(p), &req);
        let exact = reference(&at(p));
        for (q, e) in img.data.iter().zip(&exact) {
            // the 16-bit reference rounds too, so a level half-way can go either way
            assert!((f64::from(q[1]) - e).abs() <= 0.51, "{p:?}: {} for {e}", q[1]);
        }
    }
}

#[test]
fn a_zoomed_window_dithers_as_the_same_pixels_of_the_whole_render() {
    let s = at(Process::V8);
    let full = image(&s, &RenderRequest::fit(W, H));
    // an origin off the 8×8 grid: the pattern belongs to the whole output, not to the window
    let win = PixelWindow { x: 77, y: 13, w: 101, h: 37 };
    let part = image(&s, &RenderRequest { window: Some(win), ..RenderRequest::fit(W, H) });
    assert_eq!((part.width, part.height), (win.w, win.h));
    for y in 0..win.h {
        for x in 0..win.w {
            assert_eq!(part.data[y * win.w + x], full.data[(win.y + y) * W + win.x + x], "({x}, {y}) of the window");
        }
    }
}

/// The dither is for 8-bit output only: the 16-bit and float renders of a non-raw source are the same on V8 as on V7
/// (colour noise reduction off; V8 changes it too, and raw highlights, which this ramp doesn't have).
#[test]
fn v8_leaves_16_bit_and_float_output_as_v7_renders_it() {
    let quiet = |p: Process| {
        let mut s = at(p);
        s.detail.nr_color = 0.0;
        s
    };
    for depth in [OutputDepth::U16, OutputDepth::F32Linear] {
        let req = RenderRequest { depth, ..RenderRequest::fit(W, H) };
        let deep = |p: Process| render(&ramp(), &SourceInfo::default(), &quiet(p), &req).deep;
        let (v7, v8) = (deep(Process::V7), deep(Process::V8));
        assert!(v7.is_some(), "{depth:?}: a deep render");
        assert_eq!(v7, v8, "{depth:?}: V8 renders as V7");
    }
}
