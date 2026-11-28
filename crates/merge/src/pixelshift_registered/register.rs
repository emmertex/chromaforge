//! The registration of a 16-shot series: brightness, tiles, positions, gate, gains, and the decision to register or to
//! fall back to the plain merge.
//!
//! Order: the global gains of the frames are estimated first; if they vary by more than noise the green planes are put on
//! a common level before anything is measured. A quick check of the nominal table on eight tiles decides whether the
//! nominal start is worth a full measurement; its result is used if it is measurable (at least eight tiles), at most a
//! tenth of its tiles failed to converge and the gate does not call the positions worse than the table; otherwise the
//! series is measured from the coarse start. The gate decides: `Better` and `Equal` register (the kernel
//! reconstruction removes the placement's aliasing pattern even when the positions are about nominal), `Worse` refuses.
//! A series that cannot be measured (too few tiles, no texture, no start) is refused with the reason.

use super::coarse::nominal_positions;
use super::gain::{Gains, TileGainReport, global_gains, normalise_green, tile_gains};
use super::gate::{GateResult, GateVerdict, gate};
use super::measure::EarlyStop;
use super::positions::{MeasureError, Options, Progress, SeriesFit, Start, measure_series, quick_nominal_check};
use super::samples::Samples;
use super::series::GreenSeries;
use super::tiles::{Tile, select_tiles};
use std::time::Instant;

/// Fewest converged tiles for a measurement to be trusted.
pub const MIN_TILES: usize = 8;
/// Largest share of the usable tiles that may fail to converge in the nominal result.
pub const MAX_DROPPED: f64 = 0.10;

#[derive(Debug, Clone, PartialEq)]
pub enum FallbackReason {
    /// The series does not have 16 frames.
    FrameCount(usize),
    /// No tile of the series has enough texture to measure.
    NoTexture,
    /// No start positions could be found (the frames do not look alike).
    NoStart,
    /// Fewer converged tiles than needed.
    TooFewTiles { usable: usize, needed: usize },
    /// The measured positions predict the data significantly worse than the nominal table.
    MeasuredWorse(GateResult),
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Timings {
    pub gains_ms: f64,
    pub quick_ms: f64,
    pub measure_ms: f64,
    pub gate_ms: f64,
    pub tile_gains_ms: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Registered {
    pub fit: SeriesFit,
    pub gate: GateResult,
    /// Per-frame, per-colour gains for the reconstruction (the robust per-tile estimate, else the global one).
    pub gains: Gains,
    pub tile_gain_report: Option<TileGainReport>,
    /// The green planes were put on a common level before the measurement.
    pub normalised: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Registered(Box<Registered>),
    Fallback {
        reason: FallbackReason,
        /// Gains for the plain merge: only when the frames' brightness varies by more than noise.
        plain_gains: Option<Gains>,
    },
    /// The progress callback asked to stop.
    Cancelled,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Registration {
    pub outcome: Outcome,
    /// The global gains of the frames (when they could be estimated).
    pub global: Option<Gains>,
    pub timings: Timings,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegisterOptions {
    pub early_stop: Option<EarlyStop>,
}

impl Default for RegisterOptions {
    fn default() -> RegisterOptions {
        RegisterOptions { early_stop: Some(EarlyStop::default()) }
    }
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// Registers a series with the tiles the texture selection picks.
pub fn register(series: &mut GreenSeries, samples: &dyn Samples, opts: &RegisterOptions) -> Registration {
    register_progress(series, samples, opts, None)
}

/// As [`register`], calling `progress` with the fraction of the measurement done; it returns false to cancel.
pub fn register_progress(series: &mut GreenSeries, samples: &dyn Samples, opts: &RegisterOptions, progress: Option<Progress>) -> Registration {
    let tiles = select_tiles(series).tiles;
    register_with(series, samples, &tiles, opts, progress, &measure_series)
}

/// Registers a series with the given tiles. `series` is modified: its green planes are put on a common level when the
/// frames' brightness varies by more than noise.
pub fn register_tiles(series: &mut GreenSeries, samples: &dyn Samples, tiles: &[Tile], opts: &RegisterOptions) -> Registration {
    register_with(series, samples, tiles, opts, None, &measure_series)
}

/// The measurement step of the registration (replaceable so that tests can feed the decision a measurement that is wrong).
pub type MeasureFn<'a> = dyn Fn(&GreenSeries, &[Tile], Start, &Options<'_>) -> Result<SeriesFit, MeasureError> + 'a;

fn register_with(
    series: &mut GreenSeries,
    samples: &dyn Samples,
    tiles: &[Tile],
    opts: &RegisterOptions,
    progress: Option<Progress>,
    measure: &MeasureFn,
) -> Registration {
    let mut tm = Timings::default();
    let n = series.frame_count();
    let fallback = |reason: FallbackReason, global: &Option<Gains>, tm: Timings| Registration {
        outcome: Outcome::Fallback { reason, plain_gains: global.clone().filter(Gains::significant) },
        global: global.clone(),
        timings: tm,
    };
    if n != 16 || samples.frame_count() != 16 {
        return fallback(FallbackReason::FrameCount(n.max(samples.frame_count())), &None, tm);
    }
    let t = Instant::now();
    let global = global_gains(samples);
    tm.gains_ms = ms(t);
    let normalised = global.as_ref().is_some_and(Gains::significant);
    if let (true, Some(g)) = (normalised, &global) {
        normalise_green(series, g);
    }
    if tiles.is_empty() {
        return fallback(FallbackReason::NoTexture, &global, tm);
    }
    let nominal = nominal_positions();
    let options = Options { early_stop: opts.early_stop, progress };
    let accept = |fit: SeriesFit, gate_result: GateResult, tm: &mut Timings| -> Registration {
        let t = Instant::now();
        let tg = tile_gains(samples, &fit.tiles.iter().map(|x| x.tile).collect::<Vec<_>>(), &fit.models);
        tm.tile_gains_ms = ms(t);
        let (gains, report) = match tg {
            Some((g, r)) => (g, Some(r)),
            None => (global.clone().unwrap_or_else(|| Gains::identity(n)), None),
        };
        Registration {
            outcome: Outcome::Registered(Box::new(Registered { fit, gate: gate_result, gains, tile_gain_report: report, normalised })),
            global: global.clone(),
            timings: *tm,
        }
    };
    // the nominal start, if a quick check on a few tiles says the camera followed the table
    let t = Instant::now();
    let quick = quick_nominal_check(series, tiles);
    tm.quick_ms = ms(t);
    if quick.passes {
        let t = Instant::now();
        let fit = measure(series, tiles, Start::Nominal, &options);
        tm.measure_ms += ms(t);
        if matches!(fit, Err(MeasureError::Cancelled)) {
            return Registration { outcome: Outcome::Cancelled, global, timings: tm };
        }
        if let Ok(fit) = fit
            && fit.tiles.len() >= MIN_TILES
            && fit.dropped_fraction() <= MAX_DROPPED
        {
            let t = Instant::now();
            let g = gate(series, &fit.tiles, &nominal);
            tm.gate_ms += ms(t);
            if let Some(g) = g
                && g.verdict != GateVerdict::Worse
            {
                return accept(fit, g, &mut tm);
            }
        }
    }
    // the coarse start
    let t = Instant::now();
    let fit = measure(series, tiles, Start::Coarse, &options);
    tm.measure_ms += ms(t);
    let fit = match fit {
        Ok(f) => f,
        Err(MeasureError::NoStart) => return fallback(FallbackReason::NoStart, &global, tm),
        Err(MeasureError::TooFewTiles(k, _)) => return fallback(FallbackReason::TooFewTiles { usable: k, needed: MIN_TILES }, &global, tm),
        Err(MeasureError::NoTiles) => return fallback(FallbackReason::NoTexture, &global, tm),
        Err(MeasureError::FrameCount(k)) => return fallback(FallbackReason::FrameCount(k), &global, tm),
        Err(MeasureError::Cancelled) => return Registration { outcome: Outcome::Cancelled, global, timings: tm },
    };
    if fit.tiles.len() < MIN_TILES {
        return fallback(FallbackReason::TooFewTiles { usable: fit.tiles.len(), needed: MIN_TILES }, &global, tm);
    }
    let t = Instant::now();
    let g = gate(series, &fit.tiles, &nominal);
    tm.gate_ms += ms(t);
    match g {
        None => fallback(FallbackReason::TooFewTiles { usable: fit.tiles.len(), needed: MIN_TILES }, &global, tm),
        Some(g) if g.verdict == GateVerdict::Worse => fallback(FallbackReason::MeasuredWorse(g), &global, tm),
        Some(g) => accept(fit, g, &mut tm),
    }
}

#[cfg(test)]
mod tests {
    use super::super::synth::{Spec, mosaics, series as green_series};
    use super::*;

    const SIZE: usize = 800;

    fn tiles(n: usize) -> Vec<Tile> {
        [(301, 300), (501, 300), (301, 500), (501, 500), (401, 400), (601, 400), (401, 250), (401, 550), (301, 400), (501, 400)]
            .iter()
            .take(n)
            .map(|&(x, y)| Tile { xc: x, yc: y, rx: x as f64 - SIZE as f64 / 2.0, ry: y as f64 - SIZE as f64 / 2.0 })
            .collect()
    }

    fn run(spec: Spec, n_tiles: usize) -> Registration {
        let m = mosaics(&spec).unwrap();
        let mut s = green_series(&spec).unwrap();
        register_tiles(&mut s, &m, &tiles(n_tiles), &RegisterOptions::default())
    }

    fn spec() -> Spec {
        Spec { aw: SIZE, ah: SIZE, frames: 16, ..Spec::default() }
    }

    fn deviated(f: impl Fn(usize) -> [f64; 2]) -> Vec<[f64; 2]> {
        nominal_positions().iter().enumerate().map(|(k, n)| [n[0] + f(k)[0], n[1] + f(k)[1]]).collect()
    }

    #[test]
    fn a_series_that_deviates_from_the_table_registers_from_the_nominal_start() {
        let pos = deviated(|k| [0.9 * ((k * 3 % 7) as f64 / 6.0 - 0.5), 1.0 * ((k * 5 % 11) as f64 / 10.0 - 0.5)]);
        let r = run(Spec { positions: Some(pos), ..spec() }, 10);
        let Outcome::Registered(reg) = r.outcome else { panic!("{:?}", r.outcome) };
        assert_eq!(reg.fit.start, Start::Nominal);
        assert_eq!(reg.gate.verdict, GateVerdict::Better, "{:?}", reg.gate);
        assert!(!reg.normalised && !reg.gains.significant(), "flat brightness: gains stay within noise ({})", reg.gains.max_sd());
        assert!(r.timings.measure_ms > 0.0 && r.timings.gate_ms > 0.0);
    }

    #[test]
    fn a_series_that_followed_the_table_is_equal_and_still_registers() {
        let r = run(Spec { positions: Some(nominal_positions()), ..spec() }, 10);
        let Outcome::Registered(reg) = r.outcome else { panic!("{:?}", r.outcome) };
        assert_eq!(reg.gate.verdict, GateVerdict::Equal, "{:?}", reg.gate);
    }

    #[test]
    fn a_drift_of_a_dozen_sites_goes_through_the_coarse_start() {
        let pos = deviated(|k| [0.7 * k as f64, -0.8 * k as f64]);
        let r = run(Spec { positions: Some(pos), noise_free: true, ..spec() }, 10);
        let Outcome::Registered(reg) = r.outcome else { panic!("{:?}", r.outcome) };
        assert_eq!(reg.fit.start, Start::Coarse);
        assert_eq!(reg.gate.verdict, GateVerdict::Better, "{:?}", reg.gate);
    }

    #[test]
    fn flicker_of_several_percent_is_normalised_before_the_measurement_and_recovered_in_the_gains() {
        let gains: Vec<f64> = (0..16).map(|k| 1.0 + 0.2 * (((k * 7) % 16) as f64 / 15.0 - 0.5)).collect();
        let pos = deviated(|k| [0.9 * ((k * 3 % 7) as f64 / 6.0 - 0.5), 1.0 * ((k * 5 % 11) as f64 / 10.0 - 0.5)]);
        let r = run(Spec { positions: Some(pos), gains: gains.clone(), ..spec() }, 10);
        assert!(r.global.as_ref().is_some_and(Gains::significant));
        let Outcome::Registered(reg) = r.outcome else { panic!("{:?}", r.outcome) };
        assert!(reg.normalised);
        let mean = gains.iter().sum::<f64>() / 16.0;
        for k in 0..16 {
            assert!(
                (reg.gains.per_frame[k][1] - gains[k] / mean).abs() < 0.01,
                "frame {k}: {} against {}",
                reg.gains.per_frame[k][1],
                gains[k] / mean
            );
        }
    }

    #[test]
    fn too_few_tiles_refuse_with_the_count_and_a_flat_scene_with_no_texture() {
        let r = run(spec(), 5);
        let Outcome::Fallback { reason, .. } = r.outcome else { panic!("{:?}", r.outcome) };
        assert_eq!(reason, FallbackReason::TooFewTiles { usable: 5, needed: MIN_TILES });
        let flat = spec();
        let m = mosaics(&Spec { flat: true, ..flat.clone() }).unwrap();
        let mut s = green_series(&Spec { flat: true, ..flat }).unwrap();
        let r = register(&mut s, &m, &RegisterOptions::default());
        let Outcome::Fallback { reason, .. } = r.outcome else { panic!("{:?}", r.outcome) };
        assert_eq!(reason, FallbackReason::NoTexture);
    }

    #[test]
    fn the_plain_merge_gets_gains_only_when_the_flicker_is_above_noise() {
        // too few tiles to register, so this is the fallback; flicker of 4 percent is above the limit, flat brightness is not
        let flicker: Vec<f64> = (0..16).map(|k| if k % 2 == 0 { 0.98 } else { 1.02 }).collect();
        let r = run(Spec { gains: flicker, ..spec() }, 3);
        let Outcome::Fallback { plain_gains, .. } = r.outcome else { panic!() };
        assert!(plain_gains.is_some_and(|g| g.significant()));
        let r = run(spec(), 3);
        let Outcome::Fallback { plain_gains, .. } = r.outcome else { panic!() };
        assert!(plain_gains.is_none(), "flat brightness: the plain merge stays PR A's");
    }

    #[test]
    fn a_measurement_that_predicts_clearly_worse_than_the_table_refuses_the_series() {
        // the camera followed the table; the measurement is off by 0.6 sites in alternating directions
        let sp = Spec { positions: Some(nominal_positions()), ..spec() };
        let m = mosaics(&sp).unwrap();
        let mut s = green_series(&sp).unwrap();
        let wrong = |s: &GreenSeries, t: &[Tile], start: Start, o: &Options| {
            let mut fit = measure_series(s, t, start, o)?;
            for tr in &mut fit.tiles {
                for (k, r) in tr.rel.iter_mut().enumerate().skip(1) {
                    r[0] += 0.6 * if k % 2 == 0 { 1.0 } else { -1.0 };
                    r[1] += 0.6 * if k % 3 == 0 { 1.0 } else { -1.0 };
                }
            }
            Ok(fit)
        };
        let r = register_with(&mut s, &m, &tiles(10), &RegisterOptions::default(), None, &wrong);
        let Outcome::Fallback { reason: FallbackReason::MeasuredWorse(g), plain_gains } = r.outcome else { panic!("{:?}", r.outcome) };
        assert_eq!(g.verdict, GateVerdict::Worse, "{g:?}");
        assert!(plain_gains.is_none(), "flat brightness: the plain merge stays PR A's");
        assert!(r.timings.measure_ms > 0.0 && r.timings.gate_ms > 0.0);
    }

    #[test]
    fn a_series_of_the_wrong_length_is_refused() {
        let spec = Spec { frames: 8, ..spec() };
        let r = run(spec, 8);
        let Outcome::Fallback { reason, .. } = r.outcome else { panic!() };
        assert_eq!(reason, FallbackReason::FrameCount(8));
    }
}
