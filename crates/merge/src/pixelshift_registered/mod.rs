//! Registered Pixel Shift merge: measuring the frames' sub-pixel positions from the data and reconstructing with them
//! (work in progress; PR A's placement in [`crate::pixelshift`] is unchanged and stays the fallback).
//!
//! This module holds the foundation: [`fft`] (transforms and phase correlation), [`series`] (the green samples of a
//! series) and [`tiles`] (tile grid, texture score and selection). The measurement, the gate, the gains and the
//! reconstruction follow in later steps.
//!
//! The method rests on published work, named in the module that uses it: Lucas and Kanade (1981) and Baker and
//! Matthews (2004) for the registration, Kuglin and Hines (1975) for the coarse start, Takeda, Farsiu and Milanfar
//! (2007) and Farsiu et al. (2004, 2006) for the reconstruction, Gunturk et al. (2002) for the colour planes. No code
//! from any other raw converter or pixel-shift tool was read or used.

pub mod fft;
pub mod series;
pub mod tiles;

#[cfg(test)]
mod synth;
