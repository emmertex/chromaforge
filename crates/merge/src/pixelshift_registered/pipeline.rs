//! The registered merge of a series, from the frames to the merged image: decode, brightness and positions, the
//! decision to register, reconstruction; or the plain merge with the reason when the series is refused.
//!
//! Memory: the frames' mosaics are kept (about 91 MB each for 45 megapixels), the green planes of the analysis (about 45
//! MB each) only until the positions are known, and the merged image (1.1 GB at 16560 × 11040 × 3 samples) is allocated
//! when the reconstruction starts, so the peak is about 2.9 GB for a 16-shot series of 45 megapixels.

use super::gain::Gains;
use super::gate::GateResult;
use super::positions::Start;
use super::recon::{Params, reconstruct};
use super::register::{FallbackReason, Outcome, RegisterOptions, Timings, register_progress};
use super::samples::{Mosaics, Samples};
use crate::pixelshift::{Merged, Pattern};
use crate::{MergeError, Result};
use lcc_raw::RawImage;
use rayon::prelude::*;
use std::time::Instant;

/// Frames decoded at the same time (each is about 91 MB of samples once decoded, plus its file).
const DECODE_AT_ONCE: usize = 4;

/// Noise of the Z 8 samples: the variance of a sample is `a` times its level above black plus `b`.
pub const NOISE: (f64, f64) = (0.315_782_850_128_106_53, 0.0);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Options {
    pub register: RegisterOptions,
    pub noise: (f64, f64),
}

impl Default for Options {
    fn default() -> Options {
        Options { register: RegisterOptions::default(), noise: NOISE }
    }
}

/// What was done and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    /// The image is the registered reconstruction (false: the plain merge).
    pub registered: bool,
    /// Why the series was merged plainly.
    pub reason: Option<FallbackReason>,
    pub gate: Option<GateResult>,
    pub start: Option<Start>,
    /// Converged tiles the positions rest on.
    pub tiles: usize,
    /// Brightness gains of the frames (reconstruction) or of the plain merge (when they were applied).
    pub gains: Option<Gains>,
    pub timings: Stages,
}

/// Seconds spent per stage.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Stages {
    pub decode: f64,
    pub green: f64,
    pub analysis: Timings,
    pub reconstruction: f64,
    pub plain: f64,
}

/// Merges the series `frames` (indices for `load`) of the 16-shot `pattern`. `progress(fraction)` returns false to cancel.
pub fn merge_series_registered(
    pattern: &'static Pattern,
    frames: std::ops::Range<usize>,
    load: &(dyn Fn(usize) -> std::result::Result<RawImage, String> + Sync),
    progress: &(dyn Fn(f32) -> bool + Sync),
    opts: &Options,
) -> Result<(Merged, Report)> {
    let mut st = Stages::default();
    let t = Instant::now();
    let n = frames.len().max(1);
    let mut mosaics: Option<Mosaics> = None;
    let indices: Vec<usize> = frames.clone().collect();
    for (c, chunk) in indices.chunks(DECODE_AT_ONCE).enumerate() {
        if !progress(0.15 * (c * DECODE_AT_ONCE) as f32 / n as f32) {
            return Err(MergeError::Cancelled);
        }
        // a few frames at a time: decoding is single-threaded per frame; each decoded frame is moved into the mosaics
        let imgs: Vec<std::result::Result<RawImage, String>> = chunk.par_iter().map(|&i| load(i)).collect();
        for img in imgs {
            let img = img.map_err(MergeError::Decode)?;
            let m = match mosaics.as_mut() {
                Some(m) => m,
                None => mosaics.insert(Mosaics::new(&img).map_err(MergeError::Mismatch)?),
            };
            m.push(img).map_err(MergeError::Mismatch)?;
        }
    }
    let mosaics = mosaics.ok_or(MergeError::TooFew(0, 1))?;
    st.decode = t.elapsed().as_secs_f64();
    if !progress(0.15) {
        return Err(MergeError::Cancelled);
    }
    let t = Instant::now();
    let mut series = mosaics.green_series(opts.noise).map_err(MergeError::Mismatch)?;
    st.green = t.elapsed().as_secs_f64();
    // the measurement may start again from the coarse positions: the reported fraction never goes back
    let top = std::sync::atomic::AtomicU32::new(0);
    let analysis = |f: f32| {
        let f = f.clamp(0.0, 1.0);
        let prev = f32::from_bits(top.fetch_max(f.to_bits(), std::sync::atomic::Ordering::Relaxed));
        progress(0.15 + 0.25 * f.max(prev))
    };
    let reg = register_progress(&mut series, &mosaics, &opts.register, Some(&analysis));
    drop(series);
    st.analysis = reg.timings;
    if !progress(0.4) {
        return Err(MergeError::Cancelled);
    }
    match reg.outcome {
        Outcome::Cancelled => Err(MergeError::Cancelled),
        Outcome::Registered(r) => {
            let t = Instant::now();
            let prm = Params::v4(mosaics.black(), opts.noise.0);
            let p = |f: f32| progress(0.4 + 0.55 * f);
            let merged = reconstruct(&mosaics, Some(&r.gains), &r.fit.models, mosaics.crop(), &prm, &p)?;
            st.reconstruction = t.elapsed().as_secs_f64();
            let report = Report {
                registered: true,
                reason: None,
                gate: Some(r.gate),
                start: Some(r.fit.start),
                tiles: r.fit.tiles.len(),
                gains: Some(r.gains),
                timings: st,
            };
            Ok((merged, report))
        }
        Outcome::Fallback { reason, plain_gains } => {
            drop(mosaics);
            let t = Instant::now();
            let p = |f: f32| progress(0.4 + 0.6 * f);
            let mut plain_load = |i: usize| load(i);
            let merged = super::fallback::merge_plain(pattern, frames, &mut plain_load, &p, plain_gains.as_ref())?;
            st.plain = t.elapsed().as_secs_f64();
            let gate = match &reason {
                FallbackReason::MeasuredWorse(g) => Some(g.clone()),
                _ => None,
            };
            let report = Report { registered: false, reason: Some(reason), gate, start: None, tiles: 0, gains: plain_gains, timings: st };
            Ok((merged, report))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::coarse::nominal_positions;
    use super::super::synth::{BLACK, Scene, Spec, WHITE};
    use super::*;
    use crate::pixelshift;
    use lcc_geom::Orientation;
    use lcc_raw::{BlackLevel, Cfa, ColorData, RawData, RawFormat, Rect};

    const W: usize = 800;
    const H: usize = 700;

    fn raw(spec: &Spec, scene: &Scene, k: usize) -> RawImage {
        RawImage {
            format: RawFormat::Nef,
            width: W,
            height: H,
            cpp: 1,
            data: RawData::U16(super::super::synth::mosaic(spec, scene, k)),
            cfa: Cfa::bayer("RGGB"),
            bits: 14,
            black: BlackLevel::uniform(BLACK as f32),
            white: vec![WHITE as f32],
            active_area: Rect::new(0, 0, W, H),
            crop: Rect::new(2, 2, W - 4, H - 4),
            orientation: Orientation::Normal,
            color: ColorData::default(),
            wb_multipliers: None,
            linearized: false,
            pixel_aspect: 1.0,
            opcodes: Default::default(),
            metadata: Default::default(),
            lens_metadata: Default::default(),
        }
    }

    fn spec() -> Spec {
        Spec {
            aw: W,
            ah: H,
            frames: 16,
            gpar: 1,
            positions: Some(nominal_positions().iter().map(|p| [p[0] + 0.3 * (p[1] - 0.25), p[1] - 0.2 * p[0]]).collect()),
            ..Spec::default()
        }
    }

    fn run(spec: &Spec, progress: &(dyn Fn(f32) -> bool + Sync)) -> Result<(Merged, Report)> {
        let scene = Scene::new(spec.seed, spec.level, spec.contrast);
        let load = |i: usize| Ok(raw(spec, &scene, i));
        merge_series_registered(&pixelshift::SHOT_16, 0..16, &load, progress, &Options::default())
    }

    #[test]
    fn a_textured_series_is_registered_and_reconstructed() {
        let s = spec();
        let last = std::sync::Mutex::new(0.0f32);
        let (m, r) = run(&s, &|f| {
            let mut l = last.lock().unwrap();
            assert!(f >= *l, "progress never goes back");
            *l = f;
            true
        })
        .unwrap();
        assert!(r.registered && r.reason.is_none() && r.gate.is_some(), "{r:?}");
        assert_eq!((m.width, m.height, m.scale), (2 * W, 2 * H, 2));
        assert_eq!(m.crop, [4, 4, 2 * (W - 4), 2 * (H - 4)]);
        assert!(r.timings.reconstruction > 0.0 && r.timings.plain == 0.0);
        assert!(*last.lock().unwrap() >= 0.95);
    }

    #[test]
    fn a_series_without_texture_is_merged_plainly_with_the_reason() {
        let s = Spec { flat: true, ..spec() };
        let (m, r) = run(&s, &|_| true).unwrap();
        assert!(!r.registered);
        assert_eq!(r.reason, Some(FallbackReason::NoTexture));
        assert_eq!((m.width, m.height), (2 * W, 2 * H));
        assert!(r.timings.plain > 0.0 && r.timings.reconstruction == 0.0);
    }

    #[test]
    fn cancelling_stops_the_merge_at_every_stage() {
        for stop_after in [0usize, 1, 3, 5, 8, 12] {
            let calls = std::sync::atomic::AtomicUsize::new(0);
            let r = run(&spec(), &|_| calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < stop_after);
            assert!(matches!(r, Err(MergeError::Cancelled)), "stop after {stop_after} calls: {:?}", r.map(|x| x.1.registered));
        }
    }

    #[test]
    fn cancelling_during_the_measurement_stops_it_between_groups_of_tiles() {
        let r = run(&spec(), &|f| f < 0.2);
        assert!(matches!(r, Err(MergeError::Cancelled)), "{:?}", r.map(|x| x.1.registered));
    }

    #[test]
    fn a_failing_load_is_reported() {
        let load = |_: usize| -> std::result::Result<RawImage, String> { Err("no such file".into()) };
        let r = merge_series_registered(&pixelshift::SHOT_16, 0..16, &load, &|_| true, &Options::default());
        assert!(matches!(r, Err(MergeError::Decode(m)) if m == "no such file"));
    }
}
