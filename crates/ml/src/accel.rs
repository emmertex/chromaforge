//! A GPU runner for a CLIP tower's transformer blocks, plugged in by a higher layer (the engine wires `lcc-gpu`'s wgpu
//! kernels in, so search uses the graphics card on every OS). The CPU encoder stays the reference: a tower is checked
//! against it on a test input ([`agree`]) before it is used, and any error sends the job back to the CPU.

use std::sync::Arc;

/// One block's weights, linear weights in `[out, in]` order.
#[derive(Clone, Debug, Default)]
pub struct BlockWeights {
    pub ln1_w: Vec<f32>,
    pub ln1_b: Vec<f32>,
    pub q_w: Vec<f32>,
    pub q_b: Vec<f32>,
    pub k_w: Vec<f32>,
    pub k_b: Vec<f32>,
    pub v_w: Vec<f32>,
    pub v_b: Vec<f32>,
    pub o_w: Vec<f32>,
    pub o_b: Vec<f32>,
    pub ln2_w: Vec<f32>,
    pub ln2_b: Vec<f32>,
    pub fc1_w: Vec<f32>,
    pub fc1_b: Vec<f32>,
    pub fc2_w: Vec<f32>,
    pub fc2_b: Vec<f32>,
}

/// A tower's blocks: pre-norm transformer blocks with GELU (erf) MLPs.
#[derive(Clone, Debug)]
pub struct TowerWeights {
    pub hidden: usize,
    pub intermediate: usize,
    pub heads: usize,
    /// Each position attends to itself and earlier ones only (the text tower).
    pub causal: bool,
    pub eps: f32,
    pub blocks: Vec<BlockWeights>,
}

/// Runs a tower's blocks somewhere other than the CPU.
pub trait Accelerator: Send + Sync {
    /// The adapter ("AMD Radeon RX 7900 XTX (Vulkan)").
    fn device(&self) -> String;

    /// `x` (`[batch, seq, hidden]`, row-major) through every block, same shape back.
    fn run_blocks(&self, x: &[f32], batch: usize, seq: usize) -> Result<Vec<f32>, String>;
}

/// Makes an [`Accelerator`] for a tower, or says why there is none.
pub type AccelFactory = Arc<dyn Fn(&TowerWeights) -> Result<Arc<dyn Accelerator>, String> + Send + Sync>;

/// Whether `got` matches the reference `want` within the tolerance the GPU path is held to.
pub fn agree(got: &[f32], want: &[f32]) -> Result<(), String> {
    if got.len() != want.len() {
        return Err(format!("{} values from the GPU, {} from the CPU", got.len(), want.len()));
    }
    let scale = want.iter().fold(0f32, |m, v| m.max(v.abs()));
    let worst = got.iter().zip(want).fold(0f32, |m, (g, w)| if g.is_finite() { m.max((g - w).abs()) } else { f32::INFINITY });
    let tolerance = 2e-3 * (1.0 + scale);
    if worst <= tolerance { Ok(()) } else { Err(format!("the GPU differs from the CPU by {worst} (tolerance {tolerance})")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agreement_has_a_tolerance_and_rejects_nan() {
        assert!(agree(&[1.0, 2.0], &[1.0005, 2.0]).is_ok());
        assert!(agree(&[1.0, 2.5], &[1.0, 2.0]).is_err());
        assert!(agree(&[f32::NAN], &[0.0]).is_err());
        assert!(agree(&[1.0], &[1.0, 2.0]).is_err());
    }
}
