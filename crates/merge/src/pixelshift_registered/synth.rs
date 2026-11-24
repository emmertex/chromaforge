//! Synthetic series for the tests of the registered merge: a procedural scene sampled through a colour filter array
//! at known sub-sample positions, with noise. Nothing here reads a file.

use super::series::GreenSeries;

/// Deterministic generator (xorshift) with a normal deviate.
pub struct Rng(pub u64);

impl Rng {
    pub fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn normal(&mut self) -> f64 {
        let (a, b) = (self.uniform().max(1e-12), self.uniform());
        (-2.0 * a.ln()).sqrt() * (2.0 * std::f64::consts::PI * b).cos()
    }
}

fn splitmix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A smooth random scene: plane waves of period 6 to 30 sites around a mean level.
pub struct Scene {
    waves: Vec<(f64, f64, f64, f64)>, // fx, fy, phase, amplitude
    chroma: [Vec<(f64, f64, f64, f64)>; 3],
    level: f64,
}

impl Scene {
    pub fn new(seed: u64, level: f64, contrast: f64) -> Scene {
        let mut r = Rng(0x9E37_79B9_7F4A_7C15 ^ seed.wrapping_mul(0x1234_5678_9ABC_DEF1) | 1);
        let mut mk = |n: usize, amp: f64| -> Vec<(f64, f64, f64, f64)> {
            (0..n)
                .map(|_| {
                    let per = 6.0 + 24.0 * r.uniform();
                    let th = 2.0 * std::f64::consts::PI * r.uniform();
                    (th.cos() / per, th.sin() / per, 2.0 * std::f64::consts::PI * r.uniform(), amp * (0.5 + r.uniform()))
                })
                .collect()
        };
        let waves = mk(10, 150.0 * contrast);
        let chroma = [mk(3, 0.1 * contrast), mk(3, 0.1 * contrast), mk(3, 0.1 * contrast)];
        Scene { waves, chroma, level }
    }

    /// Scene value above black of colour `c` (0 red, 1 green, 2 blue) at the scene position `(u, v)` in sites.
    pub fn value(&self, c: usize, u: f64, v: f64) -> f64 {
        let mut l = self.level;
        for w in &self.waves {
            l += w.3 * (2.0 * std::f64::consts::PI * (w.0 * u + w.1 * v) + w.2).cos();
        }
        let mut k = 1.0;
        if let Some(ws) = self.chroma.get(c) {
            for w in ws {
                k += w.3 * (2.0 * std::f64::consts::PI * (w.0 * u + w.1 * v) + w.2).cos();
            }
        }
        l.max(50.0) * k * [0.8, 1.0, 0.5].get(c).copied().unwrap_or(1.0)
    }
}

#[derive(Debug, Clone)]
pub struct Spec {
    /// Active area in sites (the width is even).
    pub aw: usize,
    pub ah: usize,
    pub frames: usize,
    /// Parity of `x + y` at green sites.
    pub gpar: usize,
    /// Scene without structure (noise only).
    pub flat: bool,
    /// Mean green level above black (DN).
    pub level: f64,
    /// Scales the amplitude of the scene structure.
    pub contrast: f64,
    pub seed: u64,
    /// Position of frame `k` in sites (the frame sees the scene at `site + position`); `None`: half-site offsets.
    pub positions: Option<Vec<[f64; 2]>>,
    /// Brightness factor of frame `k`; empty: all 1.
    pub gains: Vec<f64>,
    /// Leave the noise out (the values are only rounded).
    pub noise_free: bool,
}

impl Default for Spec {
    fn default() -> Spec {
        Spec {
            aw: 1600,
            ah: 1200,
            frames: 16,
            gpar: 1,
            flat: false,
            level: 2000.0,
            contrast: 1.0,
            seed: 1,
            positions: None,
            gains: Vec::new(),
            noise_free: false,
        }
    }
}

pub const BLACK: f64 = 1008.0;
pub const WHITE: f64 = 16383.0;
/// Noise variance of a sample is `NOISE_A` times its level above black.
pub const NOISE_A: f64 = 0.316;

impl Spec {
    pub fn position(&self, k: usize) -> [f64; 2] {
        match &self.positions {
            Some(p) => p.get(k).copied().unwrap_or([0.0, 0.0]),
            None => [0.5 * (k % 2) as f64, 0.5 * ((k / 2) % 2) as f64],
        }
    }
}

/// The raw mosaic of frame `k`: red where `x + y` has the other parity and `y` is even, blue where it is odd.
pub fn mosaic(spec: &Spec, scene: &Scene, k: usize) -> Vec<u16> {
    let mut rng = Rng(splitmix(spec.seed ^ (k as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15)) | 1);
    let p = spec.position(k);
    let gain = spec.gains.get(k).copied().unwrap_or(1.0);
    let mut out = Vec::with_capacity(spec.aw * spec.ah);
    for y in 0..spec.ah {
        for x in 0..spec.aw {
            let c = if (x + y) & 1 == spec.gpar {
                1
            } else if y & 1 == 0 {
                0
            } else {
                2
            };
            let v = if spec.flat { spec.level * [0.8, 1.0, 0.5][c] } else { scene.value(c, x as f64 + p[0], y as f64 + p[1]) } * gain;
            let n = if spec.noise_free { 0.0 } else { (NOISE_A * v.max(1.0)).sqrt() * rng.normal() };
            out.push((BLACK + v + n).round().clamp(0.0, 65535.0) as u16);
        }
    }
    out
}

/// The green series of the spec, built through the production extraction path.
pub fn series(spec: &Spec) -> Result<GreenSeries, String> {
    let scene = Scene::new(spec.seed, spec.level, spec.contrast);
    let mut s = GreenSeries::new(spec.aw, spec.ah, 0, spec.gpar, BLACK, WHITE, (NOISE_A, 0.0))?;
    for k in 0..spec.frames {
        s.push_mosaic(&mosaic(spec, &scene, k), spec.aw, 0, 0)?;
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_generator_is_deterministic_and_noise_has_the_stated_variance() {
        let spec = Spec { aw: 400, ah: 300, frames: 2, flat: true, ..Spec::default() };
        let a = series(&spec).unwrap();
        let b = series(&spec).unwrap();
        assert_eq!(a.g(0, 11, 10), b.g(0, 11, 10));
        assert_ne!(a.g(0, 11, 10), a.g(1, 11, 10), "frames carry independent noise");
        let (mut s, mut s2, mut n) = (0.0, 0.0, 0.0);
        for y in 0..300 {
            for x in 0..400 {
                if let Some(v) = a.g(0, x, y) {
                    let d = v - BLACK;
                    s += d;
                    s2 += d * d;
                    n += 1.0;
                }
            }
        }
        let (mean, var) = (s / n, s2 / n - (s / n).powi(2));
        assert!((mean - 2000.0).abs() < 5.0, "mean {mean}");
        assert!((var / (NOISE_A * 2000.0) - 1.0).abs() < 0.1, "variance {var}");
    }

    #[test]
    fn frames_see_the_scene_at_their_position() {
        // frame k at position p reads the scene at site + p: the point seen at x in frame 0 is seen at x - 2 in a frame at (2, 0)
        let spec = Spec { aw: 200, ah: 200, frames: 2, positions: Some(vec![[0.0, 0.0], [2.0, 0.0]]), noise_free: true, ..Spec::default() };
        let scene = Scene::new(spec.seed, spec.level, spec.contrast);
        let (m0, m1) = (mosaic(&spec, &scene, 0), mosaic(&spec, &scene, 1));
        for y in 10..190usize {
            for x in 12..190usize {
                let (a, b) = (i32::from(m0[y * 200 + x]), i32::from(m1[y * 200 + x - 2]));
                assert!((a - b).abs() <= 1, "({x}, {y}): {a} against {b}");
            }
        }
        // a half-site position changes the values
        let half = Spec { positions: Some(vec![[0.0, 0.0], [0.5, 0.0]]), ..spec.clone() };
        let m2 = mosaic(&half, &scene, 1);
        assert!(m0.iter().zip(&m2).any(|(a, b)| a != b));
    }

    #[test]
    fn gains_scale_a_frame() {
        let spec = Spec { aw: 400, ah: 300, frames: 2, flat: true, gains: vec![1.0, 1.1], ..Spec::default() };
        let s = series(&spec).unwrap();
        let mean = |k: usize| {
            let (mut t, mut n) = (0.0, 0.0);
            for y in 0..300 {
                for x in 0..400 {
                    if let Some(v) = s.g(k, x, y) {
                        t += v - BLACK;
                        n += 1.0;
                    }
                }
            }
            t / n
        };
        assert!((mean(1) / mean(0) - 1.1).abs() < 0.01);
    }
}
