//! Cutting a picture into overlapping tiles and putting the results back together.
//!
//! A model runs on a fixed square, so a picture is covered by tiles spaced evenly along each axis, each sharing
//! at least `overlap` cells with its neighbours. Each tile's influence fades towards its own edges (except at the
//! picture's edge), and the result is the sum of tile × weight divided by the sum of weights, so any set of
//! overlapping tiles blends consistently and a seam between two tiles of slightly different scale is a smooth ramp.

/// Where the tiles start along one axis of `n` cells: `[0]` when the axis fits one tile (the tile is then padded by
/// mirroring), else evenly spaced starts, the first at 0 and the last at `n − tile`, each tile sharing at least
/// `overlap` cells with the next.
pub fn starts(n: usize, tile: usize, overlap: usize) -> Vec<usize> {
    if n <= tile || tile == 0 {
        return vec![0];
    }
    let step = tile.saturating_sub(overlap).max(1);
    // tiles needed so that start k·spacing with spacing ≤ step reaches n − tile
    let Some(k) = (n - tile).div_ceil(step).checked_add(1).filter(|&k| k <= 1_000_000) else { return Vec::new() };
    let last = n - tile;
    (0..k).map(|i| if k == 1 { 0 } else { ((i as u128 * last as u128 + (k - 1) as u128 / 2) / (k - 1) as u128) as usize }).collect()
}

/// Weight of the cell at offset `p` (0-based, inside a tile of `tile` cells) along one axis: 1 in the middle, easing
/// down to a sliver over `fade` cells at an edge that has a neighbour, 1 right up to an edge that is the picture's.
pub fn weight(p: usize, tile: usize, fade: usize, has_before: bool, has_after: bool) -> f32 {
    let fade = fade.max(1) as f32;
    let ramp = |d: usize| {
        let t = ((d as f32 + 0.5) / fade).min(1.0);
        // smoothstep, never exactly 0 so that a cell is always covered
        (t * t * (3.0 - 2.0 * t)).max(1e-3)
    };
    let before = if has_before { ramp(p) } else { 1.0 };
    let after = if has_after { ramp(tile.saturating_sub(1).saturating_sub(p)) } else { 1.0 };
    before.min(after)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_axes_have_bounded_tile_counts_and_checked_spacing() {
        assert!(starts(usize::MAX, 1, 0).is_empty());
        assert!(starts(usize::MAX, 2, usize::MAX).is_empty());
        let s = starts(usize::MAX, usize::MAX / 2, 0);
        assert_eq!(s.first(), Some(&0));
        assert_eq!(s.last(), Some(&(usize::MAX - usize::MAX / 2)));
        assert!(s.windows(2).all(|s| s[0] < s[1]));
    }

    #[test]
    fn one_tile_when_it_fits() {
        assert_eq!(starts(100, 512, 64), vec![0]);
        assert_eq!(starts(512, 512, 64), vec![0]);
        assert_eq!(starts(0, 512, 64), vec![0]);
        assert_eq!(starts(10, 0, 0), vec![0]);
    }

    #[test]
    fn tiles_cover_the_axis_and_overlap_enough() {
        for n in [513usize, 600, 900, 1000, 1024, 1500, 3024, 3600, 5000, 12345] {
            let s = starts(n, 512, 64);
            assert_eq!(s[0], 0, "n={n}");
            assert_eq!(*s.last().unwrap(), n - 512, "n={n}");
            for w in s.windows(2) {
                assert!(w[1] > w[0], "starts increase, n={n}");
                assert!(w[0] + 512 >= w[1] + 64, "at least 64 shared cells between {} and {}, n={n}", w[0], w[1]);
            }
        }
    }

    #[test]
    fn weights_fade_only_towards_neighbours() {
        let (t, f) = (512, 64);
        // a lone tile: full weight everywhere
        assert!((0..t).all(|p| weight(p, t, f, false, false) == 1.0));
        // the first tile fades on its far side only
        assert_eq!(weight(0, t, f, false, true), 1.0);
        assert!(weight(t - 1, t, f, false, true) < 0.01);
        // the last tile fades on its near side only
        assert!(weight(0, t, f, true, false) < 0.01);
        assert_eq!(weight(t - 1, t, f, true, false), 1.0);
        // a middle tile is 1 in the middle and everywhere positive
        assert_eq!(weight(t / 2, t, f, true, true), 1.0);
        assert!((0..t).all(|p| weight(p, t, f, true, true) > 0.0));
    }

    #[test]
    fn two_overlapping_tiles_blend_to_a_partition_of_unity() {
        // at every cell of the shared region the normalised weights are in [0, 1] and sum to 1
        let (t, f) = (512usize, 64usize);
        let s = starts(900, t, 64);
        assert_eq!(s.len(), 2);
        for cell in s[1]..s[0] + t {
            let a = weight(cell - s[0], t, f, false, true);
            let b = weight(cell - s[1], t, f, true, false);
            let (na, nb) = (a / (a + b), b / (a + b));
            assert!((na + nb - 1.0).abs() < 1e-6 && na >= 0.0 && nb >= 0.0);
        }
    }
}
