//! Registered Pixel Shift merge: measuring the frames' sub-pixel positions from the data and reconstructing with them
//! (work in progress; PR A's placement in [`crate::pixelshift`] is unchanged and stays the fallback).
//!
//! Foundation: [`fft`] (transforms and phase correlation), [`series`] (the green samples of a series) and [`tiles`]
//! (tile grid, texture score and selection). Measurement: [`measure`] (one tile against a dense reference),
//! [`coarse`] (start positions), [`model`] (similarity per frame) and [`positions`] (a whole series). The gate, the
//! gains and the reconstruction follow in later steps.
//!
//! The method rests on published work, named in the module that uses it: Lucas and Kanade (1981) and Baker and
//! Matthews (2004) for the registration, Kuglin and Hines (1975) for the coarse start, Takeda, Farsiu and Milanfar
//! (2007) and Farsiu et al. (2004, 2006) for the reconstruction, Gunturk et al. (2002) for the colour planes. No code
//! from any other raw converter or pixel-shift tool was read or used.

pub mod coarse;
pub mod fallback;
pub mod fft;
pub mod gain;
pub mod gate;
pub mod measure;
pub mod model;
pub mod pipeline;
pub mod positions;
pub mod recon;
pub mod register;
pub mod samples;
pub mod series;
pub mod tiles;

#[cfg(test)]
mod accuracy;
#[cfg(test)]
mod synth;
