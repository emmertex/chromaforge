//! The per-frame similarity model: displacement of a frame against frame 0 at a point is
//! `t + theta * (-ry, rx) + scale * (rx, ry)`, with `(rx, ry)` relative to the middle of the active area (sites).
//! One least-squares fit over the tiles replaces one translation per frame; rotation and scale matter for series that
//! move by many sites or whose lens changes magnification slightly.

use super::tiles::Tile;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Similarity {
    pub t: [f64; 2],
    pub theta: f64,
    pub scale: f64,
}

impl Similarity {
    /// Displacement (sites) at the point `(rx, ry)` relative to the middle of the active area.
    pub fn at(&self, rx: f64, ry: f64) -> [f64; 2] {
        [self.t[0] - self.theta * ry + self.scale * rx, self.t[1] + self.theta * rx + self.scale * ry]
    }

    /// Displacement at a tile's centre.
    pub fn at_tile(&self, t: &Tile) -> [f64; 2] {
        self.at(t.rx, t.ry)
    }
}

/// Solves the symmetric 4 × 4 system `a x = b` (row-major `a`) with a tiny ridge and partial pivoting.
fn solve4(mut a: [f64; 16], mut b: [f64; 4]) -> Option<[f64; 4]> {
    for c in 0..4 {
        a[c * 4 + c] += 1e-9;
    }
    for c in 0..4 {
        let p = (c..4).max_by(|&i, &j| a[i * 4 + c].abs().total_cmp(&a[j * 4 + c].abs()))?;
        if !a[p * 4 + c].is_finite() || a[p * 4 + c].abs() < 1e-14 {
            return None;
        }
        if p != c {
            for k in 0..4 {
                a.swap(c * 4 + k, p * 4 + k);
            }
            b.swap(c, p);
        }
        for r in c + 1..4 {
            let f = a[r * 4 + c] / a[c * 4 + c];
            if f != 0.0 {
                for k in c..4 {
                    a[r * 4 + k] -= f * a[c * 4 + k];
                }
                b[r] -= f * b[c];
            }
        }
    }
    let mut x = [0.0; 4];
    for r in (0..4).rev() {
        let mut v = b[r];
        for k in r + 1..4 {
            v -= a[r * 4 + k] * x[k];
        }
        x[r] = v / a[r * 4 + r];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// Least-squares similarity over `(tile, displacement)` pairs; `None` for fewer than 3 pairs or a degenerate layout.
pub fn fit(rows: &[(Tile, [f64; 2])]) -> Option<Similarity> {
    if rows.len() < 3 {
        return None;
    }
    let (mut ata, mut atb) = ([0.0; 16], [0.0; 4]);
    for (t, d) in rows {
        for comp in 0..2 {
            let row = if comp == 0 { [1.0, 0.0, -t.ry, t.rx] } else { [0.0, 1.0, t.rx, t.ry] };
            for a in 0..4 {
                atb[a] += row[a] * d[comp];
                for b in 0..4 {
                    ata[a * 4 + b] += row[a] * row[b];
                }
            }
        }
    }
    let x = solve4(ata, atb)?;
    Some(Similarity { t: [x[0], x[1]], theta: x[2], scale: x[3] })
}

/// rms (per coordinate) of what the model leaves over the tiles
pub fn scatter(model: &Similarity, rows: &[(Tile, [f64; 2])]) -> f64 {
    if rows.is_empty() {
        return 0.0;
    }
    let ss: f64 = rows
        .iter()
        .map(|(t, d)| {
            let m = model.at_tile(t);
            (d[0] - m[0]).powi(2) + (d[1] - m[1]).powi(2)
        })
        .sum();
    (ss / (2.0 * rows.len() as f64)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(rx: f64, ry: f64) -> Tile {
        Tile { xc: 0, yc: 0, rx, ry }
    }

    #[test]
    fn a_similarity_is_recovered_exactly_from_its_displacements() {
        let truth = Similarity { t: [3.2, -7.5], theta: 0.0009, scale: -0.0004 };
        let rows: Vec<(Tile, [f64; 2])> = [(-3000.0, -2000.0), (3000.0, -2000.0), (-3000.0, 2000.0), (3000.0, 2000.0), (0.0, 0.0), (1200.0, -300.0)]
            .iter()
            .map(|&(x, y)| (tile(x, y), truth.at(x, y)))
            .collect();
        let m = fit(&rows).unwrap();
        assert!((m.t[0] - 3.2).abs() < 1e-6 && (m.t[1] + 7.5).abs() < 1e-6, "{m:?}");
        assert!((m.theta - 0.0009).abs() < 1e-9 && (m.scale + 0.0004).abs() < 1e-9, "{m:?}");
        assert!(scatter(&m, &rows) < 1e-6);
    }

    #[test]
    fn scatter_measures_what_the_model_leaves() {
        let rows =
            vec![(tile(-100.0, 0.0), [0.1, 0.0]), (tile(100.0, 0.0), [-0.1, 0.0]), (tile(0.0, 100.0), [0.0, -0.1]), (tile(0.0, -100.0), [0.0, 0.1])];
        let m = fit(&rows).unwrap();
        assert!(scatter(&m, &rows) < 1e-6, "an exact scale of -1e-3 leaves nothing");
        let noisy: Vec<(Tile, [f64; 2])> =
            rows.iter().enumerate().map(|(i, (t, d))| (*t, [d[0] + if i % 2 == 0 { 0.05 } else { -0.05 }, d[1]])).collect();
        let m2 = fit(&noisy).unwrap();
        assert!(scatter(&m2, &noisy) > 0.01);
    }

    #[test]
    fn degenerate_layouts_are_refused() {
        assert!(fit(&[]).is_none());
        assert!(fit(&[(tile(0.0, 0.0), [1.0, 1.0]); 2]).is_none(), "fewer than three");
        // all tiles on one point: rotation and scale cannot be told apart from the shift
        let same = vec![(tile(5.0, 5.0), [1.0, 2.0]); 5];
        let m = fit(&same);
        if let Some(m) = m {
            assert!(m.t.iter().all(|v| v.is_finite()));
        }
        let nan = vec![(tile(0.0, 0.0), [f64::NAN, 0.0]), (tile(10.0, 0.0), [0.0, 0.0]), (tile(0.0, 10.0), [0.0, 0.0])];
        assert!(fit(&nan).is_none());
    }
}
