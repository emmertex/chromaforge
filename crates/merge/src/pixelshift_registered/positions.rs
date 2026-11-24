//! The positions of all frames of a series: tiles measured from a start, non-converged tiles dropped, a similarity
//! model per frame fitted to the rest, and the statistics the later gate reports.

use super::coarse::{nominal_positions, tile_start, wide_start};
use super::measure::{CONVERGED, EarlyStop, Kernel, TileFit, measure_tile, measure_tiles};
use super::model::{self, Similarity};
use super::series::GreenSeries;
use super::tiles::Tile;

/// Rounds of the measurement from the nominal table and from the coarse start.
pub const ROUNDS_NOMINAL: usize = 3;
pub const ROUNDS_COARSE: usize = 7;
/// Tiles measured at the same time.
const PARALLEL_TILES: usize = 6;
/// Fewest tiles that must remain for a measurement to count.
pub const MIN_TILES: usize = 4;
/// Tiles of the quick nominal check, and how many of them must converge.
pub const QUICK_TILES: usize = 8;
pub const QUICK_NEEDED: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Start {
    /// The nominal table of a 16-shot series, 3 rounds.
    Nominal,
    /// Phase correlation and the wide first stage, up to 7 rounds.
    Coarse,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Options {
    /// Give up tiles that are clearly not settling (see [`super::measure::measure_tile`]).
    pub early_stop: Option<EarlyStop>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum MeasureError {
    #[error("the series has {0} frames; 16 are needed")]
    FrameCount(usize),
    #[error("no tile to measure")]
    NoTiles,
    #[error("no start positions could be found")]
    NoStart,
    #[error("only {0} tiles could be measured in every frame; {1} are needed")]
    TooFewTiles(usize, usize),
}

/// One tile of the fit.
#[derive(Debug, Clone, PartialEq)]
pub struct TileTrack {
    pub tile: Tile,
    /// Position of each frame against frame 0 (sites).
    pub rel: Vec<[f64; 2]>,
    pub last_update: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SeriesFit {
    pub start: Start,
    /// Tiles in which every frame was measured and that converged.
    pub tiles: Vec<TileTrack>,
    /// Tiles in which every frame was measured (before the convergence filter) and how many of them did not converge.
    pub usable: usize,
    pub dropped: usize,
    /// Similarity model of each frame against frame 0 (frame 0 is the identity).
    pub models: Vec<Similarity>,
    /// Mean position of each frame against frame 0 over the kept tiles.
    pub positions: Vec<[f64; 2]>,
    /// Mean over frames and largest over frames of the rms (per coordinate) of what the model leaves over the tiles.
    pub scatter_mean: f64,
    pub scatter_worst: f64,
    /// Largest over frames of the mean displacement from frame 0 (sites).
    pub largest_displacement: f64,
}

impl SeriesFit {
    /// Share of the usable tiles that were dropped as not converged.
    pub fn dropped_fraction(&self) -> f64 {
        if self.usable == 0 { 0.0 } else { self.dropped as f64 / self.usable as f64 }
    }
}

fn spread<T: Copy>(v: &[T], n: usize) -> Vec<T> {
    if v.is_empty() || n == 0 {
        return Vec::new();
    }
    let step = (v.len() as f64 / n as f64).max(1.0);
    (0..n.min(v.len())).filter_map(|i| v.get((i as f64 * step) as usize).copied()).collect()
}

/// Measures `tiles` from the given start and fits the models. A 16-shot series is required for the nominal start.
pub fn measure_series(s: &GreenSeries, tiles: &[Tile], start: Start, opts: &Options) -> Result<SeriesFit, MeasureError> {
    let n = s.frame_count();
    if n < 2 {
        return Err(MeasureError::FrameCount(n));
    }
    if tiles.is_empty() {
        return Err(MeasureError::NoTiles);
    }
    let kernel = Kernel::normal(n);
    let (inits, rounds): (Vec<Vec<[f64; 2]>>, usize) = match start {
        Start::Nominal => {
            let nom = nominal_positions();
            if nom.len() != n {
                return Err(MeasureError::FrameCount(n));
            }
            (tiles.iter().map(|_| nom.clone()).collect(), ROUNDS_NOMINAL)
        }
        Start::Coarse => {
            let models = wide_start(s, tiles).ok_or(MeasureError::NoStart)?;
            (tiles.iter().map(|t| tile_start(&models, t)).collect(), ROUNDS_COARSE)
        }
    };
    let par = if n >= 32 { 3 } else { PARALLEL_TILES };
    let fits = measure_tiles(s, tiles, &inits, rounds, &kernel, opts.early_stop, par);
    fit_from_tiles(start, tiles, &fits)
}

/// The models and statistics from measured tiles (`fits[i]` belongs to `tiles[i]`).
pub fn fit_from_tiles(start: Start, tiles: &[Tile], fits: &[TileFit]) -> Result<SeriesFit, MeasureError> {
    let n = fits.first().map_or(0, |f| f.positions.len());
    let complete: Vec<(&Tile, Vec<[f64; 2]>, f64)> = tiles.iter().zip(fits).filter_map(|(t, f)| Some((t, f.relative()?, f.last_update))).collect();
    let usable = complete.len();
    let kept: Vec<TileTrack> =
        complete.into_iter().filter(|(_, _, u)| *u <= CONVERGED).map(|(t, rel, u)| TileTrack { tile: *t, rel, last_update: u }).collect();
    let dropped = usable - kept.len();
    if kept.len() < MIN_TILES {
        return Err(MeasureError::TooFewTiles(kept.len(), MIN_TILES));
    }
    let mut models = vec![Similarity::default(); n];
    let mut positions = vec![[0.0f64; 2]; n];
    let (mut sum_scatter, mut worst) = (0.0f64, 0.0f64);
    let mut disp: f64 = 0.0;
    for k in 1..n {
        let rows: Vec<(Tile, [f64; 2])> = kept.iter().filter_map(|t| Some((t.tile, *t.rel.get(k)?))).collect();
        let m = model::fit(&rows).unwrap_or_default();
        let sc = model::scatter(&m, &rows);
        sum_scatter += sc;
        worst = worst.max(sc);
        if let Some(slot) = models.get_mut(k) {
            *slot = m;
        }
        let nt = rows.len().max(1) as f64;
        let mean = [rows.iter().map(|r| r.1[0]).sum::<f64>() / nt, rows.iter().map(|r| r.1[1]).sum::<f64>() / nt];
        if let Some(slot) = positions.get_mut(k) {
            *slot = mean;
        }
        // the mean distance of the tiles' displacements from zero, as the validity statistic D defines it
        disp = disp.max(rows.iter().map(|r| r.1[0].hypot(r.1[1])).sum::<f64>() / nt);
    }
    Ok(SeriesFit {
        start,
        tiles: kept,
        usable,
        dropped,
        models,
        positions,
        scatter_mean: sum_scatter / (n.saturating_sub(1)).max(1) as f64,
        scatter_worst: worst,
        largest_displacement: disp,
    })
}

/// Result of the quick check of the nominal start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuickCheck {
    pub measured: usize,
    pub converged: usize,
    pub passes: bool,
}

/// Eight spread tiles are measured from the nominal table (3 rounds, early stop); the nominal start is worth a full
/// measurement if at least six of them converge.
pub fn quick_nominal_check(s: &GreenSeries, tiles: &[Tile]) -> QuickCheck {
    let n = s.frame_count();
    let nom = nominal_positions();
    let pick = spread(tiles, QUICK_TILES);
    if nom.len() != n || pick.is_empty() {
        return QuickCheck { measured: pick.len(), converged: 0, passes: false };
    }
    let kernel = Kernel::normal(n);
    let converged = {
        use rayon::prelude::*;
        pick.par_iter().filter(|t| measure_tile(s, t, &nom, ROUNDS_NOMINAL, &kernel, Some(EarlyStop::default())).converged()).count()
    };
    QuickCheck { measured: pick.len(), converged, passes: pick.len() >= QUICK_NEEDED && converged >= QUICK_NEEDED }
}
