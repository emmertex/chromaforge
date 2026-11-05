//! Plain-`Vec<f32>` rows and the CPU operations the OWLv2 port ([`crate::owl`]) runs on them:
//! dense layers (candle matrix products), layer norm, GELU, and reading a checkpoint's tensors as
//! vectors.
//!
//! Ported from OneSlips' fork of the upstream app (MIT), where it also serves its BiRefNet port; only what
//! OWLv2 uses is here.

use candle_core::{Device, Tensor};
use rayon::prelude::*;

use crate::{Error, Result};

/// An activation applied after a layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    None,
    Gelu,
}

/// Applies `act` and the per-column bias to `rows × cols` values in place, in parallel.
pub fn bias_act(x: &mut [f32], cols: usize, bias: Option<&[f32]>, act: Act) {
    if cols == 0 || (bias.is_none() && act == Act::None) {
        return;
    }
    x.par_chunks_mut(cols * 64usize.div_ceil(cols).max(1)).for_each(|rows| {
        for row in rows.chunks_mut(cols) {
            if let Some(b) = bias {
                row.iter_mut().zip(b).for_each(|(v, b)| *v += b);
            }
            match act {
                Act::None => {}
                Act::Gelu => row.iter_mut().for_each(|v| *v = gelu(*v)),
            }
        }
    });
}

/// `nn.GELU()` (the exact, erf-based form; erf to ~1e-7, Abramowitz & Stegun 7.1.26).
pub fn gelu(x: f32) -> f32 {
    let z = (x * std::f32::consts::FRAC_1_SQRT_2).abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * z);
    let poly = t * (0.254_829_6 + t * (-0.284_496_74 + t * (1.421_413_7 + t * (-1.453_152_1 + t * 1.061_405_4))));
    let erf = 1.0 - poly * (-z * z).exp();
    0.5 * x * (1.0 + if x < 0.0 { -erf } else { erf })
}

pub fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// A dense layer `x · W + b` over rows (`W` stored `[in, out]`).
pub struct Dense {
    w: Tensor,
    b: Option<Vec<f32>>,
    pub inp: usize,
    pub out: usize,
}

impl Dense {
    /// From a row-major `[out, in]` matrix (PyTorch's `nn.Linear` layout).
    pub fn from_out_in(w: &[f32], out: usize, inp: usize, b: Option<Vec<f32>>, dev: &Device) -> Result<Self> {
        if Some(w.len()) != out.checked_mul(inp) || b.as_ref().is_some_and(|b| b.len() != out) {
            return Err(Error::Model(format!("dense layer: {} weights for {out} × {inp}", w.len())));
        }
        let mut t = vec![0f32; w.len()];
        for o in 0..out {
            for i in 0..inp {
                if let (Some(d), Some(s)) = (t.get_mut(i * out + o), w.get(o * inp + i)) {
                    *d = *s;
                }
            }
        }
        Self::from_in_out(t, inp, out, b, dev)
    }

    /// From a row-major `[in, out]` matrix.
    pub fn from_in_out(w: Vec<f32>, inp: usize, out: usize, b: Option<Vec<f32>>, dev: &Device) -> Result<Self> {
        if Some(w.len()) != out.checked_mul(inp) || b.as_ref().is_some_and(|b| b.len() != out) {
            return Err(Error::Model(format!("dense layer: {} weights for {inp} × {out}", w.len())));
        }
        Ok(Self { w: Tensor::from_vec(w, (inp, out), dev)?, b, inp, out })
    }

    /// `x` (`rows × in`, row-major) → `rows × out`, with the bias and `act` applied.
    pub fn forward(&self, x: &[f32], act: Act) -> Result<Vec<f32>> {
        let mut y = matmul(x, self.inp, &self.w)?;
        bias_act(&mut y, self.out, self.b.as_deref(), act);
        Ok(y)
    }
}

/// `a` (`rows × k`, row-major) times `w` (`[k, n]`), on `w`'s device.
pub fn matmul(a: &[f32], k: usize, w: &Tensor) -> Result<Vec<f32>> {
    if k == 0 || !a.len().is_multiple_of(k) {
        return Err(Error::Model(format!("matrix of {} values with rows of {k}", a.len())));
    }
    let rows = a.len() / k;
    if rows == 0 {
        return Ok(Vec::new());
    }
    let t = Tensor::from_slice(a, (rows, k), w.device())?;
    Ok(t.matmul(w)?.flatten_all()?.to_vec1::<f32>()?)
}

/// `nn.LayerNorm` over rows of `c` values, in place.
pub fn layer_norm(x: &mut [f32], gamma: &[f32], beta: &[f32], eps: f32) {
    let c = gamma.len();
    if c == 0 {
        return;
    }
    x.par_chunks_mut(c * 16).for_each(|rows| {
        for row in rows.chunks_mut(c) {
            let mean = row.iter().map(|v| f64::from(*v)).sum::<f64>() / c as f64;
            let var = row.iter().map(|v| (f64::from(*v) - mean).powi(2)).sum::<f64>() / c as f64;
            let inv = 1.0 / (var + f64::from(eps)).sqrt();
            for ((v, g), b) in row.iter_mut().zip(gamma).zip(beta) {
                *v = ((f64::from(*v) - mean) * inv) as f32 * g + b;
            }
        }
    });
}

/// Named tensors of a checkpoint as `f32` vectors.
pub struct Params<'a> {
    pub weights: &'a crate::weights::Weights,
    pub dev: Device,
}

impl Params<'_> {
    pub fn has(&self, name: &str) -> bool {
        self.weights.contains(name)
    }

    /// Tensor `name`: values and shape.
    pub fn get(&self, name: &str) -> Result<(Vec<f32>, Vec<usize>)> {
        let t = self.weights.read(name)?;
        let shape = t.dims().to_vec();
        Ok((t.flatten_all()?.to_vec1::<f32>()?, shape))
    }

    /// Tensor `name`, which must hold `n` values.
    pub fn vec(&self, name: &str, n: usize) -> Result<Vec<f32>> {
        let (v, _) = self.get(name)?;
        if v.len() != n {
            return Err(Error::Model(format!("tensor `{name}` has {} values, expected {n}", v.len())));
        }
        Ok(v)
    }

    /// `nn.Linear` `name` (`weight` `[out, in]`, optional `bias`).
    pub fn linear(&self, name: &str) -> Result<Dense> {
        let (w, shape) = self.get(&format!("{name}.weight"))?;
        let &[out, inp] = shape.as_slice() else { return Err(Error::Model(format!("{name}: weight of shape {shape:?}"))) };
        let b = if self.has(&format!("{name}.bias")) { Some(self.vec(&format!("{name}.bias"), out)?) } else { None };
        Dense::from_out_in(&w, out, inp, b, &self.dev)
    }

    /// `nn.LayerNorm` `name` of `n` channels: (gamma, beta).
    pub fn norm(&self, name: &str, n: usize) -> Result<(Vec<f32>, Vec<f32>)> {
        Ok((self.vec(&format!("{name}.weight"), n)?, self.vec(&format!("{name}.bias"), n)?))
    }
}

/// Per-stage wall-clock timings on stderr when `<PREFIX>_PROFILE` is set.
pub struct Profile {
    on: bool,
    what: &'static str,
    t: std::time::Instant,
}

impl Profile {
    pub fn new(what: &'static str) -> Self {
        Self { on: lcc_brand::env_is_set("PROFILE"), what, t: std::time::Instant::now() }
    }

    pub fn step(&mut self, stage: &str) {
        if self.on {
            eprintln!("{} {stage}: {:.1} ms", self.what, self.t.elapsed().as_secs_f64() * 1e3);
            self.t = std::time::Instant::now();
        }
    }
}

/// `a += b` elementwise, in parallel.
pub fn add_assign(a: &mut [f32], b: &[f32]) {
    a.par_chunks_mut(4096).zip(b.par_chunks(4096)).for_each(|(x, y)| x.iter_mut().zip(y).for_each(|(x, y)| *x += y));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gelu_layer_norm_and_dense() {
        assert!((gelu(1.0) - 0.841_344_7).abs() < 1e-6);
        assert!(gelu(-10.0).abs() < 1e-6);
        let mut x = vec![1.0, 2.0, 3.0, 4.0];
        layer_norm(&mut x, &[1.0, 1.0], &[0.0, 0.5], 1e-5);
        assert!((x[0] + 1.0).abs() < 1e-3 && (x[1] - 1.5).abs() < 1e-3);
        // [out, in] = [[1, 2], [3, 4]], bias [10, 20]: row (1, 1) → (13, 27)
        let d = Dense::from_out_in(&[1.0, 2.0, 3.0, 4.0], 2, 2, Some(vec![10.0, 20.0]), &Device::Cpu).unwrap();
        assert_eq!(d.forward(&[1.0, 1.0], Act::None).unwrap(), vec![13.0, 27.0]);
        assert!(Dense::from_out_in(&[1.0], 2, 2, None, &Device::Cpu).is_err());
        assert!(d.forward(&[1.0, 1.0, 1.0], Act::None).is_err());
        let mut a = vec![1.0; 5];
        add_assign(&mut a, &[1.0; 5]);
        assert_eq!(a, vec![2.0; 5]);
    }
}
