//! Generative fill for the Remove tool (`docs/generative-fill.md`).
//!
//! A cloud image model (the user's own account and API key) fills a hole in a crop of the photo.
//! Models take and return display-referred 8-bit RGB, but the app edits scene-linear camera
//! data, so the result is brought back to the photo's own pixels and kept there, early in the
//! develop pipeline, where white balance, exposure, noise reduction and every other edit still
//! apply on top of it:
//!
//! * [`encode`]: the fixed, invertible "neutral" transform a crop is sent with (linear as-shot
//!   Rec.2020 → a gain → sRGB primaries → a flat log curve with two stops of highlight headroom →
//!   8 bits) and its inverse for what comes back.
//! * [`matching`]: the returned pixels, back in linear light, are matched in colour and level to
//!   the original pixels around the hole (a smooth correction field, like Heal's).
//! * [`patch`]: a [`patch::PatchLayer`] holds the matched pixels at the photo's native resolution
//!   with their blend weight and their place in normalized source coordinates. It is stored as a
//!   small file next to the library (never in XMP) and composited into the scene-linear image
//!   before white balance ([`patch::composite`]).
//! * [`noise`]: a generated patch is clean; renders give it sensor noise measured around it in the
//!   very image being rendered (any size, with or without AI Denoise), before noise reduction.
//! * `cloud` (feature): the providers, their request and response shapes, API keys kept outside
//!   the catalog, and a local mock server for tests.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod encode;
pub mod matching;
pub mod noise;
pub mod patch;

#[cfg(all(feature = "cloud", not(target_arch = "wasm32")))]
pub mod cloud;

pub use encode::Neutral;
pub use patch::{PatchError, PatchInfo, PatchLayer};
