//! Small RGB / scalar grids, area reduction and bilinear sampling for registration and sampling.
//! Every index is checked (`get`): sizes come from decoded files.

/// A `w × h` grid of `T`, row-major.
#[derive(Clone, Debug)]
pub struct Grid<T> {
    pub w: usize,
    pub h: usize,
    pub data: Vec<T>,
}

impl<T: Copy + Default> Grid<T> {
    pub fn filled(w: usize, h: usize, v: T) -> Grid<T> {
        Grid { w, h, data: vec![v; w.saturating_mul(h)] }
    }

    #[inline]
    pub fn at(&self, x: usize, y: usize) -> T {
        if x >= self.w {
            return T::default();
        }
        self.data.get(y.saturating_mul(self.w).saturating_add(x)).copied().unwrap_or_default()
    }

    #[inline]
    pub fn set(&mut self, x: usize, y: usize, v: T) {
        if x < self.w
            && let Some(p) = self.data.get_mut(y.saturating_mul(self.w).saturating_add(x))
        {
            *p = v;
        }
    }

    pub fn map<U: Copy + Default>(&self, f: impl Fn(T) -> U) -> Grid<U> {
        Grid { w: self.w, h: self.h, data: self.data.iter().map(|v| f(*v)).collect() }
    }
}

/// Integer source span `[a, b)` of output cell `o` of `n_out` over `n_in` (never empty when `n_in > 0`).
fn span(o: usize, n_out: usize, n_in: usize) -> (usize, usize) {
    let a = o.saturating_mul(n_in) / n_out.max(1);
    let b = (o.saturating_add(1).saturating_mul(n_in) / n_out.max(1)).max(a.saturating_add(1)).min(n_in);
    (a.min(n_in.saturating_sub(1)), b)
}

/// Area-average `src` to `w × h`; also the per-cell maximum of `key` over the source pixels.
pub fn reduce_rgb(src: &Grid<[f32; 3]>, w: usize, h: usize, key: impl Fn([f32; 3]) -> f32) -> (Grid<[f32; 3]>, Grid<f32>) {
    let mut out = Grid::filled(w, h, [0.0f32; 3]);
    let mut mx = Grid::filled(w, h, 0.0f32);
    if src.w == 0 || src.h == 0 {
        return (out, mx);
    }
    for oy in 0..h {
        let (y0, y1) = span(oy, h, src.h);
        for ox in 0..w {
            let (x0, x1) = span(ox, w, src.w);
            let mut acc = [0.0f64; 3];
            let mut m = f32::MIN;
            let mut n = 0u32;
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = src.at(x, y);
                    acc[0] += p[0] as f64;
                    acc[1] += p[1] as f64;
                    acc[2] += p[2] as f64;
                    m = m.max(key(p));
                    n += 1;
                }
            }
            let n = n.max(1) as f64;
            out.set(ox, oy, [(acc[0] / n) as f32, (acc[1] / n) as f32, (acc[2] / n) as f32]);
            mx.set(ox, oy, if m == f32::MIN { 0.0 } else { m });
        }
    }
    (out, mx)
}

/// `(w, h)` with the long edge `long` keeping the aspect of `sw × sh` (each at least 1).
pub fn fit_long(sw: usize, sh: usize, long: usize) -> (usize, usize) {
    let long = long.max(1);
    if sw >= sh {
        (long, ((sh as f64 * long as f64 / sw.max(1) as f64).round() as usize).max(1))
    } else {
        (((sw as f64 * long as f64 / sh.max(1) as f64).round() as usize).max(1), long)
    }
}

/// Bilinear sample at continuous coordinates (pixel centres at +0.5); `None` outside the grid.
#[inline]
pub fn bilinear<T: Copy + Default + Lerp>(g: &Grid<T>, x: f32, y: f32) -> Option<T> {
    if !(x.is_finite() && y.is_finite()) || x < 0.0 || y < 0.0 || x > g.w as f32 || y > g.h as f32 || g.w == 0 || g.h == 0 {
        return None;
    }
    let fx = (x - 0.5).max(0.0);
    let fy = (y - 0.5).max(0.0);
    let x0 = (fx.floor() as usize).min(g.w - 1);
    let y0 = (fy.floor() as usize).min(g.h - 1);
    let x1 = (x0 + 1).min(g.w - 1);
    let y1 = (y0 + 1).min(g.h - 1);
    let tx = (fx - x0 as f32).clamp(0.0, 1.0);
    let ty = (fy - y0 as f32).clamp(0.0, 1.0);
    let top = T::lerp(g.at(x0, y0), g.at(x1, y0), tx);
    let bot = T::lerp(g.at(x0, y1), g.at(x1, y1), tx);
    Some(T::lerp(top, bot, ty))
}

pub trait Lerp {
    fn lerp(a: Self, b: Self, t: f32) -> Self;
}

impl Lerp for f32 {
    #[inline]
    fn lerp(a: f32, b: f32, t: f32) -> f32 {
        a + (b - a) * t
    }
}

impl Lerp for [f32; 3] {
    #[inline]
    fn lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
        [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
    }
}

/// Box blur of radius `r` (clamped window) via a summed-area table.
pub fn box_blur(g: &Grid<f32>, r: usize) -> Grid<f32> {
    let (w, h) = (g.w, g.h);
    let mut sat = vec![0.0f64; (w + 1).saturating_mul(h + 1)];
    let idx = |x: usize, y: usize| y * (w + 1) + x;
    for y in 0..h {
        let mut row = 0.0f64;
        for x in 0..w {
            row += g.at(x, y) as f64;
            let above = sat.get(idx(x + 1, y)).copied().unwrap_or(0.0);
            if let Some(p) = sat.get_mut(idx(x + 1, y + 1)) {
                *p = above + row;
            }
        }
    }
    let mut out = Grid::filled(w, h, 0.0f32);
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(r), (y + r + 1).min(h));
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(w));
            let s = |a: usize, b: usize| sat.get(idx(a, b)).copied().unwrap_or(0.0);
            let sum = s(x1, y1) - s(x0, y1) - s(x1, y0) + s(x0, y0);
            let n = ((x1 - x0) * (y1 - y0)).max(1) as f64;
            out.set(x, y, (sum / n) as f32);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduce_averages_and_tracks_max() {
        let mut g = Grid::filled(4, 4, [0.5f32; 3]);
        g.set(0, 0, [1.0, 0.0, 0.0]);
        let (r, m) = reduce_rgb(&g, 2, 2, |p| p[0].max(p[1]).max(p[2]));
        assert!((r.at(1, 1)[0] - 0.5).abs() < 1e-6);
        assert!((m.at(0, 0) - 1.0).abs() < 1e-6);
        assert!((m.at(1, 1) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn degenerate_grids_do_not_panic() {
        let g: Grid<[f32; 3]> = Grid::filled(0, 0, [0.0; 3]);
        let _ = reduce_rgb(&g, 3, 3, |p| p[0]);
        assert!(bilinear(&Grid::filled(0, 0, 0.0f32), 0.0, 0.0).is_none());
        assert!(bilinear(&Grid::filled(2, 2, 1.0f32), f32::NAN, 0.0).is_none());
        let _ = box_blur(&Grid::filled(0, 5, 0.0), 3);
    }
}
