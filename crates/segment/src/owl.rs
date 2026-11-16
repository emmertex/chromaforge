//! OWLv2 (Google, 2023: open-vocabulary object detection) in pure Rust: boxes around what a
//! text names ("car", "wedding dress"), the first half of Describe without SAM 3. The boxes then
//! go to SAM 2.1 as box prompts ([`crate::describe`]), which turns each into a mask.
//!
//! The image path: the photo padded to a square (black, bottom and right) and resized to
//! 960 × 960 ([`preprocess`]), a CLIP ViT-B/16 over the 60 × 60 patches, each patch token
//! multiplied by the class token and normalized; per patch, a box (`box_head` plus a bias that
//! centres it on the patch) and a class embedding (`class_head`). That is computed once per photo
//! ([`Owl::encode`]); a text query then costs the 12-layer CLIP text encoder over 16 tokens and a
//! dot product per patch ([`Owl::detect`]).
//!
//! The weights are `google/owlv2-base-patch16-ensemble` (Apache-2.0) in Hugging Face format, read
//! from the folder they were downloaded to ([`Owl::load`]).
//!
//! Modified work (Apache License 2.0, §4(b)): ported by the project contributors in 2026 from
//! the Python/PyTorch OWLv2 code of Hugging Face Transformers (`models/owlv2/modeling_owlv2.py`,
//! `image_processing_owlv2.py`, `processing_owlv2.py`), Copyright The HuggingFace Team and
//! Google; translated to Rust and restructured. See NOTICE.

use std::path::Path;

use candle_core::{Device, Tensor};
use rayon::prelude::*;

use crate::feat::{Act, Dense, Params, add_assign, layer_norm, sigmoid};
use crate::tokenizer::{EOS, Tokenizer};
use crate::{Error, Result, WEIGHTS_FILE, weights::Weights};

/// Side of the square model input.
pub const OWL_INPUT: usize = 960;
const PATCH: usize = 16;
/// Patches per side (60).
pub const OWL_GRID: usize = OWL_INPUT / PATCH;
const V_HIDDEN: usize = 768;
const V_HEADS: usize = 12;
const V_LAYERS: usize = 12;
const T_HIDDEN: usize = 512;
const T_HEADS: usize = 8;
const T_LAYERS: usize = 12;
/// Tokens of a text query (the tokenizer's `model_max_length`).
const T_CONTEXT: usize = 16;
/// The tokenizer's padding token (`!`).
const T_PAD: u32 = 0;
const VOCAB: usize = 49408;
const EPS: f32 = 1e-5;
/// CLIP's channel means and standard deviations (the OWLv2 processor's).
pub const CLIP_MEAN: [f32; 3] = [0.481_454_66, 0.457_827_5, 0.408_210_73];
pub const CLIP_STD: [f32; 3] = [0.268_629_54, 0.261_302_58, 0.275_777_1];
/// The padding's value (0..1): the processor pads with zeros, before normalizing.
const PAD: f32 = 0.0;
/// Most queries one description is split into.
pub const MAX_QUERIES: usize = 8;

/// The model folder's files.
pub const OWL_FILES: [&str; 4] = [WEIGHTS_FILE, "config.json", "vocab.json", "merges.txt"];

/// Whether `dir` holds an OWLv2 checkpoint (weights, a `config.json` of `model_type` `owlv2`, and
/// the tokenizer's `vocab.json` and `merges.txt`).
pub fn is_owl_dir(dir: &Path) -> bool {
    crate::dino::is_model_dir(dir, "owlv2") && dir.join("vocab.json").is_file() && dir.join("merges.txt").is_file()
}

/// One box the detector found: `rect` = (x0, y0, x1, y1) normalized to the image (0..1),
/// `score` 0..1, `query` the index of the query it matched best.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Detection {
    pub rect: [f32; 4],
    pub score: f32,
    pub query: usize,
}

/// The queries in a description: comma- or semicolon-separated phrases, trimmed, empty ones and
/// repeats dropped, at most [`MAX_QUERIES`] ("car, road" → `["car", "road"]`).
pub fn queries(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for q in text.split([',', ';']) {
        let q = q.split_whitespace().collect::<Vec<_>>().join(" ");
        if !q.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(&q)) && out.len() < MAX_QUERIES {
            out.push(q);
        }
    }
    out
}

/// What the text encoder sees for a query: the phrase in the prompt the OWL models were shown
/// with ("a photo of a car").
fn prompt(query: &str) -> String {
    format!("a photo of a {query}")
}

/// The detections to keep: those scoring at least `threshold`, strongest first, dropping any that
/// overlaps a stronger one by more than `iou`, at most `max`.
pub fn select(mut dets: Vec<Detection>, threshold: f32, iou: f32, max: usize) -> Vec<Detection> {
    dets.retain(|d| d.score.is_finite() && d.score >= threshold && d.rect.iter().all(|v| v.is_finite()) && area(d.rect) > 0.0);
    dets.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Detection> = Vec::new();
    for d in dets {
        if kept.len() >= max {
            break;
        }
        if kept.iter().all(|k| box_iou(k.rect, d.rect) <= iou) {
            kept.push(d);
        }
    }
    kept
}

fn area(r: [f32; 4]) -> f32 {
    (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0)
}

/// Intersection over union of two (x0, y0, x1, y1) boxes.
pub fn box_iou(a: [f32; 4], b: [f32; 4]) -> f32 {
    let inter = area([a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])]);
    let union = area(a) + area(b) - inter;
    if union > 0.0 { inter / union } else { 0.0 }
}

/// The bias that centres each patch's box on it: logits of the patch's corner coordinates
/// (`k / grid`, k = 1..grid) and of the patch size (`compute_box_bias`); `[grid², 4]` as
/// (cx, cy, w, h), row-major over (y, x).
pub fn box_bias(grid: usize) -> Vec<f32> {
    let logit = |v: f32| (v + 1e-4).ln() - (1.0 - v + 1e-4).ln();
    let g = grid.max(1) as f32;
    let size = logit(1.0 / g);
    let mut out = Vec::with_capacity(grid * grid * 4);
    for y in 1..=grid {
        for x in 1..=grid {
            out.extend([logit((x as f32 / g).clamp(0.0, 1.0)), logit((y as f32 / g).clamp(0.0, 1.0)), size, size]);
        }
    }
    out
}

/// A predicted box (cx, cy, w, h over the padded square, 0..1) as (x0, y0, x1, y1) normalized to
/// the `w × h` image that was padded to `side` (bottom and right), clipped to it.
pub fn unpad_box(b: [f32; 4], w: usize, h: usize, side: usize) -> [f32; 4] {
    let (sx, sy) = (side as f32 / w.max(1) as f32, side as f32 / h.max(1) as f32);
    let (hw, hh) = (b[2] / 2.0, b[3] / 2.0);
    let c = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
    [c((b[0] - hw) * sx), c((b[1] - hh) * sy), c((b[0] + hw) * sx), c((b[1] + hh) * sy)]
}

/// Taps of one output sample along an axis of the padded square (`side` samples) resized to
/// `OWL_INPUT`: a Gaussian anti-aliasing filter (σ = (factor − 1) / 2, truncated at 4σ, mirrored
/// at the edges) followed by linear interpolation at the half-pixel-centred position, as the
/// processor's `resize` (`scipy.ndimage.gaussian_filter` + `zoom(order=1, grid_mode=True)`).
fn taps(side: usize, out: usize) -> Vec<Vec<(usize, f32)>> {
    let f = side as f64 / out.max(1) as f64;
    let sigma = ((f - 1.0) / 2.0).max(0.0);
    let radius = if sigma > 0.0 { (4.0 * sigma + 0.5) as isize } else { 0 };
    let mut g: Vec<f64> = (-radius..=radius).map(|k| if sigma > 0.0 { (-0.5 * (k as f64 / sigma).powi(2)).exp() } else { 1.0 }).collect();
    let sum: f64 = g.iter().sum();
    g.iter_mut().for_each(|v| *v /= sum.max(f64::MIN_POSITIVE));
    let n = side as isize;
    // scipy's "mirror": d c b | a b c d | c b a
    let mirror = |i: isize| -> usize {
        if n <= 1 {
            return 0;
        }
        let period = 2 * (n - 1);
        let m = i.rem_euclid(period);
        (if m < n { m } else { period - m }) as usize
    };
    (0..out)
        .map(|o| {
            let c = ((o as f64 + 0.5) * f - 0.5).clamp(0.0, (side.max(1) - 1) as f64);
            let i0 = c.floor() as isize;
            let t = c - i0 as f64;
            let mut ws: Vec<(usize, f32)> = Vec::with_capacity(2 * g.len());
            for (base, wb) in [(i0, 1.0 - t), (i0 + 1, t)] {
                if wb <= 0.0 {
                    continue;
                }
                for (k, gk) in g.iter().enumerate() {
                    ws.push((mirror(base + k as isize - radius), (wb * gk) as f32));
                }
            }
            ws
        })
        .collect()
}

/// An 8-bit RGB image (`w × h`, row-major) as OWLv2's input: scaled to 0..1, padded to a square
/// with black ([`PAD`], bottom and right), resized to 960 × 960 with anti-aliasing, normalized with CLIP's
/// means and deviations, channel-first. Returns the pixels and the padded square's side.
pub fn preprocess(rgb: &[u8], w: usize, h: usize) -> Result<(Vec<f32>, usize)> {
    let n = w.checked_mul(h).and_then(|n| n.checked_mul(3)).ok_or_else(|| Error::Message("image too large".into()))?;
    if w == 0 || h == 0 || rgb.len() < n {
        return Err(Error::Message(format!("image buffer of {} bytes for {w} × {h}", rgb.len())));
    }
    let side = w.max(h);
    let t = taps(side, OWL_INPUT);
    let px = |x: usize, y: usize, c: usize| -> f32 {
        if x < w && y < h { f32::from(rgb.get((y * w + x) * 3 + c).copied().unwrap_or(0)) / 255.0 } else { PAD }
    };
    // horizontal pass over the image's rows (the padding rows below are [`PAD`] throughout)
    let mut tmp = vec![0f32; h * OWL_INPUT * 3];
    tmp.par_chunks_mut(OWL_INPUT * 3).enumerate().for_each(|(y, row)| {
        for (ox, ws) in t.iter().enumerate() {
            for c in 0..3 {
                let v: f32 = ws.iter().map(|(x, wt)| wt * px(*x, y, c)).sum();
                if let Some(d) = row.get_mut(ox * 3 + c) {
                    *d = v;
                }
            }
        }
    });
    let mut out = vec![0f32; 3 * OWL_INPUT * OWL_INPUT];
    let plane = OWL_INPUT * OWL_INPUT;
    let mut planes: Vec<&mut [f32]> = out.chunks_mut(plane).collect();
    planes.par_iter_mut().enumerate().for_each(|(c, p)| {
        let (m, s) = (CLIP_MEAN.get(c).copied().unwrap_or(0.0), CLIP_STD.get(c).copied().unwrap_or(1.0));
        for (oy, ws) in t.iter().enumerate() {
            for ox in 0..OWL_INPUT {
                let v: f32 =
                    ws.iter().map(|(y, wt)| wt * if *y < h { tmp.get((y * OWL_INPUT + ox) * 3 + c).copied().unwrap_or(PAD) } else { PAD }).sum();
                if let Some(d) = p.get_mut(oy * OWL_INPUT + ox) {
                    *d = (v - m) / s;
                }
            }
        }
    });
    Ok((out, side))
}

/// `x · sigmoid(1.702 x)` (CLIP's `quick_gelu`), in place.
fn quick_gelu(x: &mut [f32]) {
    x.par_chunks_mut(4096).for_each(|c| c.iter_mut().for_each(|v| *v *= sigmoid(1.702 * *v)));
}

/// Rows of `c` values scaled to unit length (`+ eps` in the denominator), in place.
fn normalize_rows(x: &mut [f32], c: usize, eps: f32) {
    if c == 0 {
        return;
    }
    x.par_chunks_mut(c).for_each(|r| {
        let n = r.iter().map(|v| v * v).sum::<f32>().sqrt() + eps;
        r.iter_mut().for_each(|v| *v /= n);
    });
}

/// A CLIP encoder layer (`Owlv2EncoderLayer`): x + attn(ln1(x)); x + mlp(ln2(x)), quick GELU.
struct Layer {
    ln1: (Vec<f32>, Vec<f32>),
    /// q (pre-scaled by head_dim^-½), k and v in one product.
    qkv: Dense,
    o: Dense,
    ln2: (Vec<f32>, Vec<f32>),
    fc1: Dense,
    fc2: Dense,
    hidden: usize,
    heads: usize,
}

impl Layer {
    fn load(p: &Params, name: &str, hidden: usize, heads: usize) -> Result<Self> {
        if heads == 0 || !hidden.is_multiple_of(heads) {
            return Err(Error::Model(format!("{hidden} channels do not split into {heads} heads")));
        }
        let scale = ((hidden / heads) as f32).powf(-0.5);
        let (mut w, mut b) = (Vec::with_capacity(3 * hidden * hidden), Vec::with_capacity(3 * hidden));
        for (i, n) in ["q_proj", "k_proj", "v_proj"].iter().enumerate() {
            let s = if i == 0 { scale } else { 1.0 };
            w.extend(p.vec(&format!("{name}.self_attn.{n}.weight"), hidden * hidden)?.into_iter().map(|v| v * s));
            b.extend(p.vec(&format!("{name}.self_attn.{n}.bias"), hidden)?.into_iter().map(|v| v * s));
        }
        let fc1 = p.linear(&format!("{name}.mlp.fc1"))?;
        let fc2 = p.linear(&format!("{name}.mlp.fc2"))?;
        let o = p.linear(&format!("{name}.self_attn.out_proj"))?;
        if (fc1.inp, fc2.out, o.inp, o.out) != (hidden, hidden, hidden, hidden) {
            return Err(Error::Model(format!("{name}: unexpected layer sizes")));
        }
        Ok(Self {
            ln1: p.norm(&format!("{name}.layer_norm1"), hidden)?,
            qkv: Dense::from_out_in(&w, 3 * hidden, hidden, Some(b), &p.dev)?,
            o,
            ln2: p.norm(&format!("{name}.layer_norm2"), hidden)?,
            fc1,
            fc2,
            hidden,
            heads,
        })
    }

    /// `x`: `n × hidden` tokens; `causal`: each token sees only those before it (text).
    fn forward(&self, x: &mut [f32], n: usize, causal: bool) -> Result<()> {
        let mut h = x.to_vec();
        layer_norm(&mut h, &self.ln1.0, &self.ln1.1, EPS);
        let qkv = self.qkv.forward(&h, Act::None)?;
        let a = attention(&qkv, n, self.hidden, self.heads, causal)?;
        add_assign(x, &self.o.forward(&a, Act::None)?);
        let mut h = x.to_vec();
        layer_norm(&mut h, &self.ln2.0, &self.ln2.1, EPS);
        let mut f = self.fc1.forward(&h, Act::None)?;
        quick_gelu(&mut f);
        add_assign(x, &self.fc2.forward(&f, Act::None)?);
        Ok(())
    }
}

/// Softmax attention over `n` tokens from their `[n, 3 · hidden]` projections (q pre-scaled):
/// `[n, hidden]`. The image's 3601 tokens run through the multi-threaded kernel of
/// [`crate::cpu::sdpa`]; the 16 causal text tokens in plain loops.
fn attention(qkv: &[f32], n: usize, hidden: usize, heads: usize, causal: bool) -> Result<Vec<f32>> {
    let d = hidden / heads.max(1);
    if qkv.len() != n * 3 * hidden {
        return Err(Error::Model(format!("attention: {} values for {n} tokens", qkv.len())));
    }
    if !causal {
        let t = Tensor::from_slice(qkv, (1, n, 3 * hidden), &Device::Cpu)?;
        // `[1, heads, n, d]` per part, for the multi-threaded kernel
        let part = |i: usize| {
            t.narrow(2, i * hidden, hidden).and_then(|p| p.reshape((1, n, heads, d))).and_then(|p| p.transpose(1, 2)).and_then(|p| p.contiguous())
        };
        let (q, k, v) = (part(0)?, part(1)?, part(2)?);
        let o = crate::cpu::sdpa(&q, &k, &v, None, 1.0)?;
        return Ok(o.transpose(1, 2)?.contiguous()?.flatten_all()?.to_vec1::<f32>()?);
    }
    let at = |tok: usize, part: usize, head: usize, i: usize| qkv.get(tok * 3 * hidden + part * hidden + head * d + i).copied().unwrap_or(0.0);
    let mut out = vec![0f32; n * hidden];
    for head in 0..heads {
        for qi in 0..n {
            let scores: Vec<f32> = (0..=qi).map(|ki| (0..d).map(|i| at(qi, 0, head, i) * at(ki, 1, head, i)).sum()).collect();
            let m = scores.iter().fold(f32::NEG_INFINITY, |a, b| a.max(*b));
            let e: Vec<f32> = scores.iter().map(|s| (s - m).exp()).collect();
            let z: f32 = e.iter().sum::<f32>().max(f32::MIN_POSITIVE);
            for i in 0..d {
                let v: f32 = e.iter().enumerate().map(|(ki, p)| p * at(ki, 2, head, i)).sum::<f32>() / z;
                if let Some(o) = out.get_mut(qi * hidden + head * d + i) {
                    *o = v;
                }
            }
        }
    }
    Ok(out)
}

/// A loaded OWLv2 model.
pub struct Owl {
    // vision
    patch: Dense,
    class_embedding: Vec<f32>,
    positions: Vec<f32>,
    pre_ln: (Vec<f32>, Vec<f32>),
    vision: Vec<Layer>,
    post_ln: (Vec<f32>, Vec<f32>),
    merge_ln: (Vec<f32>, Vec<f32>),
    // heads
    class_dense: Dense,
    logit_shift: Dense,
    logit_scale: Dense,
    box_head: [Dense; 3],
    box_bias: Vec<f32>,
    // text
    tokens: Vec<f32>,
    text_positions: Vec<f32>,
    text: Vec<Layer>,
    final_ln: (Vec<f32>, Vec<f32>),
    text_projection: Dense,
    tokenizer: Tokenizer,
}

/// A photo run through the image side: per patch, its box over the image and class embedding.
pub struct OwlEncoded {
    /// `[patches, 512]`, unit length.
    class_embeds: Vec<f32>,
    shift: Vec<f32>,
    scale: Vec<f32>,
    /// (x0, y0, x1, y1) normalized to the image.
    boxes: Vec<[f32; 4]>,
}

impl OwlEncoded {
    /// Patches (boxes) per photo.
    pub fn len(&self) -> usize {
        self.boxes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.boxes.is_empty()
    }

    /// Each patch's box: (x0, y0, x1, y1) normalized to the image after [`Owl::encode`]; (cx, cy,
    /// w, h) over the padded square after [`Owl::encode_pixels`] (tests).
    pub fn boxes(&self) -> &[[f32; 4]] {
        &self.boxes
    }

    /// The patches' class embeddings, `[patches, 512]`, unit length (tests).
    pub fn class_embeds(&self) -> &[f32] {
        &self.class_embeds
    }
}

impl Owl {
    /// Load from a directory holding `model.safetensors`, `config.json`, `vocab.json` and
    /// `merges.txt` (`google/owlv2-base-patch16-ensemble` in Hugging Face format).
    pub fn load(dir: &Path) -> Result<Self> {
        Self::load_inner(dir).map_err(|e| match e {
            Error::Model(m) => Error::Message(format!("OWLv2 model: {m}")),
            Error::Candle(c) => Error::Message(format!("OWLv2 model: unexpected weights ({c})")),
            other => other,
        })
    }

    fn load_inner(dir: &Path) -> Result<Self> {
        if !is_owl_dir(dir) {
            return Err(Error::Message(format!("OWLv2 model files not found in {} (expected {})", dir.display(), OWL_FILES.join(", "))));
        }
        let config = crate::dino::read_config(dir).ok_or_else(|| Error::Model("config.json".into()))?;
        let vc = config.get("vision_config");
        let image = vc.and_then(|v| v.get("image_size")).and_then(|v| v.as_u64()).unwrap_or(OWL_INPUT as u64);
        let patch = vc.and_then(|v| v.get("patch_size")).and_then(|v| v.as_u64()).unwrap_or(PATCH as u64);
        if (image, patch) != (OWL_INPUT as u64, PATCH as u64) {
            return Err(Error::Model(format!("only the base model (960 px, 16 px patches) is supported, not {image} px / {patch} px")));
        }
        let weights = Weights::open(&dir.join(WEIGHTS_FILE))?;
        let p = Params { weights: &weights, dev: Device::Cpu };
        let tokenizer = Tokenizer::load(dir)?;
        Self::from_params(&p, tokenizer)
    }

    fn from_params(p: &Params, tokenizer: Tokenizer) -> Result<Self> {
        let v = "owlv2.vision_model";
        let t = "owlv2.text_model";
        let tokens_n = OWL_GRID * OWL_GRID + 1;
        let patch_w = p.vec(&format!("{v}.embeddings.patch_embedding.weight"), V_HIDDEN * 3 * PATCH * PATCH)?;
        let box_head = [p.linear("box_head.dense0")?, p.linear("box_head.dense1")?, p.linear("box_head.dense2")?];
        if box_head[2].out != 4 {
            return Err(Error::Model("box head: 4 outputs expected".into()));
        }
        let class_dense = p.linear("class_head.dense0")?;
        let text_projection = p.linear("owlv2.text_projection")?;
        if class_dense.out != text_projection.out || class_dense.inp != V_HIDDEN || text_projection.inp != T_HIDDEN {
            return Err(Error::Model("class head and text projection disagree".into()));
        }
        Ok(Self {
            patch: Dense::from_out_in(&patch_w, V_HIDDEN, 3 * PATCH * PATCH, None, &p.dev)?,
            class_embedding: p.vec(&format!("{v}.embeddings.class_embedding"), V_HIDDEN)?,
            positions: p.vec(&format!("{v}.embeddings.position_embedding.weight"), tokens_n * V_HIDDEN)?,
            pre_ln: p.norm(&format!("{v}.pre_layernorm"), V_HIDDEN)?,
            vision: (0..V_LAYERS).map(|i| Layer::load(p, &format!("{v}.encoder.layers.{i}"), V_HIDDEN, V_HEADS)).collect::<Result<_>>()?,
            post_ln: p.norm(&format!("{v}.post_layernorm"), V_HIDDEN)?,
            merge_ln: p.norm("layer_norm", V_HIDDEN)?,
            class_dense,
            logit_shift: p.linear("class_head.logit_shift")?,
            logit_scale: p.linear("class_head.logit_scale")?,
            box_head,
            box_bias: box_bias(OWL_GRID),
            tokens: p.vec(&format!("{t}.embeddings.token_embedding.weight"), VOCAB * T_HIDDEN)?,
            text_positions: p.vec(&format!("{t}.embeddings.position_embedding.weight"), T_CONTEXT * T_HIDDEN)?,
            text: (0..T_LAYERS).map(|i| Layer::load(p, &format!("{t}.encoder.layers.{i}"), T_HIDDEN, T_HEADS)).collect::<Result<_>>()?,
            final_ln: p.norm(&format!("{t}.final_layer_norm"), T_HIDDEN)?,
            text_projection,
            tokenizer,
        })
    }

    /// Run the image side on an RGB image (8-bit, row-major, `w × h`).
    pub fn encode(&self, rgb: &[u8], w: usize, h: usize) -> Result<OwlEncoded> {
        let (pixels, side) = preprocess(rgb, w, h)?;
        let mut enc = self.encode_pixels(&pixels)?;
        for b in &mut enc.boxes {
            *b = unpad_box(*b, w, h, side);
        }
        Ok(enc)
    }

    /// The image side on preprocessed pixels (`[3, 960, 960]`, [`preprocess`]); boxes as
    /// (cx, cy, w, h) over the padded square (tests).
    pub fn encode_pixels(&self, pixels: &[f32]) -> Result<OwlEncoded> {
        let mut prof = crate::feat::Profile::new("owlv2");
        let np = OWL_GRID * OWL_GRID;
        let k = 3 * PATCH * PATCH;
        let plane = OWL_INPUT * OWL_INPUT;
        if pixels.len() != 3 * plane {
            return Err(Error::Model(format!("{} input values, expected {}", pixels.len(), 3 * plane)));
        }
        // the patch convolution (16 × 16, stride 16) as one product over unfolded patches
        let mut patches = vec![0f32; np * k];
        patches.par_chunks_mut(k).enumerate().for_each(|(pi, row)| {
            let (py, px) = (pi / OWL_GRID, pi % OWL_GRID);
            for c in 0..3 {
                for ky in 0..PATCH {
                    let src = c * plane + (py * PATCH + ky) * OWL_INPUT + px * PATCH;
                    let at = c * PATCH * PATCH + ky * PATCH;
                    if let (Some(d), Some(s)) = (row.get_mut(at..at + PATCH), pixels.get(src..src + PATCH)) {
                        d.copy_from_slice(s);
                    }
                }
            }
        });
        let embedded = self.patch.forward(&patches, Act::None)?;
        let n = np + 1;
        let mut x = Vec::with_capacity(n * V_HIDDEN);
        x.extend_from_slice(&self.class_embedding);
        x.extend_from_slice(&embedded);
        add_assign(&mut x, &self.positions);
        layer_norm(&mut x, &self.pre_ln.0, &self.pre_ln.1, EPS);
        prof.step("embed");
        for l in &self.vision {
            l.forward(&mut x, n, false)?;
        }
        prof.step("vision transformer");
        layer_norm(&mut x, &self.post_ln.0, &self.post_ln.1, EPS);
        // every patch token times the class token, normalized
        let (cls, rest) = x.split_at_mut(V_HIDDEN);
        rest.par_chunks_mut(V_HIDDEN).for_each(|r| r.iter_mut().zip(cls.iter()).for_each(|(v, c)| *v *= c));
        let mut feats = rest.to_vec();
        layer_norm(&mut feats, &self.merge_ln.0, &self.merge_ln.1, EPS);
        // heads
        let mut class_embeds = self.class_dense.forward(&feats, Act::None)?;
        normalize_rows(&mut class_embeds, self.class_dense.out, 1e-6);
        let shift = self.logit_shift.forward(&feats, Act::None)?;
        let scale: Vec<f32> = self.logit_scale.forward(&feats, Act::None)?.into_iter().map(|v| if v > 0.0 { v } else { v.exp_m1() } + 1.0).collect();
        let [b0, b1, b2] = &self.box_head;
        let raw = b2.forward(&b1.forward(&b0.forward(&feats, Act::Gelu)?, Act::Gelu)?, Act::None)?;
        let boxes: Vec<[f32; 4]> =
            raw.as_chunks::<4>().0.iter().zip(self.box_bias.as_chunks::<4>().0).map(|(r, b)| std::array::from_fn(|i| sigmoid(r[i] + b[i]))).collect();
        prof.step("heads");
        if boxes.len() != np || shift.len() != np || scale.len() != np {
            return Err(Error::Model("heads: unexpected output sizes".into()));
        }
        Ok(OwlEncoded { class_embeds, shift, scale, boxes })
    }

    /// The query embedding of `text` (unit length, `[512]`).
    pub fn embed_text(&self, text: &str) -> Result<Vec<f32>> {
        let (ids, valid) = self.tokenizer.encode_padded(text, T_CONTEXT, T_PAD);
        let mut x = Vec::with_capacity(T_CONTEXT * T_HIDDEN);
        for id in &ids {
            let at = (*id as usize).min(VOCAB - 1) * T_HIDDEN;
            x.extend_from_slice(self.tokens.get(at..at + T_HIDDEN).ok_or_else(|| Error::Model("token embedding".into()))?);
        }
        add_assign(&mut x, &self.text_positions);
        for l in &self.text {
            l.forward(&mut x, T_CONTEXT, true)?;
        }
        layer_norm(&mut x, &self.final_ln.0, &self.final_ln.1, EPS);
        // pooled at the end token (the first one: padding comes after it)
        let eos = ids.iter().zip(&valid).position(|(id, v)| *v && *id == EOS).unwrap_or(0);
        let pooled = x.get(eos * T_HIDDEN..(eos + 1) * T_HIDDEN).ok_or_else(|| Error::Model("pooled text token".into()))?;
        let mut e = self.text_projection.forward(pooled, Act::None)?;
        normalize_rows(&mut e, self.text_projection.out, 1e-6);
        Ok(e)
    }

    /// Every patch's box with its best score over `queries` (phrases, see [`queries`]), unfiltered.
    pub fn detect(&self, enc: &OwlEncoded, queries: &[String]) -> Result<Vec<Detection>> {
        let embeds = queries.iter().map(|q| self.embed_text(&prompt(q))).collect::<Result<Vec<_>>>()?;
        Ok(score(enc, &embeds))
    }
}

/// Each patch's best query: `sigmoid((⟨patch, query⟩ + shift) · scale)` (`Owlv2ClassPredictionHead`).
fn score(enc: &OwlEncoded, embeds: &[Vec<f32>]) -> Vec<Detection> {
    let dim = embeds.first().map_or(0, Vec::len);
    if dim == 0 {
        return Vec::new();
    }
    enc.boxes
        .par_iter()
        .enumerate()
        .filter_map(|(i, rect)| {
            let e = enc.class_embeds.get(i * dim..(i + 1) * dim)?;
            let (shift, scale) = (enc.shift.get(i).copied()?, enc.scale.get(i).copied()?);
            let (query, logit) = embeds
                .iter()
                .enumerate()
                .map(|(q, t)| (q, (e.iter().zip(t).map(|(a, b)| a * b).sum::<f32>() + shift) * scale))
                .max_by(|a, b| a.1.total_cmp(&b.1))?;
            Some(Detection { rect: *rect, score: sigmoid(logit), query })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_split_on_commas() {
        assert_eq!(queries(" car,  road ;; Car , the  red dress"), ["car", "road", "the red dress"]);
        assert!(queries(" , ;").is_empty());
        assert_eq!(queries(&"a,".repeat(3)).len(), 1);
        assert_eq!(queries(&(0..20).map(|i| format!("thing {i}")).collect::<Vec<_>>().join(",")).len(), MAX_QUERIES);
        assert_eq!(prompt("car"), "a photo of a car");
    }

    #[test]
    fn box_bias_centres_boxes_on_their_patch() {
        let g = 4;
        let b = box_bias(g);
        assert_eq!(b.len(), g * g * 4);
        // patch (row 1, column 2): corner coordinates 3/4, 2/4; size 1/4
        let at = (g + 2) * 4;
        let s = |v: f32| sigmoid(v);
        assert!((s(b[at]) - 0.75).abs() < 1e-3 && (s(b[at + 1]) - 0.5).abs() < 1e-3, "{:?}", &b[at..at + 4]);
        assert!((s(b[at + 2]) - 0.25).abs() < 1e-3 && (s(b[at + 3]) - 0.25).abs() < 1e-3);
        // the last patch's corner is the image corner (clipped, finite)
        assert!(b.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn boxes_come_back_to_the_unpadded_image() {
        // a 200 × 100 image padded to 200 × 200: the box over the top half's middle
        let r = unpad_box([0.5, 0.25, 0.5, 0.5], 200, 100, 200);
        assert_eq!(r, [0.25, 0.0, 0.75, 1.0]);
        // boxes reaching into the padding are clipped; garbage is harmless
        assert_eq!(unpad_box([0.5, 0.75, 1.0, 0.5], 200, 100, 200)[1..], [1.0, 1.0, 1.0]);
        assert_eq!(unpad_box([f32::NAN, 0.5, 0.2, 0.2], 0, 0, 0)[0], 0.0);
    }

    #[test]
    fn selection_thresholds_and_drops_overlaps() {
        let d = |x: f32, score: f32| Detection { rect: [x, 0.0, x + 0.2, 0.2], score, query: 0 };
        let dets = vec![
            d(0.0, 0.3),
            d(0.01, 0.5),
            d(0.5, 0.2),
            d(0.8, 0.05),
            Detection { rect: [0.1, 0.1, 0.1, 0.5], score: 0.9, query: 1 },
            d(f32::NAN, 0.9),
        ];
        let kept = select(dets.clone(), 0.1, 0.5, 10);
        // the 0.5 box wins over its near-duplicate; the empty, NaN and weak boxes go
        assert_eq!(kept.iter().map(|k| k.score).collect::<Vec<_>>(), [0.5, 0.2]);
        assert_eq!(select(dets, 0.1, 0.5, 1).len(), 1);
        assert!((box_iou([0.0, 0.0, 1.0, 1.0], [0.0, 0.0, 0.5, 1.0]) - 0.5).abs() < 1e-6);
        assert_eq!(box_iou([0.0; 4], [0.0; 4]), 0.0);
    }

    #[test]
    fn preprocessing_pads_and_normalizes() {
        // a white 64 × 32 image: the top half white, the bottom half the padding
        let rgb = vec![255u8; 64 * 32 * 3];
        let (px, side) = preprocess(&rgb, 64, 32).unwrap();
        assert_eq!((px.len(), side), (3 * OWL_INPUT * OWL_INPUT, 64));
        let at = |c: usize, y: usize, x: usize| px[c * OWL_INPUT * OWL_INPUT + y * OWL_INPUT + x] * CLIP_STD[c] + CLIP_MEAN[c];
        assert!((at(0, 100, 500) - 1.0).abs() < 1e-4, "{}", at(0, 100, 500));
        assert!((at(2, 900, 10) - PAD).abs() < 1e-4);
        // the seam blends the two
        let seam = at(1, OWL_INPUT / 2, 300);
        assert!(seam > PAD && seam < 1.0, "{seam}");
        // a large image is filtered before resizing: a fine checkerboard averages to grey
        let (w, h) = (1920, 1920);
        let checker: Vec<u8> = (0..w * h).flat_map(|i| if (i % w + i / w) % 2 == 0 { [255u8; 3] } else { [0u8; 3] }).collect();
        let (px, _) = preprocess(&checker, w, h).unwrap();
        let v = px[OWL_INPUT * 480 + 480] * CLIP_STD[0] + CLIP_MEAN[0];
        assert!((v - 0.5).abs() < 0.02, "{v}");
        // bad buffers are errors
        assert!(preprocess(&rgb, 64, 33).is_err());
        assert!(preprocess(&[], 0, 0).is_err());
    }

    #[test]
    fn causal_attention_sees_only_earlier_tokens() {
        // 3 tokens, 1 head of 2: values (token index) — the first token can only see itself
        let (n, hidden) = (3, 2);
        let mut qkv = vec![0f32; n * 3 * hidden];
        for t in 0..n {
            qkv[t * 6 + 4] = t as f32;
            qkv[t * 6 + 5] = 1.0;
        }
        let out = attention(&qkv, n, hidden, 1, true).unwrap();
        assert_eq!(&out[..2], &[0.0, 1.0]);
        // equal scores: the mean of the tokens seen so far
        assert!((out[2] - 0.5).abs() < 1e-6 && (out[4] - 1.0).abs() < 1e-6);
        let full = attention(&qkv, n, hidden, 1, false).unwrap();
        assert!(full.chunks(2).all(|r| (r[0] - 1.0).abs() < 1e-5));
        assert!(attention(&qkv[1..], n, hidden, 1, true).is_err());
    }

    #[test]
    fn scores_pick_each_patchs_best_query() {
        let enc = OwlEncoded {
            class_embeds: vec![1.0, 0.0, 0.0, 1.0],
            shift: vec![0.0, -1.0],
            scale: vec![2.0, 1.0],
            boxes: vec![[0.0, 0.0, 0.5, 0.5], [0.5, 0.5, 1.0, 1.0]],
        };
        let dets = score(&enc, &[vec![1.0, 0.0], vec![0.0, 1.0]]);
        assert_eq!(dets.iter().map(|d| d.query).collect::<Vec<_>>(), [0, 1]);
        assert!((dets[0].score - sigmoid(2.0)).abs() < 1e-6 && (dets[1].score - 0.5).abs() < 1e-6);
        assert!(score(&enc, &[]).is_empty());
    }

    #[test]
    fn missing_or_wrong_folders_are_errors() {
        let d = std::env::temp_dir().join(format!("lc-owl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert!(!is_owl_dir(&d));
        let e = Owl::load(&d).err().unwrap().to_string();
        assert!(e.contains("OWLv2 model files not found"), "{e}");
        for f in OWL_FILES {
            std::fs::write(d.join(f), b"{}").unwrap();
        }
        std::fs::write(d.join("config.json"), br#"{"model_type": "owlv2", "vision_config": {"image_size": 1008, "patch_size": 14}}"#).unwrap();
        assert!(is_owl_dir(&d));
        let e = Owl::load(&d).err().unwrap().to_string();
        assert!(e.starts_with("OWLv2 model:"), "{e}");
        let _ = std::fs::remove_dir_all(&d);
    }
}
