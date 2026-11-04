//! Multi-threaded CPU kernels for the hot paths of the SAM 2 encoder.
//!
//! candle runs element-wise operations (bias, GELU, residual adds) on one thread, and its
//! batched matmul is slow for many small matrices (the Hiera windows). These kernels split the
//! same arithmetic over rayon's threads; on other devices the plain candle operations run.
//! Own design (OneSlips).

use candle_core::{CpuStorage, CustomOp2, CustomOp3, Layout, Shape, Tensor};
use candle_nn::Linear;
use rayon::prelude::*;

use crate::Result;

type CResult<T> = candle_core::Result<T>;

fn f32s<'a>(s: &'a CpuStorage, l: &Layout, what: &str) -> CResult<&'a [f32]> {
    let CpuStorage::F32(v) = s else { candle_core::bail!("{what}: f32 expected") };
    let Some((a, b)) = l.contiguous_offsets() else { candle_core::bail!("{what}: contiguous input expected") };
    v.get(a..b).ok_or_else(|| candle_core::Error::Msg(format!("{what}: storage too small")))
}

#[inline]
fn gelu(v: f32) -> f32 {
    (candle_core::cpu::erf::erf_f32(v * std::f32::consts::FRAC_1_SQRT_2) + 1.0) * 0.5 * v
}

/// Rows per parallel task for row-wise kernels.
const ROWS: usize = 64;

/// `y + bias`, then GELU (erf) when `gelu`.
struct BiasAct {
    gelu: bool,
}

impl CustomOp2 for BiasAct {
    fn name(&self) -> &'static str {
        "lc-bias-act"
    }

    fn cpu_fwd(&self, s1: &CpuStorage, l1: &Layout, s2: &CpuStorage, l2: &Layout) -> CResult<(CpuStorage, Shape)> {
        let (y, b) = (f32s(s1, l1, "bias-act")?, f32s(s2, l2, "bias-act bias")?);
        let c = b.len();
        if c == 0 || y.len() % c != 0 {
            candle_core::bail!("bias-act: {} values for {c} channels", y.len())
        }
        let mut out = vec![0f32; y.len()];
        out.par_chunks_mut(c * ROWS).zip(y.par_chunks(c * ROWS)).for_each(|(o, y)| {
            for (orow, yrow) in o.chunks_exact_mut(c).zip(y.chunks_exact(c)) {
                for ((o, y), b) in orow.iter_mut().zip(yrow).zip(b) {
                    let v = y + b;
                    *o = if self.gelu { gelu(v) } else { v };
                }
            }
        });
        Ok((CpuStorage::F32(out), l1.shape().clone()))
    }
}

/// `y + bias + residual`.
struct BiasResidual;

impl CustomOp3 for BiasResidual {
    fn name(&self) -> &'static str {
        "lc-bias-residual"
    }

    fn cpu_fwd(&self, s1: &CpuStorage, l1: &Layout, s2: &CpuStorage, l2: &Layout, s3: &CpuStorage, l3: &Layout) -> CResult<(CpuStorage, Shape)> {
        let (y, b, r) = (f32s(s1, l1, "bias-residual")?, f32s(s2, l2, "bias-residual bias")?, f32s(s3, l3, "bias-residual residual")?);
        let c = b.len();
        if c == 0 || y.len() % c != 0 || r.len() != y.len() {
            candle_core::bail!("bias-residual: {} values, {} residual, {c} channels", y.len(), r.len())
        }
        let mut out = vec![0f32; y.len()];
        out.par_chunks_mut(c * ROWS).zip(y.par_chunks(c * ROWS)).zip(r.par_chunks(c * ROWS)).for_each(|((o, y), r)| {
            for ((orow, yrow), rrow) in o.chunks_exact_mut(c).zip(y.chunks_exact(c)).zip(r.chunks_exact(c)) {
                for (((o, y), b), r) in orow.iter_mut().zip(yrow).zip(b).zip(rrow) {
                    *o = r + (y + b);
                }
            }
        });
        Ok((CpuStorage::F32(out), l1.shape().clone()))
    }
}

/// What follows a linear layer.
#[derive(Clone, Copy, PartialEq)]
pub enum Act {
    None,
    Gelu,
}

/// `x · Wᵀ + b` over the last axis (then `act`, then `+ residual`), as one 2-D matmul and a
/// multi-threaded epilogue on the CPU.
pub fn linear(x: &Tensor, l: &Linear, act: Act, residual: Option<&Tensor>) -> Result<Tensor> {
    let w = l.weight();
    let (out_c, in_c) = w.dims2()?;
    let dims = x.dims().to_vec();
    let rows = x.elem_count() / in_c.max(1);
    let y = x.reshape((rows, in_c))?.matmul(&w.t()?)?;
    let mut shape = dims;
    if let Some(last) = shape.last_mut() {
        *last = out_c;
    }
    let Some(b) = l.bias() else {
        let y = if act == Act::Gelu { y.gelu_erf()? } else { y };
        let y = y.reshape(shape)?;
        return Ok(match residual {
            Some(r) => (r + y)?,
            None => y,
        });
    };
    if !x.device().is_cpu() {
        let y = y.broadcast_add(b)?;
        let y = if act == Act::Gelu { y.gelu_erf()? } else { y };
        let y = y.reshape(shape)?;
        return Ok(match residual {
            Some(r) => (r + y)?,
            None => y,
        });
    }
    let y = match residual {
        Some(r) if act == Act::None => y.apply_op3_no_bwd(b, &r.contiguous()?.reshape((rows, out_c))?, &BiasResidual)?,
        Some(r) => (r.reshape((rows, out_c))? + y.apply_op2_no_bwd(b, &BiasAct { gelu: true })?)?,
        None => y.apply_op2_no_bwd(b, &BiasAct { gelu: act == Act::Gelu })?,
    };
    Ok(y.reshape(shape)?)
}

/// Queries per parallel attention task.
const TILE: usize = 16;

#[inline]
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut acc = [0f32; 8];
    let n = a.len().min(b.len());
    let (ca, ra) = a.get(..n).unwrap_or_default().as_chunks::<8>();
    let (cb, rb) = b.get(..n).unwrap_or_default().as_chunks::<8>();
    let tail: f32 = ra.iter().zip(rb).map(|(x, y)| x * y).sum();
    for (x, y) in ca.iter().zip(cb) {
        for ((a, x), y) in acc.iter_mut().zip(x).zip(y) {
            *a += x * y;
        }
    }
    acc.iter().sum::<f32>() + tail
}

/// Softmax attention of `q` `[BH, Lq, d]` over `k`, `v` `[BH, Lk, d]` (contiguous).
struct Attention {
    scale: f32,
}

impl CustomOp3 for Attention {
    fn name(&self) -> &'static str {
        "lc-attention"
    }

    fn cpu_fwd(&self, s1: &CpuStorage, l1: &Layout, s2: &CpuStorage, l2: &Layout, s3: &CpuStorage, l3: &Layout) -> CResult<(CpuStorage, Shape)> {
        let (q, k, v) = (f32s(s1, l1, "attention q")?, f32s(s2, l2, "attention k")?, f32s(s3, l3, "attention v")?);
        let (bh, lq, d) = l1.shape().dims3()?;
        let (bk, lk, dk) = l2.shape().dims3()?;
        if bk != bh || dk != d || l3.shape().dims() != l2.shape().dims() || d == 0 || lk == 0 {
            candle_core::bail!("attention: q {:?}, k {:?}, v {:?}", l1.shape(), l2.shape(), l3.shape())
        }
        let scale = self.scale;
        let mut out = vec![0f32; q.len()];
        // tasks: (head, tile of queries)
        out.par_chunks_mut(lq * d).zip(q.par_chunks(lq * d)).zip(k.par_chunks(lk * d).zip(v.par_chunks(lk * d))).for_each(|((o, q), (k, v))| {
            o.par_chunks_mut(TILE * d).zip(q.par_chunks(TILE * d)).for_each(|(o, q)| {
                let n = q.len() / d;
                let mut s = vec![0f32; n * lk];
                for (j, kj) in k.chunks_exact(d).enumerate() {
                    for (i, qi) in q.chunks_exact(d).enumerate() {
                        if let Some(x) = s.get_mut(i * lk + j) {
                            *x = dot(qi, kj) * scale;
                        }
                    }
                }
                for row in s.chunks_exact_mut(lk) {
                    let m = row.iter().fold(f32::NEG_INFINITY, |m, x| m.max(*x));
                    let mut sum = 0f32;
                    for x in row.iter_mut() {
                        *x = (*x - m).exp();
                        sum += *x;
                    }
                    let inv = 1.0 / sum;
                    row.iter_mut().for_each(|x| *x *= inv);
                }
                for (j, vj) in v.chunks_exact(d).enumerate() {
                    for (i, oi) in o.chunks_exact_mut(d).enumerate() {
                        let p = s.get(i * lk + j).copied().unwrap_or(0.0);
                        for (o, x) in oi.iter_mut().zip(vj) {
                            *o += p * x;
                        }
                    }
                }
            });
        });
        Ok((CpuStorage::F32(out), l1.shape().clone()))
    }
}

/// Scaled dot-product attention on `[B, H, L, d]` (no mask).
pub fn attention(q: &Tensor, k: &Tensor, v: &Tensor, scale: f64) -> Result<Tensor> {
    if !q.device().is_cpu() {
        return crate::nn::sdpa(q, k, v, None, scale);
    }
    let (b, h, lq, d) = q.dims4()?;
    let lk = k.dim(2)?;
    let q3 = q.contiguous()?.reshape((b * h, lq, d))?;
    let k3 = k.contiguous()?.reshape((b * h, lk, d))?;
    let v3 = v.contiguous()?.reshape((b * h, lk, d))?;
    Ok(q3.apply_op3_no_bwd(&k3, &v3, &Attention { scale: scale as f32 })?.reshape((b, h, lq, d))?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use candle_core::{Device, Module};

    fn close(a: &Tensor, b: &Tensor) -> f32 {
        let (a, b) = (a.flatten_all().unwrap().to_vec1::<f32>().unwrap(), b.flatten_all().unwrap().to_vec1::<f32>().unwrap());
        a.iter().zip(&b).fold(0f32, |m, (x, y)| m.max((x - y).abs()))
    }

    #[test]
    fn kernels_match_candle() {
        let d = Device::Cpu;
        let x = Tensor::randn(0f32, 1., (3, 70, 24), &d).unwrap();
        let l = Linear::new(Tensor::randn(0f32, 1., (40, 24), &d).unwrap(), Some(Tensor::randn(0f32, 1., 40, &d).unwrap()));
        let r = Tensor::randn(0f32, 1., (3, 70, 40), &d).unwrap();
        let want = l.forward(&x).unwrap();
        assert!(close(&linear(&x, &l, Act::None, None).unwrap(), &want) < 1e-4);
        assert!(close(&linear(&x, &l, Act::Gelu, None).unwrap(), &want.gelu_erf().unwrap()) < 1e-4);
        assert!(close(&linear(&x, &l, Act::None, Some(&r)).unwrap(), &(&want + &r).unwrap()) < 1e-4);
        assert!(close(&linear(&x, &l, Act::Gelu, Some(&r)).unwrap(), &(want.gelu_erf().unwrap() + &r).unwrap()) < 1e-4);

        let q = Tensor::randn(0f32, 1., (2, 3, 37, 20), &d).unwrap();
        let k = Tensor::randn(0f32, 1., (2, 3, 50, 20), &d).unwrap();
        let v = Tensor::randn(0f32, 1., (2, 3, 50, 20), &d).unwrap();
        let want = crate::nn::sdpa(&q, &k, &v, None, 0.3).unwrap();
        assert!(close(&attention(&q, &k, &v, 0.3).unwrap(), &want) < 1e-4);
    }
}
