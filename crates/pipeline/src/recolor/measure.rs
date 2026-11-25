//! Measuring a recolored area's main colour ([`super::Reference`]) on a fixed-size render of the
//! whole frame, through the same develop steps the finish stage runs before it recolors (local
//! tone, calibration, the tone map and the colour stage), so the main colour lands on the picked
//! one with Shadows, Contrast or Clarity set too.
//!
//! This repo's adaptation of upstream's `recolor/measure.rs`: the proxy colour is a
//! full [`crate::render`] of the frame at [`PROXY_EDGE`] px on the long side (linear float,
//! recolor disabled, a re-entrancy guard keeps the inner plan from measuring again), and the
//! masks' alphas are evaluated on a white-balanced proxy of the same size (as `mask_tone` does),
//! so preview, export, zoomed windows, CPU and GPU all measure the same numbers.

use std::sync::{Mutex, PoisonError};

use lcc_color::perceptual::oklab_from_2020;
use lcc_color::transfer::srgb_to_linear;
use lcc_color::{REC2020, SRGB};
use lcc_develop::{DevelopSettings, Mask};
use lcc_raster::{Plane, Rgb32f};

use super::{Reference, hue, is_on, saturation, source_rgb, srgb_to_oklab, wrap};
use crate::geometry::Frame;
use crate::local::{frame_proxy, log_lum, white_balance};
use crate::masks::Mattes;
use crate::{OutputDepth, OutputSpace, Quality, RenderRequest, SourceInfo};

/// Long edge of the render the main colours are measured on: fixed, so every render size gets the
/// same numbers.
pub const PROXY_EDGE: usize = 384;
/// Long edge of the second render for a mask too small to measure on the first ([`SMALL_WEIGHT`]).
pub const FINE_EDGE: usize = 1024;
/// A mask with less alpha than this (in proxy pixels) is measured again on the [`FINE_EDGE`] render.
pub const SMALL_WEIGHT: f32 = 64.0;
/// Least total alpha (in proxy pixels) an area needs for a main colour: a mask with less recolors
/// nothing.
pub const MIN_WEIGHT: f32 = 1.0;
/// A pixel counts towards the main hue when its chroma and saturation pass these.
pub const CHROMATIC: [f32; 2] = [0.025, 0.08];
/// Hue histogram bins.
const BINS: usize = 72;
/// The peak's neighbourhood that sets the main hue, and the cluster around it (radians).
const PEAK_HALF: f32 = 0.45;
const CLUSTER_HALF: f32 = 0.6;
/// How many measured edits are kept.
const KEPT: usize = 4;

/// OkLab of a rendered sample: linear sRGB (a deep render's samples) → linear Rec.2020 → OkLab.
/// The eyedropper (`mask.recolorPick`) reads the same kind of render, so a picked colour is the
/// colour a measurement of its point finds.
pub(crate) fn sample_to_oklab(lin: [f32; 3]) -> [f32; 3] {
    oklab_from_2020(SRGB.to_space(&REC2020).apply_f32(lin.map(|v| srgb_to_linear(v.clamp(0.0, 1.0)))))
}

/// The main colour of `samples` (OkLab, alpha): the peak of a hue histogram of its coloured pixels
/// weighted by chroma × alpha, refined to their weighted circular mean, with that cluster's
/// lightness, chroma and spread. With `from` (the Colour to Replace, OkLab) the cluster is the one
/// around its hue. Mostly grey samples give a grey reference (their mean). `None` with less than
/// [`MIN_WEIGHT`] of alpha. Ported from upstream (`recolor/measure.rs`).
pub fn main_colour(samples: &[([f32; 3], f32)], from: Option<[f32; 3]>) -> Option<Reference> {
    let ok = |(lab, w): &&([f32; 3], f32)| w.is_finite() && *w > 0.0 && lab.iter().all(|v| v.is_finite());
    let lch = |lab: [f32; 3]| (lab[0], lab[1].hypot(lab[2]), hue(lab[1], lab[2]));
    let (mut total, mut sl, mut sc, mut sa, mut sb, mut chroma_w) = (0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
    let mut hist = [0f64; BINS];
    for (lab, w) in samples.iter().filter(ok) {
        let (l, c, h) = lch(*lab);
        let w64 = f64::from(*w);
        total += w64;
        sl += w64 * f64::from(l);
        sc += w64 * f64::from(c);
        sa += w64 * f64::from(lab[1]);
        sb += w64 * f64::from(lab[2]);
        if c > CHROMATIC[0] && saturation(c, l) > CHROMATIC[1] {
            chroma_w += w64;
            let bin = (((h + std::f32::consts::PI) / std::f32::consts::TAU * BINS as f32) as usize).min(BINS - 1);
            if let Some(b) = hist.get_mut(bin) {
                *b += w64 * f64::from(c);
            }
        }
    }
    if total < f64::from(MIN_WEIGHT) {
        return None;
    }
    let mean_hue = [(sa / total) as f32, (sb / total) as f32];
    let grey = || {
        let n = mean_hue[0].hypot(mean_hue[1]);
        let h = if n > 1e-9 { [mean_hue[0] / n, mean_hue[1] / n] } else { [1.0, 0.0] };
        Reference { l: (sl / total) as f32, c: (sc / total) as f32, h, spread: 0.0 }
    };
    let peak = match from {
        Some(f) => hue(f[1], f[2]),
        None if chroma_w < 0.1 * total => return Some(grey()),
        None => {
            let smoothed = (0..BINS).map(|i| {
                [(-2isize, 1.0), (-1, 2.0), (0, 3.0), (1, 2.0), (2, 1.0)]
                    .iter()
                    .map(|(d, k)| k * hist.get((i as isize + d).rem_euclid(BINS as isize) as usize).copied().unwrap_or(0.0))
                    .sum::<f64>()
            });
            let best = smoothed.enumerate().max_by(|a, b| a.1.total_cmp(&b.1)).map_or(0, |(i, _)| i);
            (best as f32 + 0.5) / BINS as f32 * std::f32::consts::TAU - std::f32::consts::PI
        }
    };
    // the main hue: the weighted circular mean near the peak
    let (mut ca, mut cb) = (0f64, 0f64);
    for (lab, w) in samples.iter().filter(ok) {
        let (l, c, h) = lch(*lab);
        if c > CHROMATIC[0] && saturation(c, l) > CHROMATIC[1] && wrap(h - peak).abs() < PEAK_HALF {
            ca += f64::from(*w) * f64::from(lab[1]);
            cb += f64::from(*w) * f64::from(lab[2]);
        }
    }
    let main = if ca.hypot(cb) > 1e-12 { (cb as f32).atan2(ca as f32) } else { peak };
    // the cluster around it: its lightness, chroma and spread
    let (mut w_sum, mut l_sum, mut c_sum, mut spread) = (0f64, 0f64, 0f64, 0f64);
    for (lab, w) in samples.iter().filter(ok) {
        let (l, c, h) = lch(*lab);
        let d = wrap(h - main);
        if c > CHROMATIC[0] && saturation(c, l) > CHROMATIC[1] && d.abs() < CLUSTER_HALF {
            let w64 = f64::from(*w);
            w_sum += w64;
            l_sum += w64 * f64::from(l);
            c_sum += w64 * f64::from(c);
            spread += w64 * f64::from(d * d);
        }
    }
    let h = [main.cos(), main.sin()];
    if w_sum < f64::from(MIN_WEIGHT) * 0.5 {
        // a Colour to Replace the area hardly holds: its own lightness and chroma
        return from.map(|f| Reference { l: f[0], c: f[1].hypot(f[2]), h, spread: 0.15 }).or_else(|| Some(grey()));
    }
    Some(Reference { l: (l_sum / w_sum) as f32, c: (c_sum / w_sum) as f32, h, spread: (spread / w_sum).sqrt() as f32 })
}

/// Set while [`references`] renders: the inner plan must not measure again.
fn measuring() -> bool {
    MEASURING.with(|m| m.get())
}

thread_local! {
    static MEASURING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The settings the references read: everything but each mask's Recolor amount, target colour and
/// Range (which act after the measurement, and whose drag must not measure again).
fn key_of(src: &Rgb32f, info: &SourceInfo, s: &DevelopSettings, frame: &Frame, mattes: Option<&Mattes>) -> u64 {
    let mut m = s.clone();
    for mask in &mut m.masks {
        let a = &mut mask.adjust;
        let on = is_on(a);
        (a.recolor, a.recolor_r, a.recolor_g, a.recolor_b, a.recolor_range, a.recolor_from_r, a.recolor_from_g, a.recolor_from_b) =
            (if on { 100.0 } else { 0.0 }, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    }
    let mattes_id = mattes.map(|m| (std::ptr::from_ref(m) as usize, m.bytes()));
    crate::hash_of((
        crate::optics::fingerprint(src),
        format!("{m:?}"),
        format!("{frame:?}"),
        [info.as_shot_temp, info.as_shot_tint].map(f64::to_bits),
        mattes_id,
    ))
}

/// The main colour of every visible mask whose Recolor is on (by mask id), measured on a
/// [`PROXY_EDGE`] render of the whole frame (a mask too small for it on a [`FINE_EDGE`] one). The
/// sizes don't depend on the request, so every full render of one edit (preview, export, a window,
/// CPU or GPU) gets the same numbers. The last few edits' answers are kept: a Recolor amount,
/// colour or Range drag measures nothing again (their values are keyed out). Measured on the CPU
/// even for GPU renders (the GPU reads [`crate::Plan::recolor`] like [`crate::Plan::mask_tone`]).
pub(crate) fn references(src: &Rgb32f, info: &SourceInfo, s: &DevelopSettings, frame: &Frame, mattes: Option<&Mattes>) -> Vec<(u32, Reference)> {
    if measuring() {
        return Vec::new();
    }
    let on: Vec<&Mask> = s.masks.iter().filter(|m| m.visible && !m.components.is_empty() && is_on(&m.adjust) && m.adjust.amount != 0.0).collect();
    if on.is_empty() || src.width == 0 || src.height == 0 {
        return Vec::new();
    }
    type Entry = (u64, Vec<(u32, Reference)>);
    static LAST: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
    let key = key_of(src, info, s, frame, mattes);
    {
        let cache = LAST.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(found) = cache.iter().rev().find(|e| e.0 == key) {
            return found.1.clone();
        }
    }
    let out = measure_all(src, info, s, frame, mattes, &on);
    let mut cache = LAST.lock().unwrap_or_else(PoisonError::into_inner);
    cache.push((key, out.clone()));
    if cache.len() > KEPT {
        cache.remove(0);
    }
    out
}

/// Every wanted mask's main colour: a full render of the frame at [`PROXY_EDGE`] px (recolor
/// disabled), then one [`Reference`] per wanted mask from its alpha's samples; a mask with too
/// little weight is measured again on a [`FINE_EDGE`] render.
fn measure_all(src: &Rgb32f, info: &SourceInfo, s: &DevelopSettings, frame: &Frame, mattes: Option<&Mattes>, on: &[&Mask]) -> Vec<(u32, Reference)> {
    let ids: Vec<u32> = on.iter().map(|m| m.id).collect();
    let froms: Vec<Option<[f32; 3]>> =
        on.iter().map(|m| s.masks.iter().find(|x| x.id == m.id).and_then(|x| source_rgb(&x.adjust)).map(srgb_to_oklab)).collect();
    let (first, small) = measure_pass(src, info, s, frame, mattes, &ids, &froms, PROXY_EDGE);
    let mut out: Vec<(u32, Reference)> = first.into_iter().filter_map(|(id, r)| r.map(|r| (id, r))).collect();
    if !small.is_empty() {
        let froms2: Vec<Option<[f32; 3]>> =
            small.iter().map(|id| s.masks.iter().find(|x| x.id == *id).and_then(|x| source_rgb(&x.adjust)).map(srgb_to_oklab)).collect();
        let (fine, _) = measure_pass(src, info, s, frame, mattes, &small, &froms2, FINE_EDGE);
        for (fid, fr) in fine.into_iter().filter_map(|(id, r)| r.map(|r| (id, r))) {
            match out.iter_mut().find(|(id, _)| *id == fid) {
                Some(slot) => slot.1 = fr,
                None => out.push((fid, fr)),
            }
        }
    }
    // masks never measurable (no weight at either size) recolor nothing: they stay out
    out
}

/// One measurement pass on a render of long edge `edge`: per wanted mask id, its [`Reference`] (or
/// `None`: unmeasurable here), and whether it wants the finer pass (`true` when its total alpha is
/// under [`SMALL_WEIGHT`]).
#[allow(clippy::too_many_arguments)]
fn measure_pass(
    src: &Rgb32f,
    info: &SourceInfo,
    s: &DevelopSettings,
    frame: &Frame,
    mattes: Option<&Mattes>,
    ids: &[u32],
    froms: &[Option<[f32; 3]>],
    edge: usize,
) -> (Vec<(u32, Option<Reference>)>, Vec<u32>) {
    let req = RenderRequest { depth: OutputDepth::F32Linear, space: OutputSpace::Srgb, quality: Quality::Full, ..RenderRequest::fit(edge, edge) };
    // a render without Recolor (the measurement runs inside the plan of a render with it on)
    let mut s0 = s.clone();
    for m in &mut s0.masks {
        let a = &mut m.adjust;
        (a.recolor, a.recolor_r, a.recolor_g, a.recolor_b, a.recolor_range, a.recolor_from_r, a.recolor_from_g, a.recolor_from_b) =
            (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    }
    MEASURING.with(|m| m.set(true));
    let rendered = crate::render(src, info, &s0, &req);
    MEASURING.with(|m| m.set(false));
    let samples = match rendered.deep.as_ref().map(|d| (&d.samples, d.width, d.height)) {
        Some((crate::DeepSamples::F32(v), w, h)) => Some((v, w, h)),
        _ => None,
    };
    let Some((deep, pw, ph)) = samples else { return (ids.iter().map(|id| (*id, None)).collect(), Vec::new()) };
    if pw == 0 || ph == 0 {
        return (ids.iter().map(|id| (*id, None)).collect(), Vec::new());
    }
    // the masks' alphas, on a white-balanced proxy of the rendered size (as mask_tone evaluates them)
    let mut img = Rgb32f::clone(&frame_proxy(src, frame, pw, ph));
    white_balance(&mut img, info, s);
    let log_l: Plane = img.map(log_lum);
    let ev = s.light.exposure as f32;
    let lab: Vec<[f32; 3]> = deep.as_chunks::<3>().0.iter().copied().map(sample_to_oklab).collect();
    let out: Vec<(u32, Option<Reference>, f32)> = ids
        .iter()
        .zip(froms)
        .map(|(id, from)| {
            let Some(m) = s.masks.iter().find(|m| m.id == *id) else { return (*id, None, 0.0) };
            let alpha = crate::masks::evaluate_one(m, frame, pw, ph, &img, &log_l, ev, mattes);
            let mut picked: Vec<([f32; 3], f32)> = Vec::new();
            for (l, a) in lab.iter().zip(&alpha.data) {
                if *a > 0.0 && a.is_finite() {
                    picked.push((*l, *a));
                }
            }
            let weight: f32 = picked.iter().map(|(_, a)| a).sum();
            (*id, main_colour(&picked, *from), weight)
        })
        .collect();
    let small = out.iter().filter(|(_, _, w)| *w < SMALL_WEIGHT).map(|(id, _, _)| *id).collect();
    (out.into_iter().map(|(id, r, _)| (id, r)).collect(), small)
}
