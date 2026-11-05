//! Point prompts (SAM 3's interactive "tracker" head, SAM 2 style): a prompt encoder for
//! positive/negative clicks and a two-way-transformer mask decoder over the image features.
//!
//! The same module serves SAM 2 (`Sam2PromptEncoder` + `Sam2MaskDecoder`, which SAM 3's tracker
//! reuses unchanged): only the input side and the embedding grid differ
//! ([`Tracker::with_geometry`]).
//!
//! Modified work (Apache License 2.0, §4(b)): ported by the project contributors in 2026 from
//! the Python/PyTorch SAM 3 and SAM 2 code of Hugging Face Transformers (`models/sam3_tracker/modeling_sam3_tracker.py`,
//! `models/sam2/modeling_sam2.py`), Copyright The HuggingFace
//! Team and Meta Platforms, Inc.; translated to Rust on candle and restructured. See NOTICE.

use candle_core::{DType, Device, IndexOp, Module, Tensor};
use candle_nn::{Conv2d, Conv2dConfig, ConvTranspose2d, ConvTranspose2dConfig, LayerNorm, VarBuilder};

use crate::nn::{Attention, FeedForward, TORCH_LN_EPS, ln, ln_channels};
use crate::vit::{GRID, IMAGE};
use crate::{Result, nn::to_vec};

const HIDDEN: usize = 256;
const MASK_TOKENS: usize = 4;
/// Low-resolution mask side of SAM 3 (4 × the 72 grid).
pub const LOW_RES: usize = 288;

/// The image features the decoder needs, computed once per image.
pub struct Features {
    /// Level 2 (72 × 72, 256 channels).
    pub image: Tensor,
    /// `conv_s0`(level 0): `[1, 32, 288, 288]`.
    pub s0: Tensor,
    /// `conv_s1`(level 1): `[1, 64, 144, 144]`.
    pub s1: Tensor,
}

/// One click: position normalized to the image (0..1), and whether it includes or excludes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Click {
    pub x: f32,
    pub y: f32,
    pub positive: bool,
}

/// A predicted mask: low-resolution logits (4 × the grid per side) over the (squared) model
/// input, and its quality.
pub struct Prediction {
    pub logits: Vec<f32>,
    pub iou: f32,
    pub object_score: f32,
}

/// Random-Fourier positional encoding of points normalized to 0..1 (`Sam3TrackerPositionalEmbedding`).
struct Fourier {
    pe: Vec<f32>, // [2, 128]
}

impl Fourier {
    fn encode(&self, x: f32, y: f32) -> Vec<f32> {
        let half = self.pe.len() / 2;
        let (cx, cy) = (2.0 * x - 1.0, 2.0 * y - 1.0);
        let proj: Vec<f32> = (0..half)
            .map(|i| {
                let a = cx * self.pe.get(i).copied().unwrap_or(0.0) + cy * self.pe.get(half + i).copied().unwrap_or(0.0);
                2.0 * std::f32::consts::PI * a
            })
            .collect();
        proj.iter().map(|a| a.sin()).chain(proj.iter().map(|a| a.cos())).collect()
    }
}

struct TwoWayBlock {
    self_attn: Attention,
    ln1: LayerNorm,
    t2i: Attention,
    ln2: LayerNorm,
    mlp: FeedForward,
    ln3: LayerNorm,
    ln4: LayerNorm,
    i2t: Attention,
    skip_first_pe: bool,
}

/// `Sam3TrackerAttention` on `[1, L, C]` (the point batch of one is folded away).
fn attn(a: &Attention, q: &Tensor, k: &Tensor, v: &Tensor) -> Result<Tensor> {
    a.forward(q, k, v, None)
}

impl TwoWayBlock {
    fn forward(&self, queries: &Tensor, keys: &Tensor, qpe: &Tensor, kpe: &Tensor) -> Result<(Tensor, Tensor)> {
        let queries = if self.skip_first_pe {
            attn(&self.self_attn, queries, queries, queries)?
        } else {
            let q = (queries + qpe)?;
            (queries + attn(&self.self_attn, &q, &q, queries)?)?
        };
        let queries = self.ln1.forward(&queries)?;
        let (q, k) = ((&queries + qpe)?, (keys + kpe)?);
        let queries = self.ln2.forward(&(&queries + attn(&self.t2i, &q, &k, keys)?)?)?;
        let queries = self.ln3.forward(&(&queries + self.mlp.forward(&queries)?)?)?;
        let (q, k) = ((&queries + qpe)?, (keys + kpe)?);
        let keys = self.ln4.forward(&(keys + attn(&self.i2t, &k, &q, &queries)?)?)?;
        Ok((queries, keys))
    }
}

/// All four decoder outputs for one prompt: mask 0 is the single-mask output, 1..3 the
/// alternatives.
pub struct Decoded {
    /// `[4, side, side]` logits.
    pub masks: Tensor,
    /// Predicted IoU of each mask.
    pub iou: Vec<f32>,
    pub object_score: f32,
}

pub struct Tracker {
    device: Device,
    /// Side of the square model input (click coordinates are scaled to it).
    image: usize,
    point_pe: Fourier,
    image_pe: Tensor,
    point_embed: Tensor,
    not_a_point: Tensor,
    no_mask: Tensor,
    conv_s0: Conv2d,
    conv_s1: Conv2d,
    output_tokens: Tensor,
    blocks: Vec<TwoWayBlock>,
    final_t2i: Attention,
    ln_final: LayerNorm,
    up1: ConvTranspose2d,
    up_ln: LayerNorm,
    up2: ConvTranspose2d,
    hyper: Vec<FeedForward>,
    iou_head: FeedForward,
    obj_head: FeedForward,
}

fn read_vec(vb: &VarBuilder, shape: (usize, usize), name: &str) -> Result<Vec<f32>> {
    to_vec(&vb.get(shape, name)?)
}

impl Tracker {
    /// `vb` at `tracker_model` (SAM 3: 1008 input, 72 × 72 grid).
    pub fn new(vb: VarBuilder) -> Result<Self> {
        Self::with_geometry(vb, IMAGE, GRID)
    }

    /// `vb` at the module holding `prompt_encoder`, `mask_decoder` and `shared_image_embedding`;
    /// `image`: side of the model input; `grid`: side of the image-embedding grid (SAM 2: 1024, 64).
    pub fn with_geometry(vb: VarBuilder, image: usize, grid: usize) -> Result<Self> {
        if image == 0 || grid == 0 || grid > 1024 {
            return Err(crate::Error::Model(format!("mask decoder geometry {image} / {grid}")));
        }
        let device = vb.device().clone();
        let pe = vb.pp("prompt_encoder");
        let md = vb.pp("mask_decoder");
        let image_fourier = Fourier { pe: read_vec(&vb, (2, HIDDEN / 2), "shared_image_embedding.positional_embedding")? };
        // image-wide positional embedding: pixel centres of the grid (72 × 72 for SAM 3)
        let n = grid;
        let mut grid_pe = Vec::with_capacity(n * n * HIDDEN);
        for r in 0..n {
            for c in 0..n {
                grid_pe.extend(image_fourier.encode((c as f32 + 0.5) / n as f32, (r as f32 + 0.5) / n as f32));
            }
        }
        let image_pe = Tensor::from_vec(grid_pe, (1, n * n, HIDDEN), &device)?;
        let t = md.pp("transformer");
        let mut blocks = Vec::new();
        for i in 0..2 {
            let b = t.pp(format!("layers.{i}"));
            blocks.push(TwoWayBlock {
                self_attn: Attention::new(HIDDEN, HIDDEN, 8, b.pp("self_attn"))?,
                ln1: ln(HIDDEN, TORCH_LN_EPS, b.pp("layer_norm1"))?,
                t2i: Attention::new(HIDDEN, HIDDEN / 2, 8, b.pp("cross_attn_token_to_image"))?,
                ln2: ln(HIDDEN, TORCH_LN_EPS, b.pp("layer_norm2"))?,
                mlp: FeedForward::new(HIDDEN, 2048, HIDDEN, 2, false, b.pp("mlp"))?,
                ln3: ln(HIDDEN, TORCH_LN_EPS, b.pp("layer_norm3"))?,
                ln4: ln(HIDDEN, TORCH_LN_EPS, b.pp("layer_norm4"))?,
                i2t: Attention::new(HIDDEN, HIDDEN / 2, 8, b.pp("cross_attn_image_to_token"))?,
                skip_first_pe: i == 0,
            });
        }
        let output_tokens = Tensor::cat(
            &[
                md.get((1, HIDDEN), "obj_score_token.weight")?,
                md.get((1, HIDDEN), "iou_token.weight")?,
                md.get((MASK_TOKENS, HIDDEN), "mask_tokens.weight")?,
            ],
            0,
        )?;
        let up = ConvTranspose2dConfig { stride: 2, ..Default::default() };
        let one = Conv2dConfig::default();
        Ok(Self {
            point_pe: Fourier { pe: read_vec(&pe, (2, HIDDEN / 2), "shared_embedding.positional_embedding")? },
            image_pe,
            point_embed: pe.get((4, HIDDEN), "point_embed.weight")?.to_device(&Device::Cpu)?,
            not_a_point: pe.get((1, HIDDEN), "not_a_point_embed.weight")?.to_device(&Device::Cpu)?,
            no_mask: pe.get((1, HIDDEN), "no_mask_embed.weight")?.reshape((1, HIDDEN, 1, 1))?,
            conv_s0: candle_nn::conv2d(HIDDEN, HIDDEN / 8, 1, one, md.pp("conv_s0"))?,
            conv_s1: candle_nn::conv2d(HIDDEN, HIDDEN / 4, 1, one, md.pp("conv_s1"))?,
            output_tokens,
            blocks,
            final_t2i: Attention::new(HIDDEN, HIDDEN / 2, 8, t.pp("final_attn_token_to_image"))?,
            ln_final: ln(HIDDEN, TORCH_LN_EPS, t.pp("layer_norm_final_attn"))?,
            up1: candle_nn::conv_transpose2d(HIDDEN, HIDDEN / 4, 2, up, md.pp("upscale_conv1"))?,
            up_ln: ln(HIDDEN / 4, 1e-6, md.pp("upscale_layer_norm"))?,
            up2: candle_nn::conv_transpose2d(HIDDEN / 4, HIDDEN / 8, 2, up, md.pp("upscale_conv2"))?,
            hyper: (0..MASK_TOKENS)
                .map(|i| FeedForward::new(HIDDEN, HIDDEN, HIDDEN / 8, 3, false, md.pp(format!("output_hypernetworks_mlps.{i}"))))
                .collect::<Result<_>>()?,
            iou_head: FeedForward::new(HIDDEN, HIDDEN, MASK_TOKENS, 3, true, md.pp("iou_prediction_head"))?,
            obj_head: FeedForward::new(HIDDEN, HIDDEN, 1, 3, false, md.pp("pred_obj_score_head"))?,
            device,
            image,
        })
    }

    /// The decoder's image features from the tracker neck's levels (288, 144, 72 per side).
    pub fn features(&self, levels: &[Tensor]) -> Result<Features> {
        let [l0, l1, l2, ..] = levels else { return Err(crate::Error::Model("tracker needs three feature levels".into())) };
        Ok(Features { image: l2.clone(), s0: self.conv_s0.forward(l0)?, s1: self.conv_s1.forward(l1)? })
    }

    /// Sparse prompt embeddings `[1, n + 1, 256]` (a padding point closes the list).
    pub fn embed_clicks(&self, clicks: &[Click]) -> Result<Tensor> {
        self.embed_prompt(clicks, None)
    }

    /// Sparse prompt embeddings of `clicks` and an optional box (x0, y0, x1, y1, normalized):
    /// SAM 2 gives a box as two points labelled 2 (top left) and 3 (bottom right) ahead of the
    /// clicks (`Sam2ImagePredictor._predict`); a padding point closes the list.
    pub fn embed_prompt(&self, clicks: &[Click], rect: Option<[f32; 4]>) -> Result<Tensor> {
        let point_embed = to_vec(&self.point_embed)?;
        let not_a_point = to_vec(&self.not_a_point)?;
        let corners: Vec<(f32, f32, usize)> = rect.map(|r| vec![(r[0], r[1], 2), (r[2], r[3], 3)]).unwrap_or_default();
        let points: Vec<(f32, f32, usize)> = corners.into_iter().chain(clicks.iter().map(|c| (c.x, c.y, usize::from(c.positive)))).collect();
        let mut rows = Vec::with_capacity((points.len() + 1) * HIDDEN);
        for (x, y, label) in &points {
            // the reference shifts pixel coordinates (in the square input) by half a pixel
            let side = self.image as f32;
            let (x, y) = ((x * side + 0.5) / side, (y * side + 0.5) / side);
            let e = self.point_pe.encode(x, y);
            rows.extend(e.iter().enumerate().map(|(i, v)| v + point_embed.get(label * HIDDEN + i).copied().unwrap_or(0.0)));
        }
        rows.extend_from_slice(&not_a_point);
        Ok(Tensor::from_vec(rows, (1, points.len() + 1, HIDDEN), &self.device)?)
    }

    /// Predict masks for `clicks`. `multimask`: the three alternatives (best by predicted IoU is
    /// returned), else the single-mask output with SAM 2's stability fallback.
    pub fn predict(&self, f: &Features, clicks: &[Click], multimask: bool) -> Result<Prediction> {
        self.predict_prompt(f, clicks, None, multimask)
    }

    /// [`Self::predict`] with an optional box prompt (x0, y0, x1, y1, normalized).
    pub fn predict_prompt(&self, f: &Features, clicks: &[Click], rect: Option<[f32; 4]>, multimask: bool) -> Result<Prediction> {
        let Decoded { masks, iou, object_score } = self.decode_prompt(f, clicks, rect)?;
        let pick = |i: usize| -> Result<Prediction> {
            Ok(Prediction { logits: to_vec(&masks.i(i)?)?, iou: iou.get(i).copied().unwrap_or(0.0), object_score })
        };
        let best_multi = || (1..MASK_TOKENS).max_by(|a, b| iou.get(*a).partial_cmp(&iou.get(*b)).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(1);
        if multimask {
            return pick(best_multi());
        }
        // single-mask output unless it is unstable (area changes > 2 % when the threshold moves ±0.05)
        let single = masks.i(0)?;
        let inter = single.gt(0.05)?.to_dtype(DType::F32)?.sum_all()?.to_scalar::<f32>()?;
        let union = single.gt(-0.05)?.to_dtype(DType::F32)?.sum_all()?.to_scalar::<f32>()?;
        let stability = if union > 0.0 { inter / union } else { 1.0 };
        if stability >= 0.98 { pick(0) } else { pick(best_multi()) }
    }

    /// Run the decoder for `clicks`: all four masks with their predicted IoU and the object score.
    pub fn decode(&self, f: &Features, clicks: &[Click]) -> Result<Decoded> {
        self.decode_prompt(f, clicks, None)
    }

    /// [`Self::decode`] with an optional box prompt (x0, y0, x1, y1, normalized).
    pub fn decode_prompt(&self, f: &Features, clicks: &[Click], rect: Option<[f32; 4]>) -> Result<Decoded> {
        let sparse = self.embed_prompt(clicks, rect)?;
        let tokens = Tensor::cat(&[self.output_tokens.unsqueeze(0)?, sparse], 1)?; // [1, T, 256]
        let image = f.image.broadcast_add(&self.no_mask)?; // dense prompt: "no mask"
        let (b, c, h, w) = image.dims4()?;
        let keys = image.flatten_from(2)?.transpose(1, 2)?.contiguous()?; // [1, HW, 256]
        let (mut q, mut k) = (tokens.clone(), keys);
        for blk in &self.blocks {
            (q, k) = blk.forward(&q, &k, &tokens, &self.image_pe)?;
        }
        let qf = (&q + &tokens)?;
        let kf = (&k + &self.image_pe)?;
        let q = self.ln_final.forward(&(&q + attn(&self.final_t2i, &qf, &kf, &k)?)?)?;
        let iou_token = q.i((.., 1, ..))?;
        let image = k.transpose(1, 2)?.reshape((b, c, h, w))?;
        let up = (self.up1.forward(&image)? + &f.s1)?;
        let up = ln_channels(&up, &self.up_ln)?.gelu_erf()?;
        let up = (self.up2.forward(&up)? + &f.s0)?.gelu_erf()?; // [1, 32, 288, 288]
        let (_, uc, uh, uw) = up.dims4()?;
        let hyper = (0..MASK_TOKENS)
            .map(|i| self.hyper.get(i).map_or(Err(crate::Error::Model("hypernetwork".into())), |m| m.forward(&q.i((.., 2 + i, ..))?)))
            .collect::<Result<Vec<_>>>()?;
        let hyper = Tensor::stack(&hyper, 1)?; // [1, 4, 32]
        let masks = hyper.matmul(&up.reshape((b, uc, uh * uw))?)?.reshape((b, MASK_TOKENS, uh, uw))?;
        let iou = to_vec(&self.iou_head.forward(&iou_token)?)?;
        let object_score = to_vec(&self.obj_head.forward(&q.i((.., 0, ..))?)?)?.first().copied().unwrap_or(0.0);
        Ok(Decoded { masks: masks.i(0)?, iou, object_score })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SAM 2's box prompt: two corner points (labels 2 and 3) ahead of the clicks and the padding
    /// point; without a box, the clicks alone. The corners carry their own label embeddings.
    #[test]
    fn a_box_prompt_is_two_labelled_corners_ahead_of_the_clicks() {
        let map = candle_nn::VarMap::new();
        let vb = VarBuilder::from_varmap(&map, DType::F32, &Device::Cpu);
        let t = Tracker::with_geometry(vb, 128, 8).unwrap();
        let clicks = [Click { x: 0.3, y: 0.6, positive: true }, Click { x: 1.0, y: 0.0, positive: false }];
        assert_eq!(t.embed_clicks(&clicks).unwrap().dims(), &[1, 3, HIDDEN]);
        let boxed = t.embed_prompt(&clicks, Some([0.1, 0.2, 0.6, 0.9])).unwrap();
        assert_eq!(boxed.dims(), &[1, 5, HIDDEN]);
        assert_eq!(t.embed_prompt(&[], Some([0.1, 0.2, 0.6, 0.9])).unwrap().dims(), &[1, 3, HIDDEN]);
        // the clicks follow the corners unchanged
        let plain = to_vec(&t.embed_clicks(&clicks).unwrap()).unwrap();
        let rows = to_vec(&boxed).unwrap();
        assert_eq!(&rows[2 * HIDDEN..], &plain[..]);
    }
}
