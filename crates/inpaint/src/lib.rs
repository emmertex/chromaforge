//! Content-aware hole filling (inpainting) for the Remove tool.
//!
//! * [`complete`]: multi-scale patch-based completion (Wexler, Shechtman, Irani 2007) with the
//!   nearest-neighbour field found by PatchMatch (Barnes et al. 2009). See [`patchmatch`].
//!   Straight edges and lines crossing the hole (horizons, frames, wires) are found first and
//!   continued through it by guide channels in the patch distance ([`structure`]).
//! * [`complete_guided`]: the same with a [`Guide`], e.g. pixels that must never be copied from.
//! * [`membrane_fill`]: smooth harmonic fill, used to seed the coarsest level and as a fallback.
//! * [`dilate`]: grow a mask by a disc (exact Euclidean distance transform, [`mask`]).
//!
//! Buffers are interleaved `w × h × ch` floats in any colour model plus a `w × h` hole mask.
//! Everything is deterministic (fixed-seed RNG, results independent of the thread count) and never
//! panics on mismatched sizes: such calls return `None` or the input unchanged.
//!
//! Sources: C. Barnes, E. Shechtman, A. Finkelstein, D. B. Goldman, *PatchMatch: A Randomized
//! Correspondence Algorithm for Structural Image Editing*, SIGGRAPH 2009; Y. Wexler, E. Shechtman,
//! M. Irani, *Space-Time Completion of Video*, PAMI 2007.
//!
//! Ported from PhotoCraft (`crates/algo/src/inpaint.rs`, `poisson.rs`, MIT OR Apache-2.0, same
//! organisation; see `NOTICE`).
//! Synced with PhotoCraft 8d2e615 (2026-10-18): reviewed, nothing ported (upstream rewrote the solver).

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod mask;
pub mod membrane;
pub mod patchmatch;
pub mod structure;

pub use mask::dilate;
pub use membrane::membrane_fill;
pub use patchmatch::{CompleteParams, Guide, complete, complete_guided, complete_with};

/// A cancelled [`complete_with`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled")
    }
}

impl std::error::Error for Cancelled {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_sizes_do_not_panic() {
        assert_eq!(complete(4, 4, 3, &[0.0; 5], &[true; 16], &CompleteParams::default()), None);
        assert_eq!(complete(4, 4, 0, &[], &[true; 16], &CompleteParams::default()), None);
        assert_eq!(complete(usize::MAX, 2, 3, &[0.0; 6], &[true; 2], &CompleteParams::default()), None);
        assert_eq!(membrane_fill(4, 4, 1, &[0.5; 3], &[true; 16]), vec![0.5; 3]);
        // A hole covering everything: nothing to copy from.
        assert_eq!(complete(16, 16, 1, &[0.5; 256], &[true; 256], &CompleteParams::default()), None);
        // A tiny image smaller than a patch.
        let _ = complete(2, 2, 1, &[0.5; 4], &[true, false, false, false], &CompleteParams::default());
    }

    /// Hole shapes at the image edge, 1-pixel-wide images, near-full holes: the output keeps its size, is finite,
    /// leaves known pixels alone and (maximum principle) fills the hole within the known value range.
    #[test]
    fn edge_and_degenerate_holes_stay_bounded() {
        let cases: &[(usize, usize)] = &[(0, 0), (1, 1), (1, 40), (40, 1), (3, 3), (17, 9), (64, 48)];
        for &(w, h) in cases {
            let n = w * h;
            let img: Vec<f32> = (0..n).map(|i| 0.2 + 0.6 * ((i * 7919) % 101) as f32 / 100.0).collect();
            let holes: Vec<Vec<bool>> = vec![
                vec![false; n],
                (0..n).map(|i| i == 0).collect(),
                (0..n).map(|i| i + 1 == n).collect(),
                (0..n).map(|i| i % w.max(1) < 2).collect(),
                (0..n).map(|i| i != n / 2).collect(),
                vec![true; n],
            ];
            for hole in &holes {
                let known: Vec<f32> = img.iter().zip(hole).filter(|(_, h)| !**h).map(|(v, _)| *v).collect();
                let (lo, hi) = known.iter().fold((f32::MAX, f32::MIN), |(l, h), v| (l.min(*v), h.max(*v)));
                let mut outs = vec![membrane_fill(w, h, 1, &img, hole)];
                if let Some(o) = complete(w, h, 1, &img, hole, &CompleteParams::default()) {
                    outs.push(o);
                }
                for out in outs {
                    assert_eq!(out.len(), n, "{w}x{h}");
                    for (i, v) in out.iter().enumerate() {
                        assert!(v.is_finite(), "{w}x{h} px {i}");
                        if !hole[i] {
                            assert_eq!(*v, img[i], "known pixel changed at {i} in {w}x{h}");
                        } else if !known.is_empty() {
                            assert!(*v >= lo - 1e-3 && *v <= hi + 1e-3, "{w}x{h} px {i}: {v} outside {lo}..{hi}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn cancellation_stops() {
        let img = vec![0.5f32; 64 * 64];
        let hole: Vec<bool> = (0..64 * 64usize).map(|i| (i % 64).abs_diff(32) < 4 && (i / 64).abs_diff(32) < 4).collect();
        assert_eq!(complete_with(64, 64, 1, &img, &hole, &CompleteParams::default(), &|| true), Err(Cancelled));
    }
}
