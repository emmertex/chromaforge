//! Mask helpers: exact Euclidean distance transform and disc dilation.
//!
//! Source: P. F. Felzenszwalb, D. P. Huttenlocher, *Distance Transforms of Sampled Functions*,
//! Theory of Computing 8, 2012 (the lower envelope of parabolas, one pass per row and column).

/// Squared distance from every cell of a 1-D function to the nearest finite sample, i.e.
/// `out[q] = min_p (q - p)² + f[p]`. Infinite samples take no part; all-infinite input stays infinite.
fn edt_1d(f: &[f64], out: &mut [f64], v: &mut Vec<usize>, z: &mut Vec<f64>) {
    v.clear();
    z.clear();
    for (q, fq) in f.iter().enumerate() {
        if !fq.is_finite() {
            continue;
        }
        let qf = q as f64;
        loop {
            let Some(&p) = v.last() else {
                v.push(q);
                z.push(f64::NEG_INFINITY);
                break;
            };
            let pf = p as f64;
            let fp = f.get(p).copied().unwrap_or(f64::INFINITY);
            let s = ((fq + qf * qf) - (fp + pf * pf)) / (2.0 * (qf - pf));
            if z.last().is_some_and(|zk| s <= *zk) && v.len() > 1 {
                v.pop();
                z.pop();
                continue;
            }
            v.push(q);
            z.push(s);
            break;
        }
    }
    if v.is_empty() {
        out.iter_mut().for_each(|o| *o = f64::INFINITY);
        return;
    }
    let mut k = 0usize;
    for (q, o) in out.iter_mut().enumerate() {
        let qf = q as f64;
        while k + 1 < v.len() && z.get(k + 1).is_some_and(|zk| *zk < qf) {
            k += 1;
        }
        let p = v.get(k).copied().unwrap_or(0);
        let d = qf - p as f64;
        *o = d * d + f.get(p).copied().unwrap_or(f64::INFINITY);
    }
}

/// Squared Euclidean distance (in pixels) from every cell to the nearest set cell of `mask`
/// (`0` on set cells, `+∞` everywhere when nothing is set). `None` on a size mismatch.
pub(crate) fn distance_sq(w: usize, h: usize, mask: &[bool]) -> Option<Vec<f32>> {
    if Some(mask.len()) != w.checked_mul(h) {
        return None;
    }
    let mut g: Vec<f64> = mask.iter().map(|m| if *m { 0.0 } else { f64::INFINITY }).collect();
    let (mut v, mut z) = (Vec::new(), Vec::new());
    let mut col = vec![0.0f64; h];
    let mut col_out = vec![0.0f64; h];
    // Columns first, then rows.
    for x in 0..w {
        for (y, c) in col.iter_mut().enumerate() {
            *c = g[y * w + x];
        }
        edt_1d(&col, &mut col_out, &mut v, &mut z);
        for (y, c) in col_out.iter().enumerate() {
            g[y * w + x] = *c;
        }
    }
    let mut row_out = vec![0.0f64; w];
    for row in g.chunks_mut(w.max(1)) {
        edt_1d(row, &mut row_out, &mut v, &mut z);
        row.copy_from_slice(&row_out[..row.len()]);
    }
    Some(g.into_iter().map(|d| d as f32).collect())
}

/// Grow `mask` by a disc of `radius` pixels (a cell is set when a set cell lies within `radius`,
/// Euclidean). Linear time in `w × h` whatever the radius. On a size mismatch `mask` comes back
/// unchanged.
pub fn dilate(w: usize, h: usize, mask: &[bool], radius: usize) -> Vec<bool> {
    if radius == 0 {
        return mask.to_vec();
    }
    let Some(d) = distance_sq(w, h, mask) else { return mask.to_vec() };
    let r2 = (radius as f64 * radius as f64) as f32;
    d.into_iter().map(|d| d <= r2).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_matches_brute_force() {
        let (w, h) = (23, 17);
        let mask: Vec<bool> = (0..w * h).map(|i| (i * 7919) % 41 == 0).collect();
        let d = distance_sq(w, h, &mask).unwrap();
        for y in 0..h {
            for x in 0..w {
                let mut best = f32::INFINITY;
                for (j, m) in mask.iter().enumerate() {
                    if *m {
                        let (dx, dy) = ((j % w) as f32 - x as f32, (j / w) as f32 - y as f32);
                        best = best.min(dx * dx + dy * dy);
                    }
                }
                assert_eq!(d[y * w + x], best, "({x},{y})");
            }
        }
    }

    #[test]
    fn dilate_disc_and_hostile_inputs() {
        let (w, h) = (21, 21);
        let mut m = vec![false; w * h];
        m[10 * w + 10] = true;
        let d = dilate(w, h, &m, 3);
        assert_eq!(d.iter().filter(|b| **b).count(), 29); // lattice points with x² + y² ≤ 9
        assert!(d[10 * w + 13] && !d[13 * w + 13]);
        assert_eq!(dilate(w, h, &m, 0), m);
        assert_eq!(dilate(4, 4, &[true; 3], 2), vec![true; 3]);
        assert_eq!(dilate(usize::MAX, 2, &[true; 3], 2), vec![true; 3]);
        assert!(dilate(5, 5, &[false; 25], 4).iter().all(|b| !b));
        assert!(dilate(0, 0, &[], 4).is_empty());
    }
}
