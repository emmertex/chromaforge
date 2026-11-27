//! Starting positions for the measurement.
//!
//! The nominal table of a 16-shot series is a good start when the camera did what the table says (positions within
//! about a site). Otherwise the start comes from the data: phase correlation of every frame against frame 1 and
//! against the previous frame over up to 20 spread tiles (Kuglin and Hines, Proc. IEEE Int. Conf. on Cybernetics and
//! Society, 1975), the direct value kept if it agrees with the chain of consecutive values within 1.5 sites, else the
//! chain; then a wide first stage of the measurement (a blurred reference and a capture range of 3 sites, four
//! rounds on up to 24 tiles, twice) that fits a similarity model to the tile positions, so that every tile starts
//! from its own predicted positions.

use super::fft::{Fft2, phase_correlation_filtered};
use super::measure::Kernel;
use super::model::{self, Similarity};
use super::series::GreenSeries;
use super::tiles::{PLANE, Tile, diamond_plane};
use crate::pixelshift;

/// Tiles used for the phase-correlation start, and for the wide stage.
const COARSE_TILES: usize = 20;
const WIDE_TILES: usize = 24;
/// Rounds of one wide pass.
const WIDE_ROUNDS: usize = 4;
/// Low-pass of the phase correlation (cycles per plane sample, standard deviation of a Gaussian weight on the cross
/// spectrum): the camera-filled rows leave a static pattern at high frequencies that an unweighted correlation locks
/// onto (a peak at zero shift) when the scene has little energy there.
const COARSE_CUTOFF: f64 = 0.06;
/// The direct estimate is used if it agrees with the chain this closely (sites).
const CHAIN_AGREEMENT: f64 = 1.5;

/// Nominal position (sites) of each frame of a 16-shot series against frame 0, from PR A's table.
pub fn nominal_positions() -> Vec<[f64; 2]> {
    pixelshift::SHOT_16
        .shifts
        .iter()
        .map(|&(dx, dy)| [dx as f64 / pixelshift::SHOT_16.scale as f64, dy as f64 / pixelshift::SHOT_16.scale as f64])
        .collect()
}

/// A shift in plane samples (the 45-degree lattice) as a shift in sites.
pub fn plane_to_sites(d: [f64; 2]) -> [f64; 2] {
    [d[0] + d[1], d[0] - d[1]]
}

fn median(v: &mut [f64]) -> Option<f64> {
    v.sort_by(|a, b| a.total_cmp(b));
    v.get(v.len() / 2).copied()
}

/// Start positions of all frames against frame 0 from phase correlation (sites); `None` if no tile gives a plane or
/// the series has fewer than two frames.
pub fn phase_start(s: &GreenSeries, tiles: &[Tile]) -> Option<Vec<[f64; 2]>> {
    let n = s.frame_count();
    if n < 2 || tiles.is_empty() {
        return None;
    }
    let fft = Fft2::new(PLANE)?;
    let step = (tiles.len() / COARSE_TILES).max(1);
    let planes: Vec<Vec<Vec<f64>>> = tiles
        .iter()
        .step_by(step)
        .take(COARSE_TILES)
        .filter_map(|t| (0..n).map(|k| diamond_plane(s, k, t.xc, t.yc)).collect::<Option<Vec<_>>>())
        .collect();
    if planes.is_empty() {
        return None;
    }
    let shift = |a: usize, b: usize| -> Option<[f64; 2]> {
        let (mut vx, mut vy) = (Vec::new(), Vec::new());
        for p in &planes {
            if let Some(r) = phase_correlation_filtered(&fft, p.get(a)?, p.get(b)?, Some(COARSE_CUTOFF)) {
                let d = plane_to_sites(r.shift);
                vx.push(d[0]);
                vy.push(d[1]);
            }
        }
        Some([median(&mut vx)?, median(&mut vy)?])
    };
    let mut out = vec![[0.0, 0.0]];
    let mut chain = [0.0, 0.0];
    for k in 1..n {
        let st = shift(k - 1, k)?;
        chain = [chain[0] + st[0], chain[1] + st[1]];
        let d = shift(0, k)?;
        let use_direct = (d[0] - chain[0]).abs() <= CHAIN_AGREEMENT && (d[1] - chain[1]).abs() <= CHAIN_AGREEMENT;
        out.push(if use_direct { d } else { chain });
    }
    Some(out)
}

/// Start positions of one tile from a similarity model per frame (frame 0 at the origin).
pub fn tile_start(models: &[Similarity], t: &Tile) -> Vec<[f64; 2]> {
    models.iter().enumerate().map(|(k, m)| if k == 0 { [0.0, 0.0] } else { m.at_tile(t) }).collect()
}

/// Similarity model per frame fitted to first-stage tile positions (`stage[i]` belongs to `tiles[i]`); a frame with
/// fewer than four usable tiles keeps `fallback` as a pure translation.
fn fit_models(tiles: &[Tile], stage: &[Vec<Option<[f64; 2]>>], n: usize, fallback: &[[f64; 2]]) -> Vec<Similarity> {
    let mut out = vec![Similarity::default(); n];
    for k in 1..n {
        let rows: Vec<(Tile, [f64; 2])> = tiles
            .iter()
            .zip(stage)
            .filter_map(|(t, v)| {
                let (a, b) = ((*v.first()?)?, (*v.get(k)?)?);
                Some((*t, [b[0] - a[0], b[1] - a[1]]))
            })
            .collect();
        let fb = fallback.get(k).copied().unwrap_or([0.0, 0.0]);
        out[k] = if rows.len() < 4 {
            Similarity { t: fb, ..Similarity::default() }
        } else {
            model::fit(&rows).unwrap_or(Similarity { t: fb, ..Similarity::default() })
        };
    }
    out
}

/// Coarse start followed by the wide first stage. Returns the similarity model of every frame (frame 0 identity);
/// `None` if the phase correlation could not give a start.
pub fn wide_start(s: &GreenSeries, tiles: &[Tile]) -> Option<Vec<Similarity>> {
    let n = s.frame_count();
    let mut init = phase_start(s, tiles)?;
    let sub: Vec<Tile> = tiles.iter().step_by((tiles.len() / WIDE_TILES).max(1)).take(WIDE_TILES).copied().collect();
    let kernel = Kernel::wide(n);
    let mut models: Option<Vec<Similarity>> = None;
    for pass in 0..2 {
        let inits: Vec<Vec<[f64; 2]>> = sub
            .iter()
            .map(|t| match (&models, pass) {
                (Some(m), 1) => tile_start(m, t),
                _ => init.clone(),
            })
            .collect();
        let stage: Vec<Vec<Option<[f64; 2]>>> =
            super::measure::measure_tiles(s, &sub, &inits, WIDE_ROUNDS, &kernel, None, 6).into_iter().map(|f| f.positions).collect();
        // per-frame median displacement against frame 0 over the tiles that measured both
        let mut next = vec![[0.0f64, 0.0]; n];
        for k in 1..n {
            let (mut vx, mut vy) = (Vec::new(), Vec::new());
            for v in &stage {
                if let (Some(Some(a)), Some(Some(b))) = (v.first(), v.get(k)) {
                    vx.push(b[0] - a[0]);
                    vy.push(b[1] - a[1]);
                }
            }
            next[k] = if vx.len() >= 3 { [median(&mut vx)?, median(&mut vy)?] } else { init.get(k).copied().unwrap_or([0.0, 0.0]) };
        }
        models = Some(fit_models(&sub, &stage, n, &next));
        init = next;
    }
    models
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_nominal_table_is_pr_as_table_in_sites() {
        let n = nominal_positions();
        assert_eq!(n.len(), 16);
        assert_eq!(n[0], [0.0, 0.0]);
        assert_eq!(n[1], [1.0, 0.0]);
        assert_eq!(n[2], [1.0, 1.0]);
        assert_eq!(n[4], [0.5, 0.0]);
        assert_eq!(n[8], [0.5, 0.5]);
        assert_eq!(n[12], [0.0, 0.5]);
    }

    #[test]
    fn plane_shifts_convert_to_site_shifts() {
        assert_eq!(plane_to_sites([1.0, 0.0]), [1.0, 1.0]);
        assert_eq!(plane_to_sites([0.0, 1.0]), [1.0, -1.0]);
    }

    #[test]
    fn tile_start_places_frame_zero_at_the_origin() {
        let models = vec![Similarity::default(), Similarity { t: [1.0, 2.0], theta: 0.001, scale: 0.0 }];
        let t = Tile { xc: 0, yc: 0, rx: 1000.0, ry: 0.0 };
        let s = tile_start(&models, &t);
        assert_eq!(s[0], [0.0, 0.0]);
        assert!((s[1][0] - 1.0).abs() < 1e-12 && (s[1][1] - 3.0).abs() < 1e-12, "{s:?}");
    }

    #[test]
    fn no_tiles_or_a_single_frame_give_no_start() {
        let s = super::super::synth::series(&super::super::synth::Spec { aw: 400, ah: 400, frames: 1, ..Default::default() }).unwrap();
        assert!(phase_start(&s, &[]).is_none());
        assert!(phase_start(&s, &[Tile { xc: 201, yc: 200, rx: 0.0, ry: 0.0 }]).is_none(), "one frame");
        assert!(wide_start(&s, &[]).is_none());
    }
}
