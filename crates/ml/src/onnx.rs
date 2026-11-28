//! CLIP's vision tower through ONNX Runtime (issue #491, used first by orientation detection, #490).
//!
//! The model is not downloaded as `.onnx`: [`vision_tower_onnx`] writes the tower the CPU encoder already loaded
//! ([`VisionTowerWeights`], from the same `model.safetensors`) as an ONNX graph in memory, so there is one checkpoint,
//! one licence and one SHA-256 to verify. [`OrtVision`] runs it on the graphics card through `lcc-onnxrt` (WebGPU,
//! Core ML or CUDA) as an [`Accelerator`]; like the wgpu runner it is checked against the CPU on a test input before
//! it is used, and any error sends the job back to the CPU encoder, which stays the reference.
//!
//! The graph is the forward pass of `clip.rs` written as ONNX operators (opset 17): `Conv` patch embedding, class
//! token and position embedding, pre-`LayerNormalization`, pre-norm transformer blocks (multi-head attention with
//! `Softmax`, exact-`Erf` GELU MLP), post-norm on the class token and the visual projection.

use crate::accel::{Accelerator, VisionTowerWeights};
use lcc_onnxrt::graph::{Attr, Dim, Graph};

/// The operator set the graph is written for (`LayerNormalization` is opset 17).
pub const OPSET: i64 = 17;

/// `w` (`[rows, cols]`, row-major) transposed to `[cols, rows]`.
fn transpose(w: &[f32], rows: usize, cols: usize) -> Result<Vec<f32>, String> {
    if rows.checked_mul(cols) != Some(w.len()) {
        return Err(format!("a weight of {} values is not {rows}×{cols}", w.len()));
    }
    let mut out = vec![0.0f32; w.len()];
    for r in 0..rows {
        for c in 0..cols {
            if let (Some(dst), Some(src)) = (out.get_mut(c * rows + r), w.get(r * cols + c)) {
                *dst = *src;
            }
        }
    }
    Ok(out)
}

/// `y = x·Wᵀ + b` for a linear layer stored `[out, in]`.
fn linear(g: &mut Graph, x: &str, name: &str, w: &[f32], b: Option<&[f32]>, inp: usize, out: usize) -> Result<String, String> {
    let wt = g.init_f32(&format!("{name}.w"), &[inp, out], &transpose(w, out, inp)?)?;
    let y = g.node("MatMul", &[x, &wt], &[]);
    match b {
        Some(b) => {
            let bias = g.init_f32(&format!("{name}.b"), &[out], b)?;
            Ok(g.node("Add", &[&y, &bias], &[]))
        }
        None => Ok(y),
    }
}

fn layer_norm(g: &mut Graph, x: &str, name: &str, w: &[f32], b: &[f32], eps: f32) -> Result<String, String> {
    let scale = g.init_f32(&format!("{name}.w"), &[w.len()], w)?;
    let bias = g.init_f32(&format!("{name}.b"), &[b.len()], b)?;
    Ok(g.node("LayerNormalization", &[x, &scale, &bias], &[Attr::Int("axis", -1), Attr::Float("epsilon", eps)]))
}

/// The ONNX bytes of CLIP's vision tower: input `pixels` `[batch, 3, img_h, img_w]` (preprocessed, as
/// [`crate::preprocess_image`] makes it), output `embedding` `[batch, embed_dim]` (not yet normalised).
pub fn vision_tower_onnx(v: &VisionTowerWeights) -> Result<Vec<u8>, String> {
    let (hidden, heads, p) = (v.hidden, v.heads, v.patch_size);
    if hidden == 0 || heads == 0 || !hidden.is_multiple_of(heads) || p == 0 || !v.img_w.is_multiple_of(p) || !v.img_h.is_multiple_of(p) {
        return Err(format!("a vision tower the export doesn't take (hidden {hidden}, heads {heads}, patch {p}, {}×{})", v.img_w, v.img_h));
    }
    let head_dim = hidden / heads;
    let patches = (v.img_w / p) * (v.img_h / p);
    if v.num_positions != patches + 1 {
        return Err(format!("{} positions for {patches} patches and the class token", v.num_positions));
    }
    let to_i64 = |n: usize| i64::try_from(n).map_err(|_| "a dimension too large".to_string());
    let (hidden_i, heads_i, head_dim_i) = (to_i64(hidden)?, to_i64(heads)?, to_i64(head_dim)?);
    let mut g = Graph::new();
    g.input("pixels", &[Dim::Named("batch"), Dim::Fixed(3), Dim::Fixed(v.img_h as u64), Dim::Fixed(v.img_w as u64)]);

    // patch embedding → [batch, patches, hidden]
    let patch_w = g.init_f32("patch.w", &[hidden, 3, p, p], &v.patch_weight)?;
    let stride = to_i64(p)?;
    let conv =
        g.node("Conv", &["pixels", &patch_w], &[Attr::Ints("kernel_shape", vec![stride, stride]), Attr::Ints("strides", vec![stride, stride])]);
    let flat_shape = g.init_i64("shape.flat", &[3], &[0, hidden_i, -1])?;
    let flat = g.node("Reshape", &[&conv, &flat_shape], &[]);
    let tokens = g.node("Transpose", &[&flat], &[Attr::Ints("perm", vec![0, 2, 1])]);

    // the class token for every image: the first patch times zero plus the embedding (no shape arithmetic needed)
    let zero_i = g.init_i64("i.0", &[1], &[0])?;
    let one_i = g.init_i64("i.1", &[1], &[1])?;
    let first = g.node("Slice", &[&tokens, &zero_i, &one_i, &one_i], &[]);
    let zero = g.init_f32("f.0", &[], &[0.0])?;
    let cleared = g.node("Mul", &[&first, &zero], &[]);
    let cls_emb = g.init_f32("cls", &[1, 1, hidden], &v.class_embedding)?;
    let cls = g.node("Add", &[&cleared, &cls_emb], &[]);
    let seq = g.node("Concat", &[&cls, &tokens], &[Attr::Int("axis", 1)]);
    let pos = g.init_f32("pos", &[1, v.num_positions, hidden], &v.position_embedding)?;
    let embedded = g.node("Add", &[&seq, &pos], &[]);
    let mut x = layer_norm(&mut g, &embedded, "pre_ln", &v.pre_ln_w, &v.pre_ln_b, v.eps)?;

    let heads_shape = g.init_i64("shape.heads", &[4], &[0, 0, heads_i, head_dim_i])?;
    let merge_shape = g.init_i64("shape.merge", &[3], &[0, 0, hidden_i])?;
    let scale = g.init_f32("attn.scale", &[], &[1.0 / (head_dim as f32).sqrt()])?;
    let inv_sqrt2 = g.init_f32("gelu.inv_sqrt2", &[], &[std::f32::consts::FRAC_1_SQRT_2])?;
    let one = g.init_f32("f.1", &[], &[1.0])?;
    let half = g.init_f32("f.half", &[], &[0.5])?;
    let inter = v.intermediate;
    for (i, b) in v.blocks.iter().enumerate() {
        let n = format!("block{i}");
        let normed = layer_norm(&mut g, &x, &format!("{n}.ln1"), &b.ln1_w, &b.ln1_b, v.eps)?;
        let split = |g: &mut Graph, name: &str, w: &[f32], bias: &[f32], perm: Vec<i64>| -> Result<String, String> {
            let y = linear(g, &normed, &format!("{n}.{name}"), w, Some(bias), hidden, hidden)?;
            let r = g.node("Reshape", &[&y, &heads_shape], &[]);
            Ok(g.node("Transpose", &[&r], &[Attr::Ints("perm", perm)]))
        };
        let q = split(&mut g, "q", &b.q_w, &b.q_b, vec![0, 2, 1, 3])?;
        let kt = split(&mut g, "k", &b.k_w, &b.k_b, vec![0, 2, 3, 1])?;
        let val = split(&mut g, "v", &b.v_w, &b.v_b, vec![0, 2, 1, 3])?;
        let scores = g.node("MatMul", &[&q, &kt], &[]);
        let scaled = g.node("Mul", &[&scores, &scale], &[]);
        let probs = g.node("Softmax", &[&scaled], &[Attr::Int("axis", -1)]);
        let ctx = g.node("MatMul", &[&probs, &val], &[]);
        let ctx_t = g.node("Transpose", &[&ctx], &[Attr::Ints("perm", vec![0, 2, 1, 3])]);
        let merged = g.node("Reshape", &[&ctx_t, &merge_shape], &[]);
        let attn = linear(&mut g, &merged, &format!("{n}.o"), &b.o_w, Some(&b.o_b), hidden, hidden)?;
        let x1 = g.node("Add", &[&x, &attn], &[]);

        let normed2 = layer_norm(&mut g, &x1, &format!("{n}.ln2"), &b.ln2_w, &b.ln2_b, v.eps)?;
        let h = linear(&mut g, &normed2, &format!("{n}.fc1"), &b.fc1_w, Some(&b.fc1_b), hidden, inter)?;
        // exact GELU, as candle's `gelu_erf`: x · ½ · (1 + erf(x / √2))
        let s = g.node("Mul", &[&h, &inv_sqrt2], &[]);
        let e = g.node("Erf", &[&s], &[]);
        let e1 = g.node("Add", &[&e, &one], &[]);
        let gx = g.node("Mul", &[&h, &e1], &[]);
        let gelu = g.node("Mul", &[&gx, &half], &[]);
        let mlp = linear(&mut g, &gelu, &format!("{n}.fc2"), &b.fc2_w, Some(&b.fc2_b), inter, hidden)?;
        x = g.node("Add", &[&x1, &mlp], &[]);
    }

    let cls_index = g.init_i64("i.cls", &[], &[0])?;
    let pooled = g.node("Gather", &[&x, &cls_index], &[Attr::Int("axis", 1)]);
    let post = layer_norm(&mut g, &pooled, "post_ln", &v.post_ln_w, &v.post_ln_b, v.eps)?;
    let proj = g.init_f32("proj.w", &[hidden, v.embed_dim], &transpose(&v.projection, v.embed_dim, hidden)?)?;
    g.node_named("MatMul", &[&post, &proj], &[], "embedding");
    g.output("embedding", &[Dim::Named("batch"), Dim::Fixed(v.embed_dim as u64)]);
    g.finish("clip_vision", OPSET)
}

/// CLIP's vision tower in an ONNX Runtime session on the graphics card.
pub struct OrtVision {
    session: lcc_onnxrt::Session,
    embed_dim: usize,
}

impl OrtVision {
    /// The tower `v` on the first graphics-card provider that loads it, or why there is none (no ONNX Runtime in
    /// this build or beside the app, no provider that takes the graph).
    pub fn new(v: &VisionTowerWeights) -> Result<OrtVision, String> {
        if !lcc_onnxrt::compiled_in() {
            return Err("ONNX Runtime is not in this build".into());
        }
        // (first so a missing library is said before the export is made)
        lcc_onnxrt::runtime().map_err(|e| e.to_string())?;
        let bytes = vision_tower_onnx(v)?;
        let session = lcc_onnxrt::Session::from_bytes(&bytes, lcc_onnxrt::Want::Gpu).map_err(|e| e.to_string())?;
        Ok(OrtVision { session, embed_dim: v.embed_dim })
    }
}

impl Accelerator for OrtVision {
    fn device(&self) -> String {
        self.session.describe()
    }

    fn run_vision(&self, images: &[f32], batch: usize, h: usize, w: usize) -> Result<Vec<f32>, String> {
        let input = lcc_onnxrt::Tensor::new(vec![batch, 3, h, w], images.to_vec()).map_err(|e| e.to_string())?;
        let out = self.session.run_one(input).map_err(|e| e.to_string())?;
        if out.shape != [batch, self.embed_dim] {
            return Err(format!("ONNX Runtime gave a {:?} embedding, not [{batch}, {}]", out.shape, self.embed_dim));
        }
        Ok(out.data)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::accel::BlockWeights;

    /// A small random tower (2 blocks, 2 heads) shaped like CLIP's.
    pub(crate) fn random_tower() -> VisionTowerWeights {
        let mut seed = 0x9e37_79b9_u32;
        let mut r = |n: usize, s: f32| -> Vec<f32> {
            (0..n)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    ((seed as f32 / u32::MAX as f32) - 0.5) * 2.0 * s
                })
                .collect()
        };
        let (d, inter, p, img, e) = (16, 32, 4, 8, 6);
        let np = 1 + (img / p) * (img / p);
        let block = |r: &mut dyn FnMut(usize, f32) -> Vec<f32>| BlockWeights {
            ln1_w: r(d, 1.0).iter().map(|v| 1.0 + 0.1 * v).collect(),
            ln1_b: r(d, 0.1),
            q_w: r(d * d, 0.3),
            q_b: r(d, 0.1),
            k_w: r(d * d, 0.3),
            k_b: r(d, 0.1),
            v_w: r(d * d, 0.3),
            v_b: r(d, 0.1),
            o_w: r(d * d, 0.3),
            o_b: r(d, 0.1),
            ln2_w: r(d, 1.0).iter().map(|v| 1.0 + 0.1 * v).collect(),
            ln2_b: r(d, 0.1),
            fc1_w: r(inter * d, 0.3),
            fc1_b: r(inter, 0.1),
            fc2_w: r(d * inter, 0.3),
            fc2_b: r(d, 0.1),
        };
        let blocks = vec![block(&mut r), block(&mut r)];
        VisionTowerWeights {
            hidden: d,
            intermediate: inter,
            heads: 2,
            eps: 1e-5,
            embed_dim: e,
            patch_size: p,
            num_positions: np,
            img_w: img,
            img_h: img,
            patch_weight: r(d * 3 * p * p, 0.3),
            class_embedding: r(d, 0.5),
            position_embedding: r(np * d, 0.5),
            pre_ln_w: vec![1.0; d],
            pre_ln_b: vec![0.0; d],
            blocks,
            post_ln_w: vec![1.0; d],
            post_ln_b: vec![0.0; d],
            projection: r(e * d, 0.5),
        }
    }

    #[test]
    fn transposes_and_refuses_a_wrong_size() {
        assert_eq!(transpose(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], 2, 3).unwrap(), vec![1.0, 4.0, 2.0, 5.0, 3.0, 6.0]);
        assert!(transpose(&[1.0; 5], 2, 3).is_err());
    }

    #[test]
    fn the_export_writes_a_model_and_refuses_bad_shapes() {
        let v = random_tower();
        let bytes = vision_tower_onnx(&v).unwrap();
        // every weight ends up in the file
        let weights = v.patch_weight.len() + v.position_embedding.len() + v.blocks.len() * (4 * 16 * 16 + 2 * 16 * 32);
        assert!(bytes.len() > weights * 4);
        let mut bad = random_tower();
        bad.heads = 3;
        assert!(vision_tower_onnx(&bad).is_err());
        let mut bad = random_tower();
        bad.num_positions += 1;
        assert!(vision_tower_onnx(&bad).is_err());
        let mut bad = random_tower();
        bad.blocks[0].q_w.pop();
        assert!(vision_tower_onnx(&bad).is_err());
        let mut bad = random_tower();
        bad.patch_size = 0;
        assert!(vision_tower_onnx(&bad).is_err());
    }
}
