//! Describe with open models: OWLv2 ([`crate::owl`]) finds boxes around what the text names,
//! SAM 2.1 ([`crate::Sam2`]) turns each box into a mask, and the masks are merged into one
//! selection of every instance ("car, road": every car and the road), as SAM 3's text prompt
//! does. No gated model is involved.

use crate::owl::{self, Detection, Owl, OwlEncoded};
use crate::{Prediction, Result, Sam2, Sam2Encoded};

/// Settings of a Describe selection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    /// Boxes scoring below this are not the thing described (OWLv2's scores are low: the
    /// reference post-processing keeps 0.1 and up).
    pub threshold: f32,
    /// Weaker boxes than this share of the best one are dropped too (stray matches next to a
    /// clear one).
    pub relative: f32,
    /// A box overlapping a stronger one by more than this (IoU) is the same instance.
    pub iou: f32,
    /// Most instances per selection.
    pub max: usize,
}

impl Default for Params {
    fn default() -> Self {
        Self { threshold: 0.1, relative: 0.25, iou: 0.5, max: 24 }
    }
}

/// The boxes to segment for `dets`: above the threshold and relative share of the strongest,
/// overlapping duplicates dropped.
pub fn boxes(dets: Vec<Detection>, p: &Params) -> Vec<Detection> {
    let best = dets.iter().map(|d| d.score).filter(|s| s.is_finite()).fold(0.0f32, f32::max);
    owl::select(dets, p.threshold.max(best * p.relative), p.iou, p.max)
}

/// The union of `masks` (logits of the same size): the most confident of them at each pixel.
pub fn union(masks: &[Vec<f32>]) -> Option<Vec<f32>> {
    let first = masks.first()?;
    let mut out = first.clone();
    for m in masks.iter().skip(1) {
        out.iter_mut().zip(m).for_each(|(o, v)| *o = o.max(*v));
    }
    Some(out)
}

/// Everything `text` describes on the photo: OWLv2's boxes (`owl_enc`), each segmented by SAM 2.1
/// (`sam_enc`, the same photo), merged. Logits as SAM 2.1's ([`crate::SAM2_MASK_SIDE`]² over the
/// stretched photo); the score is the strongest box's. `None` when nothing matches.
pub fn describe(owl: &Owl, owl_enc: &OwlEncoded, sam: &Sam2, sam_enc: &Sam2Encoded, text: &str, p: &Params) -> Result<Option<Prediction>> {
    let queries = owl::queries(text);
    if queries.is_empty() {
        return Ok(None);
    }
    let found = boxes(owl.detect(owl_enc, &queries)?, p);
    log::info!(
        "describe {text:?}: {} box(es){}",
        found.len(),
        found.iter().map(|d| format!(" {:.2}@[{:.2} {:.2} {:.2} {:.2}]", d.score, d.rect[0], d.rect[1], d.rect[2], d.rect[3])).collect::<String>()
    );
    let Some(score) = found.first().map(|d| d.score) else { return Ok(None) };
    let masks = found.iter().map(|d| sam.segment_box(sam_enc, d.rect, &[]).map(|m| m.logits)).collect::<Result<Vec<_>>>()?;
    Ok(union(&masks).map(|logits| Prediction { logits, score }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boxes_keep_the_clear_matches() {
        let d = |x: f32, score: f32| Detection { rect: [x, 0.0, x + 0.1, 0.1], score, query: 0 };
        let p = Params::default();
        // a strong match: weak ones near the floor go (relative), duplicates go (IoU)
        let kept = boxes(vec![d(0.0, 0.8), d(0.005, 0.7), d(0.3, 0.3), d(0.6, 0.15)], &p);
        assert_eq!(kept.iter().map(|k| k.score).collect::<Vec<_>>(), [0.8, 0.3]);
        // only weak matches: the absolute threshold decides
        assert_eq!(boxes(vec![d(0.0, 0.12), d(0.5, 0.05)], &p).len(), 1);
        assert!(boxes(vec![d(0.0, 0.05)], &p).is_empty());
        assert!(boxes(Vec::new(), &p).is_empty());
    }

    #[test]
    fn union_takes_the_most_confident() {
        assert_eq!(union(&[vec![-1.0, 2.0, -3.0], vec![1.0, -2.0, -4.0]]), Some(vec![1.0, 2.0, -3.0]));
        assert_eq!(union(&[]), None);
    }
}
