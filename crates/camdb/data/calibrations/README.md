# Camera calibrations matched to other editors

One JSON file per camera model, lens and editor (`<Make>_<Model>.json` for Lightroom, `<Make>_<Model>_<editor>.json` for
Capture One, darktable, RawTherapee or DxO PhotoLab; `<Make>_<Model>_<Lens>[_<editor>].json` for a fit made for one
lens, `--lens`), embedded into the crate at build time by `build.rs`. Each holds
the camera's colour model and named looks, fitted black-box by `<cli> calibrate-lightroom fit [--editor ID]` to that
editor's exports of a contributor's own photos (aggregate statistics only: no image content, file names, dates, serial
numbers or locations; a lens-specific fit records the lens model identifier only). A photo uses its lens's
calibration when there is one, else the lens-less one, never another lens's. The format and its checks are in `src/calibration.rs`; how contributors make one is in
[`docs/calibrate-lightroom.md`](../../../../docs/calibrate-lightroom.md).

Files are added only with `<cli> calibrate-lightroom add FILE`, which validates the contribution, writes it here in
the canonical form and adds its row to `assets/ATTRIBUTION.md`; it refuses to overwrite a file for the same camera, lens and editor unless
`--replace`.

This folder stays in the repository even while it holds no calibration: the build script watches it, and a watched
path that doesn't exist made cargo rebuild this crate and everything that depends on it on every run.
