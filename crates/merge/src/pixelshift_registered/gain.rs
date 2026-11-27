//! Brightness of the frames: one gain per frame and colour.
//!
//! Two estimates. The **global** gain of a frame is its mean level over the central half of the active area against the
//! mean over the frames; it is available before any position is known, so a series whose light changes by many percent
//! can still be measured (the green planes are normalised before the measurement), and it is what the plain merge uses
//! when registration is refused. The **per-tile** gain, after the measurement, takes a Gaussian-weighted mean level at
//! the same scene point in every frame in each textured tile and combines the tiles robustly (tiles whose gains stray
//! from the median are left out), so that a local change of the scene does not pass for a change of exposure.
//!
//! A gain below noise is not applied: [`Gains::significant`] is true when the gains of any colour vary by more than
//! [`SD_LIMIT`] between frames (0.5 %).

use super::model::Similarity;
use super::samples::Samples;
use super::series::GreenSeries;
use super::tiles::Tile;
use rayon::prelude::*;

/// Standard deviation over frames (as a fraction) above which the gains are applied.
pub const SD_LIMIT: f64 = 0.005;
/// Samples at or above this fraction of white are clipped and do not enter a mean (16000 of 16383).
const CLIP_FRACTION: f64 = 0.9766;
/// Radius and Gaussian scale (sites) of the per-tile mean.
const TILE_RADIUS: i64 = 75;
const TILE_SIGMA: f64 = 30.0;
/// Tiles with a mean level below this (DN above black) are not used for the per-tile gain.
const MIN_LEVEL: f64 = 30.0;
/// Fewest tiles for a per-tile gain, and the outlier limit in robust standard deviations.
const MIN_TILES: usize = 6;
const OUTLIER_SD: f64 = 3.0;

/// Gain of each frame and colour (mean 1 over the frames per colour): the factor a frame's level is above the average.
#[derive(Debug, Clone, PartialEq)]
pub struct Gains {
    pub per_frame: Vec<[f64; 3]>,
}

impl Gains {
    pub fn identity(n: usize) -> Gains {
        Gains { per_frame: vec![[1.0; 3]; n] }
    }

    /// Standard deviation over frames of the gain of colour `c`.
    pub fn sd(&self, c: usize) -> f64 {
        let n = self.per_frame.len().max(1) as f64;
        let m = self.per_frame.iter().map(|g| g[c]).sum::<f64>() / n;
        (self.per_frame.iter().map(|g| (g[c] - m).powi(2)).sum::<f64>() / n).sqrt()
    }

    pub fn max_sd(&self) -> f64 {
        (0..3).map(|c| self.sd(c)).fold(0.0, f64::max)
    }

    /// True if the gains vary by more than [`SD_LIMIT`] in some colour.
    pub fn significant(&self) -> bool {
        self.max_sd() > SD_LIMIT
    }

    /// The value of a raw sample of frame `k` and colour `c` on the common level.
    pub fn apply(&self, k: usize, c: usize, v: f64, black: f64) -> f64 {
        let g = self.per_frame.get(k).and_then(|g| g.get(c)).copied().unwrap_or(1.0);
        black + (v - black) / g
    }
}

fn clip_level(white: f64) -> f64 {
    white * CLIP_FRACTION
}

/// Mean levels per frame and colour over the central half of the active area, as gains. `None` if a frame or colour
/// has no usable sample or the series has fewer than two frames.
pub fn global_gains(samples: &dyn Samples) -> Option<Gains> {
    let n = samples.frame_count();
    if n < 2 {
        return None;
    }
    let (aw, ah) = samples.size();
    let (x0, x1, y0, y1) = ((aw / 4) as i64, (3 * aw / 4) as i64, (ah / 4) as i64, (3 * ah / 4) as i64);
    let (black, clip) = (samples.black(), clip_level(samples.white()));
    let means: Vec<Option<[f64; 3]>> = (0..n)
        .into_par_iter()
        .map(|k| {
            let (mut sum, mut cnt) = ([0.0f64; 3], [0.0f64; 3]);
            for y in y0..y1 {
                for x in x0..x1 {
                    if let Some((c, v)) = samples.sample(k, x, y) {
                        let v = f64::from(v);
                        if v < clip {
                            sum[c as usize % 3] += v - black;
                            cnt[c as usize % 3] += 1.0;
                        }
                    }
                }
            }
            (cnt.iter().all(|c| *c > 0.0)).then(|| [sum[0] / cnt[0], sum[1] / cnt[1], sum[2] / cnt[2]])
        })
        .collect();
    let means: Option<Vec<[f64; 3]>> = means.into_iter().collect();
    let means = means?;
    let mut per_frame = vec![[1.0; 3]; n];
    for c in 0..3 {
        let avg = means.iter().map(|m| m[c]).sum::<f64>() / n as f64;
        if avg.is_nan() || avg <= MIN_LEVEL {
            return None;
        }
        for (g, m) in per_frame.iter_mut().zip(&means) {
            g[c] = m[c] / avg;
        }
    }
    Some(Gains { per_frame })
}

/// Puts the green planes of the frames on a common level (in place): frame `k` is divided by its green gain.
pub fn normalise_green(series: &mut GreenSeries, gains: &Gains) {
    let black = series.black;
    series.map_frames(|k, v| {
        let g = gains.per_frame.get(k).map_or(1.0, |g| g[1]);
        (black + (f64::from(v) - black) / g).round().clamp(0.0, 65535.0) as u16
    });
}

/// What the per-tile estimate used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileGainReport {
    pub tiles: usize,
    pub used: usize,
    pub skipped_clipped: usize,
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    v.get(v.len() / 2).copied().unwrap_or(f64::NAN)
}

/// Robust per-tile gains from the tiles of the measurement: `models[k]` is the displacement model of frame `k` against
/// frame 0. `None` if fewer than six tiles are usable.
pub fn tile_gains(samples: &dyn Samples, tiles: &[Tile], models: &[Similarity]) -> Option<(Gains, TileGainReport)> {
    let n = samples.frame_count();
    if n < 2 || models.len() != n {
        return None;
    }
    let (black, clip) = (samples.black(), clip_level(samples.white()));
    let per_tile: Vec<Option<Vec<[f64; 3]>>> = tiles
        .par_iter()
        .map(|t| {
            let mut m = vec![[0.0f64; 3]; n];
            for (k, mk) in m.iter_mut().enumerate() {
                let d = models.get(k).map_or([0.0, 0.0], |s| s.at_tile(t));
                let (cx, cy) = ((t.xc as f64 - d[0]).round() as i64, (t.yc as f64 - d[1]).round() as i64);
                let (mut num, mut den) = ([0.0f64; 3], [0.0f64; 3]);
                for y in cy - TILE_RADIUS..=cy + TILE_RADIUS {
                    for x in cx - TILE_RADIUS..=cx + TILE_RADIUS {
                        let Some((c, v)) = samples.sample(k, x, y) else { continue };
                        let v = f64::from(v);
                        if v >= clip {
                            return None;
                        }
                        let w = (-(((x - cx) * (x - cx) + (y - cy) * (y - cy)) as f64) / (2.0 * TILE_SIGMA * TILE_SIGMA)).exp();
                        num[c as usize % 3] += w * (v - black);
                        den[c as usize % 3] += w;
                    }
                }
                for c in 0..3 {
                    mk[c] = num[c] / den[c].max(1e-9);
                }
            }
            let mut g = vec![[1.0f64; 3]; n];
            for c in 0..3 {
                let mean = m.iter().map(|x| x[c]).sum::<f64>() / n as f64;
                if mean.is_nan() || mean < MIN_LEVEL {
                    return None;
                }
                for (gk, mk) in g.iter_mut().zip(&m) {
                    gk[c] = mk[c] / mean;
                }
            }
            Some(g)
        })
        .collect();
    let skipped_clipped = per_tile.iter().filter(|g| g.is_none()).count();
    let usable: Vec<Vec<[f64; 3]>> = per_tile.into_iter().flatten().collect();
    if usable.len() < MIN_TILES {
        return None;
    }
    let mut med = vec![[0.0f64; 3]; n];
    for (k, mk) in med.iter_mut().enumerate() {
        for c in 0..3 {
            mk[c] = median(&mut usable.iter().map(|g| g[k][c]).collect::<Vec<_>>());
        }
    }
    let dev: Vec<f64> = usable
        .iter()
        .map(|g| ((0..n).flat_map(|k| (0..3).map(move |c| (k, c))).map(|(k, c)| (g[k][c] - med[k][c]).powi(2)).sum::<f64>() / (3 * n) as f64).sqrt())
        .collect();
    let md = median(&mut dev.clone());
    let mad = median(&mut dev.iter().map(|d| (d - md).abs()).collect::<Vec<_>>());
    let lim = md + OUTLIER_SD * 1.4826 * mad;
    let mut keep: Vec<usize> = (0..usable.len()).filter(|&i| dev.get(i).is_some_and(|d| *d <= lim)).collect();
    if keep.len() < MIN_TILES {
        keep = (0..usable.len()).collect();
    }
    let mut per_frame = vec![[1.0f64; 3]; n];
    for c in 0..3 {
        for (k, g) in per_frame.iter_mut().enumerate() {
            g[c] = keep.iter().filter_map(|&i| usable.get(i).and_then(|u| u.get(k)).map(|x| x[c])).sum::<f64>() / keep.len() as f64;
        }
        let mean = per_frame.iter().map(|g| g[c]).sum::<f64>() / n as f64;
        for g in per_frame.iter_mut() {
            g[c] /= mean;
        }
    }
    Some((Gains { per_frame }, TileGainReport { tiles: tiles.len(), used: keep.len(), skipped_clipped }))
}

#[cfg(test)]
mod tests {
    use super::super::coarse::nominal_positions;
    use super::super::synth::{Spec, mosaics, series};
    use super::*;

    #[test]
    fn gains_apply_and_report_their_spread() {
        let g = Gains { per_frame: vec![[1.01, 1.0, 0.99], [0.99, 1.0, 1.01], [1.0; 3], [1.0; 3]] };
        assert!(g.sd(1) < 1e-12);
        assert!((g.sd(0) - 0.01 * 0.5f64.sqrt()).abs() < 1e-9, "{}", g.sd(0));
        assert!(Gains::identity(16).max_sd() == 0.0 && !Gains::identity(16).significant());
        assert!(g.significant() == (g.max_sd() > SD_LIMIT));
        assert!((g.apply(0, 0, 2008.0, 1008.0) - (1008.0 + 1000.0 / 1.01)).abs() < 1e-9);
        assert_eq!(g.apply(9, 0, 2008.0, 1008.0), 2008.0, "an unknown frame is left alone");
    }

    #[test]
    fn global_gains_follow_a_flicker_of_several_percent() {
        let gains: Vec<f64> = (0..16).map(|k| 1.0 + 0.05 * ((k * 7 % 16) as f64 / 15.0 - 0.5)).collect();
        let spec = Spec { aw: 800, ah: 800, frames: 16, gains: gains.clone(), ..Spec::default() };
        let m = mosaics(&spec).unwrap();
        let g = global_gains(&m).unwrap();
        let mean = gains.iter().sum::<f64>() / 16.0;
        for k in 0..16 {
            for c in 0..3 {
                assert!(
                    (g.per_frame[k][c] - gains[k] / mean).abs() < 0.003,
                    "frame {k} colour {c}: {} against {}",
                    g.per_frame[k][c],
                    gains[k] / mean
                );
            }
        }
        assert!(g.significant());
        // a flat-brightness series stays within noise
        let flat = global_gains(&mosaics(&Spec { aw: 800, ah: 800, frames: 16, ..Spec::default() }).unwrap()).unwrap();
        assert!(!flat.significant(), "sd {}", flat.max_sd());
    }

    #[test]
    fn normalising_the_green_planes_removes_the_flicker() {
        let gains: Vec<f64> = (0..16).map(|k| if k % 2 == 0 { 0.9 } else { 1.1 }).collect();
        let spec = Spec { aw: 800, ah: 800, frames: 16, gains, noise_free: true, ..Spec::default() };
        let m = mosaics(&spec).unwrap();
        let mut s = series(&spec).unwrap();
        let g = global_gains(&m).unwrap();
        let level = |s: &GreenSeries, k: usize| {
            (0..800i64).flat_map(|y| (0..800i64).map(move |x| (x, y))).filter_map(|(x, y)| s.g(k, x, y)).sum::<f64>() / (800.0 * 800.0 / 2.0)
        };
        let before = (level(&s, 1) - 1008.0) / (level(&s, 0) - 1008.0);
        assert!((before - 1.1 / 0.9).abs() < 0.02, "{before}");
        normalise_green(&mut s, &g);
        let after = (level(&s, 1) - 1008.0) / (level(&s, 0) - 1008.0);
        assert!((after - 1.0).abs() < 0.003, "levels after: {after}");
    }

    #[test]
    fn per_tile_gains_recover_the_flicker_and_leave_out_a_local_change() {
        let gains: Vec<f64> = (0..16).map(|k| 1.0 + 0.04 * (((k * 5) % 16) as f64 / 15.0 - 0.5)).collect();
        let spec =
            Spec { aw: 800, ah: 800, frames: 16, gains: gains.clone(), noise_free: true, positions: Some(nominal_positions()), ..Spec::default() };
        let m = mosaics(&spec).unwrap();
        let tiles: Vec<Tile> = [(301, 300), (501, 300), (301, 500), (501, 500), (401, 400), (601, 400), (401, 250), (401, 550)]
            .iter()
            .map(|&(x, y)| Tile { xc: x, yc: y, rx: x as f64 - 400.0, ry: y as f64 - 400.0 })
            .collect();
        // positions: the models of the true displacement of each frame against frame 0
        let nom = nominal_positions();
        let models: Vec<Similarity> = nom.iter().map(|n| Similarity { t: [n[0] - nom[0][0], n[1] - nom[0][1]], ..Similarity::default() }).collect();
        let (g, rep) = tile_gains(&m, &tiles, &models).unwrap();
        assert_eq!(rep.used, 8);
        let mean = gains.iter().sum::<f64>() / 16.0;
        for k in 0..16 {
            assert!((g.per_frame[k][1] - gains[k] / mean).abs() < 0.002, "frame {k}: {} against {}", g.per_frame[k][1], gains[k] / mean);
        }
        assert!(tile_gains(&m, &tiles[..3], &models).is_none(), "fewer than six tiles");
        assert!(tile_gains(&m, &tiles, &models[..5]).is_none(), "models for the wrong number of frames");
    }

    #[test]
    fn hostile_input_gives_none() {
        let one = mosaics(&Spec { aw: 400, ah: 400, frames: 1, ..Spec::default() }).unwrap();
        assert!(global_gains(&one).is_none());
        let dark = mosaics(&Spec { aw: 400, ah: 400, frames: 4, level: 5.0, flat: true, ..Spec::default() }).unwrap();
        assert!(global_gains(&dark).is_none(), "no level to compare");
    }
}
