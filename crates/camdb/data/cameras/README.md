# Camera data

One TOML file per maker (`<maker>.toml`), embedded into the crate at build time by `build.rs`. The format and the
provenance rules are in the crate documentation (`src/lib.rs`); `<cli> camdb extract` prints entries from DNGs the
camera itself wrote.

This folder stays in the repository even while it holds no camera file: the build script watches it, and a watched
path that doesn't exist made cargo rebuild this crate and everything that depends on it on every run.
