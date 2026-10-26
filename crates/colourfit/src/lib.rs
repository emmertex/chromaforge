//! Lightroom-matched camera calibration: fits a camera's colour model and named looks
//! ([`lcc_camdb::calibration::Calibration`]) to Lightroom's exports of the same raws.
//!
//! Stages (one module each):
//! - [`compare`]: register two renders of one photo and report ΔE00 by tone and hue (any editor).
//! - [`dataset`]: pair raws with exports, decode both, register them, sample colour pairs ([`dataset::PhotoSamples`]).
//! - [`fit`]: tone curve, colour and forward matrices, hue/saturation map, per-look look tables, closed loop through
//!   the real pipeline ([`fit::fit`]).
//! - [`preset`]: the baseline Lightroom preset contributors apply before exporting.
//! - [`probe`]: synthetic probe DNGs and slider sweeps, read back by position.
//! - [`report`]: held-out ΔE00 and the local HTML report (contains images: never sent).
//! - [`verify`]: the app's render of edited photos against their edited exports (migration check).
//!
//! Sources: own design; DNG specification 1.7 chapter 6 (profile model); CIEDE2000 (Sharma, Wu, Dalal 2005).
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod compare;
pub mod dataset;
pub mod deltae;
pub mod fit;
pub mod preset;
pub mod probe;
pub mod report;
pub mod verify;

/// Errors a contributor can act on.
#[derive(Debug, Clone, PartialEq)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<String> for Error {
    fn from(s: String) -> Self {
        Error(s)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
