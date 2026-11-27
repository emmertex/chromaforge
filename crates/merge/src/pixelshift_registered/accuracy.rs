//! Accuracy of the measurement on synthetic series with known positions: the final positions after refinement, the
//! bias over fractional shifts, drift, rotation and scale, early stop and the quick nominal check. The phase
//! correlation of the coarse start is held to a much coarser tolerance in its own tests (`fft`).

use super::coarse::nominal_positions;
use super::measure::{CONVERGED, EarlyStop, Kernel, measure_tile};
use super::positions::{Options, Start, measure_series, quick_nominal_check};
use super::series::GreenSeries;
use super::synth::{Spec, series};
use super::tiles::Tile;

const SIZE: usize = 800;

fn tile(xc: i64, yc: i64) -> Tile {
    Tile { xc, yc, rx: xc as f64 - SIZE as f64 / 2.0, ry: yc as f64 - SIZE as f64 / 2.0 }
}

/// Tiles of an 800 x 800 series (green sites have x + y odd): all further than 180 sites from the edge.
fn tiles(n: usize) -> Vec<Tile> {
    [(301, 300), (501, 300), (301, 500), (501, 500), (401, 400), (601, 400), (401, 250), (401, 550)]
        .iter()
        .take(n)
        .map(|&(x, y)| tile(x, y))
        .collect()
}

fn truth(dev: impl Fn(usize) -> [f64; 2]) -> Vec<[f64; 2]> {
    nominal_positions().iter().enumerate().map(|(k, n)| [n[0] + dev(k)[0], n[1] + dev(k)[1]]).collect()
}

fn synth(positions: Vec<[f64; 2]>, noise_free: bool, models: Vec<[f64; 2]>) -> GreenSeries {
    series(&Spec { aw: SIZE, ah: SIZE, frames: 16, positions: Some(positions), noise_free, models, ..Spec::default() }).unwrap()
}

/// Positions of frames against frame 0.
fn rel(p: &[[f64; 2]]) -> Vec<[f64; 2]> {
    p.iter().map(|q| [q[0] - p[0][0], q[1] - p[0][1]]).collect()
}

fn worst(a: &[[f64; 2]], b: &[[f64; 2]]) -> f64 {
    a.iter().zip(b).map(|(x, y)| (x[0] - y[0]).abs().max((x[1] - y[1]).abs())).fold(0.0, f64::max)
}

#[test]
fn fractional_shifts_from_zero_to_one_and_a_quarter_pixels_are_measured_without_bias() {
    // frames 1..=11 deviate from the table by 0, 1/8, ..., 10/8 sites in x and by 10/8, ..., 0 in y
    let pos = truth(|k| if (1..=11).contains(&k) { [(k - 1) as f64 / 8.0, (11 - k) as f64 / 8.0] } else { [0.0, 0.0] });
    let s = synth(pos.clone(), true, Vec::new());
    // start half way between the table and the truth (a coarse start is good to about a site)
    let init: Vec<[f64; 2]> = nominal_positions().iter().zip(&pos).map(|(n, p)| [(n[0] + p[0]) / 2.0, (n[1] + p[1]) / 2.0]).collect();
    let fit = measure_tile(&s, &tile(401, 400), &init, 7, &Kernel::normal(16), None);
    assert!(fit.converged(), "last update {} after {} rounds", fit.last_update, fit.rounds_run);
    let got = fit.relative().unwrap();
    let err: Vec<f64> = got.iter().zip(rel(&pos)).map(|(g, t)| (g[0] - t[0]).abs().max((g[1] - t[1]).abs())).collect();
    let w = err.iter().cloned().fold(0.0, f64::max);
    assert!(w < 0.01, "worst position error {w:.4} sites; per frame {err:.4?}");
}

#[test]
fn the_same_holds_with_the_noise_of_the_camera() {
    let pos = truth(|k| if (1..=11).contains(&k) { [(k - 1) as f64 / 8.0, (11 - k) as f64 / 8.0] } else { [0.0, 0.0] });
    let s = synth(pos.clone(), false, Vec::new());
    let init: Vec<[f64; 2]> = nominal_positions().iter().zip(&pos).map(|(n, p)| [(n[0] + p[0]) / 2.0, (n[1] + p[1]) / 2.0]).collect();
    let fit = measure_tile(&s, &tile(401, 400), &init, 7, &Kernel::normal(16), None);
    assert!(fit.converged());
    let w = worst(&fit.relative().unwrap(), &rel(&pos));
    assert!(w < 0.015, "worst position error with noise {w:.4} sites");
}

#[test]
fn rotation_and_scale_are_recovered_across_the_tiles() {
    // frames 1..=15 each rotate by up to 1.5e-3 rad and scale by up to 1e-3 about the middle (0.3 sites at 200 sites)
    let models: Vec<[f64; 2]> = (0..16)
        .map(|k| [1.5e-3 * (k as f64 / 15.0) * if k % 2 == 0 { 1.0 } else { -1.0 }, 1.0e-3 * (k as f64 / 15.0) * if k % 3 == 0 { -1.0 } else { 1.0 }])
        .collect();
    let pos = truth(|k| [0.05 * (k % 3) as f64, -0.04 * (k % 4) as f64]);
    let s = synth(pos.clone(), true, models.clone());
    let fit = measure_series(&s, &tiles(8), Start::Nominal, &Options::default()).unwrap();
    assert_eq!(fit.dropped, 0, "{} of {} tiles dropped", fit.dropped, fit.usable);
    for k in 1..16 {
        let (m, t) = (fit.models[k], models[k]);
        // frame 0 is rotated and scaled too, so the measured model is the difference
        let (dt, ds) = (t[0] - models[0][0], t[1] - models[0][1]);
        assert!((m.theta - dt).abs() < 1.5e-4, "frame {k}: rotation {} against {dt}", m.theta);
        assert!((m.scale - ds).abs() < 1.5e-4, "frame {k}: scale {} against {ds}", m.scale);
    }
    assert!(fit.scatter_worst < 0.03, "scatter {}", fit.scatter_worst);
}

#[test]
fn drift_of_a_dozen_sites_is_followed_from_the_coarse_start() {
    // the camera drifts by (0.7, -0.8) sites per frame: 10.5 and -12 sites over the series
    let pos = truth(|k| [0.7 * k as f64, -0.8 * k as f64]);
    let s = synth(pos.clone(), true, Vec::new());
    let fit = measure_series(&s, &tiles(8), Start::Coarse, &Options::default()).unwrap();
    assert_eq!(fit.start, Start::Coarse);
    let w = worst(&fit.positions, &rel(&pos));
    assert!(w < 0.03, "worst position error {w:.4} sites; {} of {} tiles dropped", fit.dropped, fit.usable);
    assert!(fit.largest_displacement > 10.0, "displacement {}", fit.largest_displacement);
}

#[test]
fn a_tile_that_does_not_settle_is_given_up_early_and_a_settled_one_is_not_touched() {
    let pos = truth(|k| [0.03 * (k % 5) as f64, 0.02 * (k % 3) as f64]);
    let s = synth(pos.clone(), true, Vec::new());
    let (a, b) = (
        measure_tile(&s, &tile(401, 400), &nominal_positions(), 7, &Kernel::normal(16), None),
        measure_tile(&s, &tile(401, 400), &nominal_positions(), 7, &Kernel::normal(16), Some(EarlyStop::default())),
    );
    assert!(a.converged() && b.converged());
    assert_eq!(a.positions, b.positions, "early stop must not change a settled tile");
    assert!(!b.stopped_early);
    // a start with every other frame 2.5 sites off is outside the capture range of the normal kernel: the frames fail, no panic
    let far: Vec<[f64; 2]> = nominal_positions().iter().enumerate().map(|(k, n)| [n[0] + 2.5 * (k % 2) as f64, n[1]]).collect();
    let lost = measure_tile(&s, &tile(401, 400), &far, 7, &Kernel::normal(16), Some(EarlyStop::default()));
    assert!(!lost.converged());
    // heavy noise: nothing to settle on
    let noisy = series(&Spec { aw: SIZE, ah: SIZE, frames: 16, contrast: 0.02, level: 300.0, positions: Some(pos), ..Spec::default() }).unwrap();
    let n = measure_tile(&noisy, &tile(401, 400), &nominal_positions(), 7, &Kernel::normal(16), Some(EarlyStop::default()));
    assert!(!n.converged() || n.last_update <= CONVERGED);
}

#[test]
fn the_quick_check_passes_near_the_table_and_fails_far_from_it() {
    let near = synth(truth(|k| [0.04 * (k % 4) as f64, -0.03 * (k % 5) as f64]), true, Vec::new());
    let q = quick_nominal_check(&near, &tiles(8));
    assert_eq!((q.measured, q.passes), (8, true), "{q:?}");
    let far = synth(truth(|k| [3.0 * (k % 2) as f64, 3.0 * ((k / 2) % 2) as f64]), true, Vec::new());
    let q = quick_nominal_check(&far, &tiles(8));
    assert!(!q.passes && q.converged < 6, "{q:?}");
    // too few tiles: cannot pass
    assert!(!quick_nominal_check(&near, &tiles(3)).passes);
}

#[test]
fn hostile_input_gives_errors_or_failed_tiles_not_panics() {
    let one = series(&Spec { aw: SIZE, ah: SIZE, frames: 1, ..Spec::default() }).unwrap();
    assert!(measure_series(&one, &tiles(4), Start::Coarse, &Options::default()).is_err());
    let s = synth(truth(|_| [0.0, 0.0]), true, Vec::new());
    assert!(measure_series(&s, &[], Start::Nominal, &Options::default()).is_err());
    // a tile whose reference region leaves the area, and a start with the wrong number of frames or NaN
    let edge = measure_tile(&s, &tile(30, 31), &nominal_positions(), 3, &Kernel::normal(16), None);
    assert!(!edge.complete());
    let short = measure_tile(&s, &tile(401, 400), &nominal_positions()[..5], 3, &Kernel::normal(16), None);
    assert!(!short.complete());
    let mut nan = nominal_positions();
    nan[3] = [f64::NAN, 0.0];
    assert!(!measure_tile(&s, &tile(401, 400), &nan, 3, &Kernel::normal(16), None).complete());
    // a series of 8 frames cannot start from the 16-shot table
    let eight = series(&Spec { aw: SIZE, ah: SIZE, frames: 8, ..Spec::default() }).unwrap();
    assert!(measure_series(&eight, &tiles(4), Start::Nominal, &Options::default()).is_err());
    // flat scene: no tile measures
    let flat = series(&Spec { aw: SIZE, ah: SIZE, frames: 16, flat: true, ..Spec::default() }).unwrap();
    assert!(measure_series(&flat, &tiles(4), Start::Nominal, &Options::default()).is_err());
}
