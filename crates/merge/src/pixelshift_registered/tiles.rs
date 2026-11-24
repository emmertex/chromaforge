//! Measurement tiles: the grid, the diamond plane of a tile, the texture score and the selection.
//!
//! A tile is measured through its diamond plane: the green sites form a quincunx lattice, so the sites
//! `(xc + u + v, yc + u - v)` for integer `(u, v)` form a square grid turned by 45 degrees, one plane sample per green
//! site. The window is `PLANE` × `PLANE` plane samples (about 180 sites across). The texture score is the mean
//! squared plane gradient over four times the expected noise variance (about 1 for pure noise); tiles without
//! texture cannot be measured.

use super::series::GreenSeries;
use rayon::prelude::*;

/// Side of a tile's diamond plane in plane samples.
pub const PLANE: usize = 128;
/// Half side (sites) of the region around a tile centre that the measurement reads for its reference.
pub const REF_HALF: i64 = 176;
/// A tile centre must be at least this far (sites) from every edge of the active area.
pub const EDGE: i64 = REF_HALF + 4;
/// Tile grid of the active area.
pub const GRID_X: usize = 16;
pub const GRID_Y: usize = 12;
/// Texture score a tile needs, and the lower bar used when too few tiles reach the first.
pub const BAR: f64 = 3.0;
pub const BAR_LOW: f64 = 1.4;
/// Fewer selected tiles than this make the selection use the lower bar.
pub const MIN_SELECTED: usize = 24;
/// Most tiles kept per cell of 2 × 2 grid tiles.
pub const PER_CELL: usize = 2;
/// Minimum mean level above black (DN) of a usable tile, and the fraction of white above which it counts as clipped.
pub const MIN_LEVEL: f64 = 200.0;
pub const CLIP_FRACTION: f64 = 0.97;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tile {
    pub xc: i64,
    pub yc: i64,
    /// Centre relative to the middle of the active area (sites).
    pub rx: f64,
    pub ry: f64,
}

/// The grid of tile centres (parity-adjusted to a green site).
pub fn grid(s: &GreenSeries) -> Vec<Tile> {
    let mut v = Vec::with_capacity(GRID_X * GRID_Y);
    for j in 0..GRID_Y {
        for i in 0..GRID_X {
            let mut xc = ((i as f64 + 0.5) * s.aw as f64 / GRID_X as f64) as i64;
            let yc = ((j as f64 + 0.5) * s.ah as f64 / GRID_Y as f64) as i64;
            if ((xc + yc) as usize & 1) != s.gpar {
                xc += 1;
            }
            v.push(Tile { xc, yc, rx: xc as f64 - s.aw as f64 / 2.0, ry: yc as f64 - s.ah as f64 / 2.0 });
        }
    }
    v
}

/// The diamond plane of frame `k` around `(xc, yc)`, row by row. Camera-filled rows are replaced by the mean of the
/// sites two rows above and below. `None` if any site is outside the area or the frame is missing.
pub fn diamond_plane(s: &GreenSeries, k: usize, xc: i64, yc: i64) -> Option<Vec<f64>> {
    let h = (PLANE / 2) as i64;
    let mut out = Vec::with_capacity(PLANE * PLANE);
    for j in 0..PLANE as i64 {
        for i in 0..PLANE as i64 {
            let (u, v) = (i - h, j - h);
            let (x, y) = (xc + u + v, yc + u - v);
            out.push(if s.masked_row(y) { 0.5 * (s.g(k, x, y - 2)? + s.g(k, x, y + 2)?) } else { s.g(k, x, y)? });
        }
    }
    Some(out)
}

/// Texture score and mean level above black of a plane.
pub fn texture(p: &[f64], black: f64, noise: (f64, f64)) -> (f64, f64) {
    if p.len() != PLANE * PLANE {
        return (0.0, 0.0);
    }
    let mean = p.iter().sum::<f64>() / p.len() as f64;
    let var = (noise.0 * (mean - black) + noise.1).max(1.0);
    let (mut g, mut n) = (0.0, 0.0);
    for y in 0..PLANE - 1 {
        for x in 0..PLANE - 1 {
            let a = p.get(y * PLANE + x).copied().unwrap_or(0.0);
            let right = p.get(y * PLANE + x + 1).copied().unwrap_or(a);
            let below = p.get((y + 1) * PLANE + x).copied().unwrap_or(a);
            g += (right - a).powi(2) + (below - a).powi(2);
            n += 1.0;
        }
    }
    (g / n / (4.0 * var), mean - black)
}

/// Textured tiles of frame 0 with their score: score at least `bar`, mean level at least [`MIN_LEVEL`], no sample
/// above [`CLIP_FRACTION`] of white, and the whole reference region inside the active area.
pub fn scored_tiles(s: &GreenSeries, bar: f64) -> Vec<(Tile, f64)> {
    grid(s)
        .into_par_iter()
        .filter_map(|t| {
            if t.xc < EDGE || t.yc < EDGE || t.xc + EDGE >= s.aw as i64 || t.yc + EDGE >= s.ah as i64 {
                return None;
            }
            let p = diamond_plane(s, 0, t.xc, t.yc)?;
            let (score, level) = texture(&p, s.black, s.noise);
            let mx = p.iter().cloned().fold(f64::MIN, f64::max);
            (score >= bar && level >= MIN_LEVEL && mx <= CLIP_FRACTION * s.white).then_some((t, score))
        })
        .collect()
}

/// At most [`PER_CELL`] tiles (the most textured) per cell of 2 × 2 grid tiles, in cell order.
pub fn select(s: &GreenSeries, scored: &[(Tile, f64)]) -> Vec<Tile> {
    let (cx, cy) = (GRID_X / 2, GRID_Y / 2);
    let cell = |t: &Tile| ((t.xc * cx as i64 / s.aw as i64).clamp(0, cx as i64 - 1), (t.yc * cy as i64 / s.ah as i64).clamp(0, cy as i64 - 1));
    let mut out = Vec::new();
    for j in 0..cy as i64 {
        for i in 0..cx as i64 {
            let mut v: Vec<&(Tile, f64)> = scored.iter().filter(|x| cell(&x.0) == (i, j)).collect();
            v.sort_by(|a, b| b.1.total_cmp(&a.1));
            out.extend(v.iter().take(PER_CELL).map(|x| x.0));
        }
    }
    out
}

#[derive(Debug, Clone)]
pub struct Selection {
    pub tiles: Vec<Tile>,
    /// The texture bar that was used.
    pub bar: f64,
    /// Tiles that passed the bar before the per-cell limit.
    pub candidates: usize,
}

/// The tiles to measure: bar [`BAR`], relaxed to [`BAR_LOW`] when fewer than [`MIN_SELECTED`] are selected.
pub fn select_tiles(s: &GreenSeries) -> Selection {
    let mut bar = BAR;
    let mut sc = scored_tiles(s, bar);
    let mut tiles = select(s, &sc);
    if tiles.len() < MIN_SELECTED {
        bar = BAR_LOW;
        sc = scored_tiles(s, bar);
        tiles = select(s, &sc);
    }
    Selection { tiles, bar, candidates: sc.len() }
}

#[cfg(test)]
mod tests {
    use super::super::synth::{Spec, series};
    use super::*;

    fn textured(aw: usize, ah: usize) -> GreenSeries {
        series(&Spec { aw, ah, frames: 1, ..Spec::default() }).unwrap()
    }

    #[test]
    fn grid_has_one_green_tile_per_cell() {
        for gpar in 0..2 {
            let s = series(&Spec { aw: 1600, ah: 1200, frames: 1, gpar, flat: true, ..Spec::default() }).unwrap();
            let g = grid(&s);
            assert_eq!(g.len(), GRID_X * GRID_Y);
            for t in &g {
                assert_eq!((t.xc + t.yc) as usize & 1, gpar);
                assert!(t.xc >= 0 && t.xc < 1600 && t.yc >= 0 && t.yc < 1200);
            }
            let (a, b) = (g.first().unwrap(), g.last().unwrap());
            assert!(a.rx < 0.0 && a.ry < 0.0 && b.rx > 0.0 && b.ry > 0.0, "relative centres run around the middle");
        }
    }

    #[test]
    fn diamond_plane_reads_the_green_lattice() {
        let s = textured(1000, 800); // green sites have x + y odd
        let at = |p: &[f64]| p[(PLANE / 2) * PLANE + PLANE / 2];
        // the centre sample of the plane is the site (xc, yc) itself
        let p = diamond_plane(&s, 0, 501, 400).unwrap();
        assert_eq!(p.len(), PLANE * PLANE);
        assert!(!s.masked_row(400));
        assert_eq!(at(&p), s.g(0, 501, 400).unwrap());
        // a camera-filled row (raw row 401 = 5 mod 12) is replaced by the mean of the sites two rows above and below
        assert!(s.masked_row(401));
        let q = diamond_plane(&s, 0, 500, 401).unwrap();
        assert_eq!(at(&q), 0.5 * (s.g(0, 500, 399).unwrap() + s.g(0, 500, 403).unwrap()));
        assert!(diamond_plane(&s, 0, 21, 400).is_none(), "window past the edge");
        assert!(diamond_plane(&s, 3, 501, 400).is_none(), "missing frame");
        assert!(diamond_plane(&s, 0, 500, 400).is_none(), "a centre that is not a green site leaves the lattice");
    }

    #[test]
    fn texture_is_about_one_for_noise_and_large_for_structure() {
        let flat = series(&Spec { aw: 1000, ah: 800, frames: 1, flat: true, ..Spec::default() }).unwrap();
        let tex = textured(1000, 800);
        let (sf, lf) = texture(&diamond_plane(&flat, 0, 501, 400).unwrap(), flat.black, flat.noise);
        let (st, _) = texture(&diamond_plane(&tex, 0, 501, 400).unwrap(), tex.black, tex.noise);
        assert!((0.5..2.0).contains(&sf), "noise-only score {sf}");
        assert!(lf > MIN_LEVEL);
        assert!(st > 10.0 * sf, "textured {st} against flat {sf}");
        assert_eq!(texture(&[1.0, 2.0], 0.0, (1.0, 0.0)), (0.0, 0.0), "wrong length");
    }

    #[test]
    fn selection_keeps_at_most_two_tiles_per_cell_and_none_near_the_edge() {
        let s = textured(2400, 1800);
        let sel = select_tiles(&s);
        assert!(sel.tiles.len() >= 24, "{} tiles", sel.tiles.len());
        assert_eq!(sel.bar, BAR);
        assert!(sel.candidates >= sel.tiles.len());
        let mut per = std::collections::HashMap::new();
        for t in &sel.tiles {
            assert!(t.xc >= EDGE && t.yc >= EDGE && t.xc + EDGE < 2400 && t.yc + EDGE < 1800);
            *per.entry((t.xc * 8 / 2400, t.yc * 6 / 1800)).or_insert(0) += 1;
        }
        assert!(per.values().all(|n| *n <= PER_CELL), "{per:?}");
    }

    #[test]
    fn flat_or_dark_or_clipped_scenes_give_no_tiles() {
        let flat = series(&Spec { aw: 2400, ah: 1800, frames: 1, flat: true, ..Spec::default() }).unwrap();
        assert!(select_tiles(&flat).tiles.is_empty(), "no texture");
        let dark = series(&Spec { aw: 2400, ah: 1800, frames: 1, level: 50.0, contrast: 0.01, ..Spec::default() }).unwrap();
        assert!(scored_tiles(&dark, BAR_LOW).is_empty(), "too dark");
        let clipped = series(&Spec { aw: 2400, ah: 1800, frames: 1, level: 20_000.0, ..Spec::default() }).unwrap();
        assert!(scored_tiles(&clipped, BAR_LOW).is_empty(), "clipped");
    }

    #[test]
    fn small_frames_have_no_tile_far_enough_from_the_edge() {
        let s = textured(300, 300);
        assert!(select_tiles(&s).tiles.is_empty());
    }

    #[test]
    fn low_bar_is_used_when_the_first_gives_too_few_tiles() {
        // a weak texture: its score lies between the low and the normal bar in most tiles
        let s = series(&Spec { aw: 2400, ah: 1800, frames: 1, contrast: 0.2, ..Spec::default() }).unwrap();
        let strict = select(&s, &scored_tiles(&s, BAR)).len();
        assert!(strict < MIN_SELECTED, "the normal bar should leave too few tiles: {strict}");
        let sel = select_tiles(&s);
        assert_eq!(sel.bar, BAR_LOW);
        assert!(sel.tiles.len() > strict, "{} tiles at the low bar against {strict}", sel.tiles.len());
    }
}
