//! The gate: do the measured positions predict the data better than the nominal table?
//!
//! A few textured tiles are held out of the position model. For each held-out tile and each frame, the green samples of
//! that frame inside the tile are predicted from the other frames, placed on the half-site node grid (nearest node, as
//! PR A places them) once at the nominal positions and once at the positions of the model fitted without the held-out
//! tiles; the prediction at the frame's own position is the cubic interpolation of the placement (Keys 1981). Per tile
//! the log of the ratio of the two squared errors is formed; a one-sided t test over the held-out tiles (Student 1908)
//! says whether the measured positions are better, and the energy gain says by how much.
//!
//! Verdicts: `Better` (t above the one-sided 95 % point and gain at least 1 %), `Worse` (the same margins the other way)
//! and `Equal` (no measurable difference). Registration goes ahead on `Better` and `Equal`; it is refused only on `Worse`.

use super::model::{self, Similarity};
use super::positions::TileTrack;
use super::series::GreenSeries;
use super::tiles::Tile;
use rayon::prelude::*;

/// Side of the placement box in nodes (the box covers 160 sites around the tile centre).
const BOX: usize = 320;
/// Smallest energy gain (as a fraction) for a verdict other than `Equal`.
pub const GAIN_MIN: f64 = 0.01;
/// Fewest and most held-out tiles.
const HOLD_MIN: usize = 3;
const HOLD_MAX: usize = 12;
/// Fewest tiles for a gate at all.
pub const MIN_TRACKS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateVerdict {
    Better,
    Equal,
    Worse,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GateResult {
    pub verdict: GateVerdict,
    pub holdout: usize,
    /// rms prediction error (DN) with the nominal and with the measured positions.
    pub e_nom: f64,
    pub e_meas: f64,
    /// (E_nom - E_meas) / E_nom in energy.
    pub gain: f64,
    pub t: f64,
    pub t_crit: f64,
}

/// One-sided 95 % point of Student's t with `k - 1` degrees of freedom.
pub fn t_crit(k: usize) -> f64 {
    [f64::INFINITY, f64::INFINITY, 6.314, 2.920, 2.353, 2.132, 2.015, 1.943, 1.895, 1.860, 1.833, 1.812, 1.796, 1.782][k.min(13)]
}

/// Catmull-Rom interpolation of a node array at fractional node coordinates.
fn cubic_at(a: &[f32], w: usize, h: usize, x: f32, y: f32) -> Option<f32> {
    let (ix, iy) = (x.floor() as i64, y.floor() as i64);
    if ix < 1 || iy < 1 || ix + 2 >= w as i64 || iy + 2 >= h as i64 {
        return None;
    }
    let (fx, fy) = (x - ix as f32, y - iy as f32);
    let k = |t: f32| -> [f32; 4] {
        [
            -0.5 * t * t * t + t * t - 0.5 * t,
            1.5 * t * t * t - 2.5 * t * t + 1.0,
            -1.5 * t * t * t + 2.0 * t * t + 0.5 * t,
            0.5 * t * t * t - 0.5 * t * t,
        ]
    };
    let (kx, ky) = (k(fx), k(fy));
    let mut v = 0.0;
    for (j, wy) in ky.iter().enumerate() {
        for (i, wx) in kx.iter().enumerate() {
            let p = *a.get((iy as usize + j - 1) * w + ix as usize + i - 1)?;
            if p.is_nan() {
                return None;
            }
            v += p * wx * wy;
        }
    }
    Some(v)
}

/// The green samples of all frames but `skip` placed on the node grid of the box with origin `(x0, y0)` (nodes), each
/// frame at the node nearest to its position `pos`; a node holds the rounded-down mean of the samples that land on it,
/// holes (value 0) are filled from their neighbours.
fn place_green(s: &GreenSeries, skip: usize, pos: &[[f64; 2]], x0: i64, y0: i64) -> Vec<f32> {
    let mut sum = vec![0f32; BOX * BOX];
    let mut cnt = vec![0u8; BOX * BOX];
    for (q, p) in pos.iter().enumerate() {
        if q == skip {
            continue;
        }
        let (dx, dy) = ((2.0 * p[0]).round() as i64, (2.0 * p[1]).round() as i64);
        for ny in y0..y0 + BOX as i64 {
            let ey = ny - dy;
            if ey.rem_euclid(2) != 0 {
                continue;
            }
            let mut nx = x0 + (x0 - dx).rem_euclid(2);
            while nx < x0 + BOX as i64 {
                if let Some(v) = s.g(q, (nx - dx) / 2, ey / 2) {
                    let i = (ny - y0) as usize * BOX + (nx - x0) as usize;
                    if let (Some(a), Some(c)) = (sum.get_mut(i), cnt.get_mut(i)) {
                        *a += v as f32;
                        *c = c.saturating_add(1);
                    }
                }
                nx += 2;
            }
        }
    }
    let mut out: Vec<f32> = sum.iter().zip(&cnt).map(|(a, c)| if *c > 0 { (a / f32::from(*c)).floor() } else { 0.0 }).collect();
    for _ in 0..6 {
        let prev = out.clone();
        for j in 1..BOX - 1 {
            for i in 1..BOX - 1 {
                if prev.get(j * BOX + i).copied() == Some(0.0) {
                    let nb: Vec<f32> = [prev[j * BOX + i - 1], prev[j * BOX + i + 1], prev[(j - 1) * BOX + i], prev[(j + 1) * BOX + i]]
                        .into_iter()
                        .filter(|v| *v != 0.0)
                        .collect();
                    if !nb.is_empty() {
                        out[j * BOX + i] = nb.iter().sum::<f32>() / nb.len() as f32;
                    }
                }
            }
        }
    }
    out
}

/// Squared prediction errors over one held-out tile: `(sum with nominal positions, sum with measured, samples)`.
fn tile_errors(s: &GreenSeries, tile: &Tile, models: &[Similarity], nom: &[[f64; 2]]) -> (f64, f64, f64) {
    let (x0, y0) = (2 * (tile.xc - 80), 2 * (tile.yc - 80));
    let meas_pos: Vec<[f64; 2]> = models.iter().map(|m| m.at_tile(tile)).collect();
    let (hx, hy) = (s.aw as f64 / 2.0, s.ah as f64 / 2.0);
    let (mut sn, mut sm, mut cnt) = (0.0f64, 0.0f64, 0.0f64);
    for f in 0..s.frame_count() {
        let (Some(nf), Some(mf)) = (nom.get(f), meas_pos.get(f)) else { continue };
        let ga = place_green(s, f, nom, x0, y0);
        let gb = place_green(s, f, &meas_pos, x0, y0);
        let (xa, xb) = (((x0 as f64 / 2.0) - mf[0]).ceil() as i64 + 3, ((x0 as f64 / 2.0) + BOX as f64 / 2.0 - mf[0]).floor() as i64 - 3);
        let (ya, yb) = (((y0 as f64 / 2.0) - mf[1]).ceil() as i64 + 3, ((y0 as f64 / 2.0) + BOX as f64 / 2.0 - mf[1]).floor() as i64 - 3);
        let Some(mdl) = models.get(f) else { continue };
        for y in ya..=yb {
            for x in xa..=xb {
                let Some(v) = s.g(f, x, y) else { continue };
                let d = mdl.at(x as f64 - hx, y as f64 - hy);
                let (qx, qy) = (2.0 * (x as f64 + d[0]) - x0 as f64, 2.0 * (y as f64 + d[1]) - y0 as f64);
                let (nx, ny) = (2.0 * (x as f64 + nf[0]) - x0 as f64, 2.0 * (y as f64 + nf[1]) - y0 as f64);
                let (Some(pa), Some(pb)) = (cubic_at(&ga, BOX, BOX, nx as f32, ny as f32), cubic_at(&gb, BOX, BOX, qx as f32, qy as f32)) else {
                    continue;
                };
                let v = v as f32;
                sn += f64::from((pa - v) * (pa - v));
                sm += f64::from((pb - v) * (pb - v));
                cnt += 1.0;
            }
        }
    }
    (sn, sm, cnt)
}

/// The gate for the tiles of a measurement. `None` for fewer than [`MIN_TRACKS`] tiles or tracks that do not match the series.
pub fn gate(s: &GreenSeries, tracks: &[TileTrack], nominal: &[[f64; 2]]) -> Option<GateResult> {
    let nt = tracks.len();
    let n = s.frame_count();
    if nt < MIN_TRACKS || nominal.len() != n || tracks.iter().any(|t| t.rel.len() != n) {
        return None;
    }
    let k = (nt / 4).clamp(HOLD_MIN, HOLD_MAX).min(nt - 1);
    let hold: Vec<usize> = (0..k).map(|i| ((i * nt) / k + nt / (2 * k)).min(nt - 1)).collect();
    // the model of the other tiles
    let mut models = vec![Similarity::default(); n];
    for (f, m) in models.iter_mut().enumerate().skip(1) {
        let rows: Vec<(Tile, [f64; 2])> =
            tracks.iter().enumerate().filter(|(i, _)| !hold.contains(i)).filter_map(|(_, t)| Some((t.tile, *t.rel.get(f)?))).collect();
        *m = model::fit(&rows)?;
    }
    let per_tile: Vec<(f64, f64, f64)> = hold.par_iter().filter_map(|&i| tracks.get(i)).map(|t| tile_errors(s, &t.tile, &models, nominal)).collect();
    let (tot_n, tot_m): (f64, f64) = (per_tile.iter().map(|p| p.0).sum(), per_tile.iter().map(|p| p.1).sum());
    let tot_c = per_tile.iter().map(|p| p.2).sum::<f64>().max(1.0);
    let d: Vec<f64> = per_tile.iter().map(|p| (p.0.max(1e-9) / p.1.max(1e-9)).ln()).collect();
    if d.len() < 2 {
        return None;
    }
    let md = d.iter().sum::<f64>() / d.len() as f64;
    let sd = (d.iter().map(|x| (x - md).powi(2)).sum::<f64>() / (d.len() as f64 - 1.0)).sqrt();
    let t = md / (sd / (d.len() as f64).sqrt()).max(1e-9);
    let tc = t_crit(d.len());
    let gain = (tot_n - tot_m) / tot_n.max(1e-9);
    let verdict = if t >= tc && gain >= GAIN_MIN {
        GateVerdict::Better
    } else if t <= -tc && gain <= -GAIN_MIN {
        GateVerdict::Worse
    } else {
        GateVerdict::Equal
    };
    Some(GateResult { verdict, holdout: d.len(), e_nom: (tot_n / tot_c).sqrt(), e_meas: (tot_m / tot_c).sqrt(), gain, t, t_crit: tc })
}

#[cfg(test)]
mod tests {
    use super::super::coarse::nominal_positions;
    use super::super::synth::{Spec, series};
    use super::*;

    const SIZE: usize = 800;

    fn tiles() -> Vec<Tile> {
        [(301, 300), (501, 300), (301, 500), (501, 500), (401, 400), (601, 400), (401, 250), (401, 550)]
            .iter()
            .map(|&(x, y)| Tile { xc: x, yc: y, rx: x as f64 - SIZE as f64 / 2.0, ry: y as f64 - SIZE as f64 / 2.0 })
            .collect()
    }

    /// A series whose frames sit at `truth`, and tracks that report `reported` positions (against frame 0) in every tile.
    fn setup(truth: &[[f64; 2]], reported: &[[f64; 2]]) -> (GreenSeries, Vec<TileTrack>) {
        let s = series(&Spec { aw: SIZE, ah: SIZE, frames: 16, positions: Some(truth.to_vec()), noise_free: false, ..Spec::default() }).unwrap();
        let rel: Vec<[f64; 2]> = reported.iter().map(|p| [p[0] - reported[0][0], p[1] - reported[0][1]]).collect();
        let tracks = tiles().into_iter().map(|tile| TileTrack { tile, rel: rel.clone(), last_update: 0.0 }).collect();
        (s, tracks)
    }

    fn deviated(f: impl Fn(usize) -> [f64; 2]) -> Vec<[f64; 2]> {
        nominal_positions().iter().enumerate().map(|(k, n)| [n[0] + f(k)[0], n[1] + f(k)[1]]).collect()
    }

    #[test]
    fn positions_that_describe_the_data_better_than_the_table_are_better() {
        let truth = deviated(|k| [0.25 * ((k * 3 % 7) as f64 / 6.0 - 0.5), 0.3 * ((k * 5 % 11) as f64 / 10.0 - 0.5)]);
        let (s, tracks) = setup(&truth, &truth);
        let g = gate(&s, &tracks, &nominal_positions()).unwrap();
        assert_eq!(g.verdict, GateVerdict::Better, "{g:?}");
        assert!(g.e_meas < g.e_nom && g.gain > GAIN_MIN);
        assert_eq!(g.holdout, 3, "eight tiles hold out three");
    }

    #[test]
    fn positions_equal_to_the_table_on_a_series_that_followed_it_are_equal() {
        let truth = nominal_positions();
        let (s, tracks) = setup(&truth, &truth);
        let g = gate(&s, &tracks, &nominal_positions()).unwrap();
        assert_eq!(g.verdict, GateVerdict::Equal, "{g:?}");
        assert!(g.gain.abs() < 1e-9);
    }

    #[test]
    fn positions_that_are_clearly_worse_than_the_table_are_refused() {
        // the camera followed the table; the "measurement" is off by 0.6 sites in alternating directions
        let truth = nominal_positions();
        let bad = deviated(|k| [0.6 * if k % 2 == 0 { 1.0 } else { -1.0 }, 0.6 * if k % 3 == 0 { 1.0 } else { -1.0 }]);
        let (s, tracks) = setup(&truth, &bad);
        let g = gate(&s, &tracks, &nominal_positions()).unwrap();
        assert_eq!(g.verdict, GateVerdict::Worse, "{g:?}");
        assert!(g.e_meas > g.e_nom && g.gain < -GAIN_MIN);
    }

    #[test]
    fn hostile_input_gives_none() {
        let truth = nominal_positions();
        let (s, tracks) = setup(&truth, &truth);
        assert!(gate(&s, &tracks[..3], &nominal_positions()).is_none(), "fewer than four tiles");
        assert!(gate(&s, &tracks, &nominal_positions()[..5]).is_none(), "table of the wrong length");
        let mut short = tracks.clone();
        short[0].rel.truncate(4);
        assert!(gate(&s, &short, &nominal_positions()).is_none(), "a track with too few frames");
        assert!(t_crit(0).is_infinite() && t_crit(99) == t_crit(13) && t_crit(8) == 1.895);
    }
}
