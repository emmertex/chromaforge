//! Reads a raw file with [LibRaw](https://github.com/LibRaw/LibRaw): its unpacked mosaic, colour filter layout,
//! black and white levels, as-shot multipliers and LibRaw's camera matrix. Nothing more: LibRaw's own processing
//! (black subtraction, scaling, white balance, demosaic, colour) is never run, the app develops the mosaic itself.
//!
//! LibRaw is C++, the one C/C++ dependency of the product (AGENTS.md, "Non-negotiables"), so it is opt-in: this
//! crate links it only when its build finds LibRaw (`LIBRAW_DIR` or `LIBRAW_SYSTEM=1`, see `build.rs`), and the engine
//! uses this crate only with its `libraw` feature. Built without it (and always on the web, Android and iOS)
//! [`available`] is false and [`decode`] says so.
//!
//! The unsafe FFI calls live in `ffi` (built with `cfg(lcc_libraw)` only); the layout arithmetic in [`layout`] is
//! plain Rust and checks LibRaw's numbers as hostile input.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(lcc_libraw)]
mod ffi;
pub mod layout;

pub use layout::{BlackGrid, Facts, Pattern, Unpacked};

/// Whether this build has LibRaw.
pub fn available() -> bool {
    cfg!(lcc_libraw)
}

/// LibRaw's version (`"0.23.0-…"`), when this build has it.
pub fn version() -> Option<String> {
    #[cfg(lcc_libraw)]
    {
        Some(ffi::version())
    }
    #[cfg(not(lcc_libraw))]
    {
        None
    }
}

/// Unpack a raw file held in memory.
pub fn decode(bytes: &[u8]) -> Result<Unpacked, String> {
    #[cfg(lcc_libraw)]
    {
        ffi::decode(bytes)
    }
    #[cfg(not(lcc_libraw))]
    {
        let _ = bytes;
        Err("this build of the app has no LibRaw".into())
    }
}
