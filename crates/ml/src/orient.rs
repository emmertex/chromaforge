//! Orientation detection (issue #490): which way up a photo is, from CLIP image embeddings.
//!
//! A photo is shown to the encoder four times, turned 0°, 90°, 180° and 270° clockwise ([`views`]). A small linear
//! classifier on CLIP's embedding ([`Head`], `4 × embed_dim` weights) says, for each view, how far clockwise the
//! upright scene has been turned in it. The four answers must agree with each other (if the photo is the upright
//! scene turned by `r`, its view turned by `k` shows `r + k`), so they are pooled ([`decide`]) into one answer: the
//! quarter turns that put the photo upright, with a confidence. Only answers at or above the head's confidence
//! threshold are applied, so a flat-lay, a texture or a sky is left alone.
//!
//! The head is fitted ([`fit`]) on photos known to be upright (a library whose photos stand the right way up),
//! each turned four ways, by multinomial logistic regression with a held-out set of photos to measure it on and to
//! choose the threshold. Fitting is plain Rust: no training framework, no photos kept, only the numbers.

use serde::{Deserialize, Serialize};

use crate::encoder::RgbImage;

/// The four answers: the upright scene turned 0, 1, 2 or 3 quarter turns clockwise.
pub const CLASSES: usize = 4;
/// The head file's format version.
pub const HEAD_VERSION: u32 = 1;
/// The largest head file read (bytes).
pub const MAX_HEAD_BYTES: u64 = 4 * 1024 * 1024;
/// The largest embedding a head may take.
pub const MAX_EMBED_DIM: usize = 4096;
/// The threshold used when a head names none.
pub const DEFAULT_MIN_CONFIDENCE: f32 = 0.9;

/// How a fit went, measured on photos held out of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FitReport {
    /// Photos the head was fitted on (each seen four ways).
    pub photos: usize,
    /// Photos held out to measure it.
    pub holdout_photos: usize,
    /// One view on its own: the share of held-out views classed right.
    pub view_accuracy: f32,
    /// All four views pooled ([`decide`]): the share of held-out photos (in each of their four turns) classed right.
    pub photo_accuracy: f32,
    /// For each threshold: the share of held-out decisions at or above it, and how many of those were right.
    pub thresholds: Vec<ThresholdRow>,
}

/// One row of [`FitReport::thresholds`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ThresholdRow {
    pub min_confidence: f32,
    /// Share of held-out decisions this confident.
    pub coverage: f32,
    /// Share of those that were right.
    pub accuracy: f32,
}

/// A fitted orientation classifier for one embedding model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Head {
    pub version: u32,
    /// The embedding model it was fitted on, by its cache tag (`<id>@<sha256-prefix>`): a head only fits the exact
    /// weights it was fitted with.
    pub embedder: String,
    pub embed_dim: u32,
    /// `CLASSES` rows of `embed_dim` weights.
    pub weights: Vec<Vec<f32>>,
    pub bias: Vec<f32>,
    /// Answers below this are not applied.
    #[serde(default = "default_min_confidence")]
    pub min_confidence: f32,
    /// What it was fitted on (shown in the status, recorded in `NOTICE` for the built-in head).
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub report: Option<FitReport>,
}

fn default_min_confidence() -> f32 {
    DEFAULT_MIN_CONFIDENCE
}

impl Head {
    /// A head from its JSON file, checked: version, shape, finite numbers, a threshold in `[0, 1]`.
    pub fn from_json(bytes: &[u8]) -> Result<Head, String> {
        if bytes.len() as u64 > MAX_HEAD_BYTES {
            return Err("the orientation head file is too large".into());
        }
        let h: Head = serde_json::from_slice(bytes).map_err(|e| format!("not an orientation head: {e}"))?;
        h.validate()?;
        Ok(h)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.version != HEAD_VERSION {
            return Err(format!("orientation head version {} (this build reads {HEAD_VERSION})", self.version));
        }
        let dim = self.embed_dim as usize;
        if dim == 0 || dim > MAX_EMBED_DIM {
            return Err(format!("an orientation head for {dim}-value embeddings"));
        }
        if self.weights.len() != CLASSES || self.bias.len() != CLASSES || self.weights.iter().any(|r| r.len() != dim) {
            return Err("the orientation head's weights don't have its shape".into());
        }
        if self.weights.iter().flatten().chain(&self.bias).any(|v| !v.is_finite()) {
            return Err("the orientation head has a number that isn't finite".into());
        }
        if !(0.0..=1.0).contains(&self.min_confidence) {
            return Err(format!("the orientation head's threshold {} is not between 0 and 1", self.min_confidence));
        }
        Ok(())
    }

    pub fn to_json(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec_pretty(self).map_err(|e| e.to_string())
    }

    /// The class scores for one embedding (`None` when it has the wrong length or isn't finite).
    pub fn logits(&self, e: &[f32]) -> Option<[f32; CLASSES]> {
        if e.len() != self.embed_dim as usize || e.iter().any(|v| !v.is_finite()) {
            return None;
        }
        let mut out = [0.0f32; CLASSES];
        for (c, slot) in out.iter_mut().enumerate() {
            let row = self.weights.get(c)?;
            *slot = self.bias.get(c)? + row.iter().zip(e).map(|(w, x)| w * x).sum::<f32>();
        }
        Some(out)
    }
}

/// Heads built in, by the tag of the embedding model they were fitted for (the JSON files beside this module;
/// what they were fitted on is in `NOTICE`). Empty until the CLIP ViT-B/32 head is fitted.
const BUILTIN: &[(&str, &str)] = &[];

/// The built-in head for the embedding model tagged `tag`, if there is one.
pub fn builtin(tag: &str) -> Option<Head> {
    let (_, json) = BUILTIN.iter().find(|(t, _)| *t == tag)?;
    match Head::from_json(json.as_bytes()) {
        Ok(h) if h.embedder == tag => Some(h),
        Ok(h) => {
            log::warn!("ml: the built-in orientation head for {tag} names {}", h.embedder);
            None
        }
        Err(e) => {
            log::warn!("ml: the built-in orientation head for {tag} is not used: {e}");
            None
        }
    }
}

/// `img` turned `quarter_turns` × 90° clockwise.
pub fn rotate(img: &RgbImage, quarter_turns: u8) -> RgbImage {
    let (w, h) = (img.width as usize, img.height as usize);
    let turns = quarter_turns % 4;
    if turns == 0 {
        return img.clone();
    }
    let (nw, nh) = if turns == 2 { (w, h) } else { (h, w) };
    let mut rgb = vec![0u8; img.rgb.len()];
    for y in 0..nh {
        for x in 0..nw {
            let (sx, sy) = match turns {
                1 => (y, h - 1 - x),
                2 => (w - 1 - x, h - 1 - y),
                _ => (w - 1 - y, x),
            };
            let (s, d) = ((sy * w + sx) * 3, (y * nw + x) * 3);
            if let (Some(src), Some(dst)) = (img.rgb.get(s..s + 3), rgb.get_mut(d..d + 3)) {
                dst.copy_from_slice(src);
            }
        }
    }
    RgbImage { width: nw as u32, height: nh as u32, rgb }
}

/// The photo turned 0, 1, 2 and 3 quarter turns clockwise, in that order: what [`decide`] takes the embeddings of.
pub fn views(img: &RgbImage) -> [RgbImage; CLASSES] {
    [img.clone(), rotate(img, 1), rotate(img, 2), rotate(img, 3)]
}

fn log_softmax(z: &[f32; CLASSES]) -> [f32; CLASSES] {
    let m = z.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));
    let lse = m + z.iter().map(|v| (v - m).exp()).sum::<f32>().ln();
    let mut out = [0.0; CLASSES];
    for (o, v) in out.iter_mut().zip(z) {
        *o = v - lse;
    }
    out
}

fn softmax(z: &[f32; CLASSES]) -> [f32; CLASSES] {
    let l = log_softmax(z);
    l.map(f32::exp)
}

/// One photo's answer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Verdict {
    /// How far clockwise the scene is turned in the photo as it is shown now, in quarter turns (0: upright).
    pub turned: u8,
    /// The quarter turns clockwise that put it upright (`(4 - turned) % 4`).
    pub correction: u8,
    /// How sure the pooled answer is (0.25 … 1).
    pub confidence: f32,
    /// The pooled probability of each `turned` value.
    pub probs: [f32; CLASSES],
}

impl Verdict {
    /// The correction in degrees clockwise (0, 90, 180 or 270).
    pub fn degrees(&self) -> u16 {
        u16::from(self.correction) * 90
    }
}

/// Pools the head's answers for the four [`views`] of one photo (their embeddings, in order) into one.
pub fn decide(head: &Head, embeddings: &[Vec<f32>]) -> Result<Verdict, String> {
    if embeddings.len() != CLASSES {
        return Err(format!("{} views of the photo, not {CLASSES}", embeddings.len()));
    }
    let mut score = [0.0f32; CLASSES];
    for (k, e) in embeddings.iter().enumerate() {
        let lp = log_softmax(&head.logits(e).ok_or("an embedding the orientation head can't take")?);
        // the view turned k more shows the scene turned (r + k): its vote for r is its score for that class
        for (r, s) in score.iter_mut().enumerate() {
            *s += lp.get((r + k) % CLASSES).copied().unwrap_or(f32::NEG_INFINITY) / CLASSES as f32;
        }
    }
    let probs = softmax(&score);
    let (turned, confidence) = probs.iter().enumerate().fold((0usize, f32::NEG_INFINITY), |b, (i, &p)| if p > b.1 { (i, p) } else { b });
    if !confidence.is_finite() {
        return Err("the orientation head gave no answer".into());
    }
    let turned = turned as u8;
    Ok(Verdict { turned, correction: (4 - turned) % 4, confidence, probs })
}

/// One upright photo's four [`views`]' embeddings, for [`fit`]. `group` keeps a photo's views on the same side of
/// the held-out split.
#[derive(Debug, Clone)]
pub struct Example {
    pub group: u64,
    pub views: Vec<Vec<f32>>,
}

/// How [`fit`] runs.
#[derive(Debug, Clone, Copy)]
pub struct FitOptions {
    /// Gradient steps.
    pub steps: usize,
    /// L2 penalty on the weights.
    pub l2: f32,
    /// One photo in this many is held out (by `group`) to measure the head.
    pub holdout_every: u64,
    /// The share of held-out decisions at or above the chosen threshold that must be right.
    pub target_accuracy: f32,
}

impl Default for FitOptions {
    fn default() -> Self {
        FitOptions { steps: 600, l2: 1e-4, holdout_every: 5, target_accuracy: 0.98 }
    }
}

/// Softmax regression by full-batch gradient descent with Nesterov momentum. The inputs are unit-norm embeddings, so
/// the loss's curvature is at most ½ and a fixed step is stable.
fn train(rows: &[(&[f32], usize)], dim: usize, opts: &FitOptions) -> (Vec<Vec<f32>>, Vec<f32>) {
    let (lr, mu) = (2.0f32, 0.9f32);
    let mut w = vec![vec![0.0f32; dim]; CLASSES];
    let mut b = vec![0.0f32; CLASSES];
    let mut vw = vec![vec![0.0f32; dim]; CLASSES];
    let mut vb = vec![0.0f32; CLASSES];
    let n = rows.len().max(1) as f32;
    for _ in 0..opts.steps {
        // look-ahead point
        let lw: Vec<Vec<f32>> = w.iter().zip(&vw).map(|(wr, vr)| wr.iter().zip(vr).map(|(a, v)| a + mu * v).collect()).collect();
        let lb: Vec<f32> = b.iter().zip(&vb).map(|(a, v)| a + mu * v).collect();
        let mut gw = vec![vec![0.0f32; dim]; CLASSES];
        let mut gb = [0.0f32; CLASSES];
        for (x, y) in rows {
            let mut z = [0.0f32; CLASSES];
            for (c, zc) in z.iter_mut().enumerate() {
                *zc = lb.get(c).copied().unwrap_or(0.0) + lw.get(c).map_or(0.0, |r| r.iter().zip(*x).map(|(a, v)| a * v).sum::<f32>());
            }
            let p = softmax(&z);
            for c in 0..CLASSES {
                let d = p.get(c).copied().unwrap_or(0.0) - if c == *y { 1.0 } else { 0.0 };
                if let Some(g) = gb.get_mut(c) {
                    *g += d;
                }
                if let Some(gr) = gw.get_mut(c) {
                    for (g, v) in gr.iter_mut().zip(*x) {
                        *g += d * v;
                    }
                }
            }
        }
        for c in 0..CLASSES {
            if let (Some(wr), Some(vr), Some(gr)) = (w.get_mut(c), vw.get_mut(c), gw.get(c)) {
                for ((a, v), g) in wr.iter_mut().zip(vr.iter_mut()).zip(gr) {
                    *v = mu * *v - lr * (g / n + opts.l2 * (*a + mu * *v));
                    *a += *v;
                }
            }
            if let (Some(a), Some(v), Some(g)) = (b.get_mut(c), vb.get_mut(c), gb.get(c)) {
                *v = mu * *v - lr * (g / n);
                *a += *v;
            }
        }
    }
    (w, b)
}

fn rows_of<'a>(examples: &[&'a Example]) -> Vec<(&'a [f32], usize)> {
    examples.iter().flat_map(|e| e.views.iter().enumerate().map(|(k, v)| (v.as_slice(), k))).collect()
}

/// Fits a head on upright photos (`examples`, each with its four views' embeddings) for the embedding model
/// `embedder` (its tag). Measures it on the held-out photos, chooses its threshold there, then fits again on all.
pub fn fit(examples: &[Example], embedder: &str, dim: usize, source: &str, opts: &FitOptions) -> Result<Head, String> {
    if dim == 0 || dim > MAX_EMBED_DIM {
        return Err(format!("{dim}-value embeddings"));
    }
    let usable: Vec<&Example> =
        examples.iter().filter(|e| e.views.len() == CLASSES && e.views.iter().all(|v| v.len() == dim && v.iter().all(|x| x.is_finite()))).collect();
    let every = opts.holdout_every.max(2);
    let (held, kept): (Vec<&Example>, Vec<&Example>) = usable.iter().partition(|e| mix(e.group).is_multiple_of(every));
    if kept.len() < 8 || held.len() < 2 {
        return Err(format!("{} usable photos are too few to fit and measure an orientation head", usable.len()));
    }
    let (w, b) = train(&rows_of(&kept), dim, opts);
    let mut head = Head {
        version: HEAD_VERSION,
        embedder: embedder.to_string(),
        embed_dim: dim as u32,
        weights: w,
        bias: b,
        min_confidence: DEFAULT_MIN_CONFIDENCE,
        source: source.to_string(),
        report: None,
    };

    // measured on the held-out photos: each view alone, and each photo in each of its four turns, pooled
    let (mut views_right, mut views) = (0usize, 0usize);
    let mut decisions: Vec<(f32, bool)> = Vec::new();
    for e in &held {
        for (k, v) in e.views.iter().enumerate() {
            if let Some(z) = head.logits(v) {
                views += 1;
                let best = z.iter().enumerate().fold((0usize, f32::NEG_INFINITY), |a, (i, &s)| if s > a.1 { (i, s) } else { a }).0;
                views_right += usize::from(best == k);
            }
        }
        for r in 0..CLASSES {
            // the photo as if it had been imported turned r: its views are the upright views starting at r
            let shown: Vec<Vec<f32>> = (0..CLASSES).filter_map(|k| e.views.get((r + k) % CLASSES).cloned()).collect();
            if let Ok(v) = decide(&head, &shown) {
                decisions.push((v.confidence, usize::from(v.turned) == r));
            }
        }
    }
    let total = decisions.len().max(1) as f32;
    let photo_accuracy = decisions.iter().filter(|d| d.1).count() as f32 / total;
    let mut thresholds = Vec::new();
    let mut chosen = None;
    for step in 0..=19 {
        let t = 0.5 + step as f32 * 0.025;
        let over: Vec<&(f32, bool)> = decisions.iter().filter(|d| d.0 >= t).collect();
        let coverage = over.len() as f32 / total;
        let accuracy = if over.is_empty() { 1.0 } else { over.iter().filter(|d| d.1).count() as f32 / over.len() as f32 };
        if chosen.is_none() && accuracy >= opts.target_accuracy && !over.is_empty() {
            chosen = Some(t);
        }
        thresholds.push(ThresholdRow { min_confidence: t, coverage, accuracy });
    }
    let report = FitReport {
        photos: kept.len(),
        holdout_photos: held.len(),
        view_accuracy: views_right as f32 / views.max(1) as f32,
        photo_accuracy,
        thresholds,
    };

    // the numbers that ship: fitted on every photo, with the threshold the held-out photos chose
    let all: Vec<&Example> = usable.clone();
    let (w, b) = train(&rows_of(&all), dim, opts);
    head.weights = w;
    head.bias = b;
    head.min_confidence = chosen.unwrap_or(0.975);
    head.report = Some(FitReport { photos: all.len(), ..report });
    head.validate()?;
    Ok(head)
}

/// A well-mixed hash of `x` (splitmix64), so the held-out split doesn't follow photo ids' order.
fn mix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(w: u32, h: u32) -> RgbImage {
        let rgb = (0..w * h).flat_map(|i| [(i % 251) as u8, (i / w) as u8, (i % w) as u8]).collect();
        RgbImage::new(w, h, rgb).unwrap()
    }

    fn px(i: &RgbImage, x: u32, y: u32) -> [u8; 3] {
        let o = ((y * i.width + x) * 3) as usize;
        [i.rgb[o], i.rgb[o + 1], i.rgb[o + 2]]
    }

    #[test]
    fn built_in_heads_load_for_their_model() {
        for (tag, _) in BUILTIN {
            let h = builtin(tag).unwrap();
            assert_eq!(&h.embedder, tag);
        }
        assert!(builtin("no-such-model@0").is_none());
    }

    #[test]
    fn rotation_turns_clockwise_and_four_turns_are_the_identity() {
        let a = img(5, 3);
        let r = rotate(&a, 1);
        assert_eq!((r.width, r.height), (3, 5));
        // the top-left corner goes to the top-right
        assert_eq!(px(&r, 2, 0), px(&a, 0, 0));
        // the top-right corner goes to the bottom-right
        assert_eq!(px(&r, 2, 4), px(&a, 4, 0));
        assert_eq!(rotate(&a, 2), rotate(&r, 1));
        assert_eq!(rotate(&a, 3), rotate(&rotate(&a, 2), 1));
        assert_eq!(rotate(&rotate(&a, 3), 1), a);
        assert_eq!(rotate(&a, 4), a);
        let v = views(&a);
        assert_eq!(v[1], r);
    }

    /// Embeddings where axis `c` says "turned c": a perfect toy encoder.
    fn toy_view(turned: usize, dim: usize, noise: f32, seed: u64) -> Vec<f32> {
        let mut v: Vec<f32> = (0..dim).map(|i| ((mix(seed ^ (i as u64) << 8) % 1000) as f32 / 1000.0 - 0.5) * noise).collect();
        v[turned] += 1.0;
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter().map(|x| x / n).collect()
    }

    fn toy_examples(n: u64, dim: usize, noise: f32) -> Vec<Example> {
        (0..n).map(|g| Example { group: g, views: (0..CLASSES).map(|k| toy_view(k, dim, noise, g * 7 + k as u64)).collect() }).collect()
    }

    #[test]
    fn fitting_learns_a_separable_toy_and_decides_every_turn() {
        let ex = toy_examples(60, 12, 0.6);
        let head = fit(&ex, "toy@1", 12, "toy", &FitOptions { steps: 300, ..FitOptions::default() }).unwrap();
        let report = head.report.clone().unwrap();
        assert!(report.photo_accuracy > 0.95, "{report:?}");
        assert!(report.holdout_photos >= 2);
        for r in 0..CLASSES {
            let shown: Vec<Vec<f32>> = (0..CLASSES).map(|k| toy_view((r + k) % CLASSES, 12, 0.3, 999 + k as u64)).collect();
            let v = decide(&head, &shown).unwrap();
            assert_eq!(usize::from(v.turned), r);
            assert_eq!((usize::from(v.turned) + usize::from(v.correction)) % 4, 0);
            assert!(v.confidence > head.min_confidence.min(0.9), "{v:?}");
        }
        // and it goes through its file
        let back = Head::from_json(&head.to_json().unwrap()).unwrap();
        assert_eq!(back, head);
    }

    #[test]
    fn an_undecidable_photo_has_low_confidence() {
        let ex = toy_examples(60, 12, 0.6);
        let head = fit(&ex, "toy@1", 12, "toy", &FitOptions { steps: 300, ..FitOptions::default() }).unwrap();
        // every view looks the same (a texture): no turn is preferred
        let mut flat = vec![0.0f32; 12];
        flat[11] = 1.0;
        let v = decide(&head, &[flat.clone(), flat.clone(), flat.clone(), flat]).unwrap();
        assert!(v.confidence < 0.5, "{v:?}");
    }

    #[test]
    fn hostile_heads_and_inputs_are_errors() {
        let ex = toy_examples(40, 8, 0.5);
        let head = fit(&ex, "toy@1", 8, "toy", &FitOptions { steps: 50, ..FitOptions::default() }).unwrap();
        let good = head.to_json().unwrap();
        assert!(Head::from_json(b"").is_err());
        assert!(Head::from_json(b"{\"version\":1}").is_err());
        assert!(Head::from_json(&vec![b' '; MAX_HEAD_BYTES as usize + 1]).is_err());
        for (find, replace) in [("\"version\": 1", "\"version\": 7"), ("\"embedDim\": 8", "\"embedDim\": 9"), ("\"embedDim\": 8", "\"embedDim\": 0")]
        {
            let s = String::from_utf8(good.clone()).unwrap().replacen(find, replace, 1);
            assert!(Head::from_json(s.as_bytes()).is_err(), "{find} → {replace}");
        }
        let mut h = head.clone();
        h.weights[2][3] = f32::NAN;
        assert!(h.validate().is_err());
        let mut h = head.clone();
        h.min_confidence = 1.5;
        assert!(h.validate().is_err());
        let mut h = head.clone();
        h.bias.pop();
        assert!(h.validate().is_err());
        // wrong number of views, wrong length, NaN
        assert!(decide(&head, &vec![vec![0.0; 8]; 3]).is_err());
        assert!(decide(&head, &[vec![0.0; 7], vec![0.0; 8], vec![0.0; 8], vec![0.0; 8]]).is_err());
        assert!(decide(&head, &[vec![f32::NAN; 8], vec![0.0; 8], vec![0.0; 8], vec![0.0; 8]]).is_err());
        // too few photos to fit, bad dimensions
        assert!(fit(&toy_examples(3, 8, 0.5), "t", 8, "t", &FitOptions::default()).is_err());
        assert!(fit(&ex, "t", 0, "t", &FitOptions::default()).is_err());
        assert!(fit(&ex, "t", 9, "t", &FitOptions::default()).is_err());
    }
}
