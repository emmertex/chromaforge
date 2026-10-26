## Colour

- **Calibrations matched to other editors:** `calibrate-lightroom fit --editor capture-one|darktable|rawtherapee|dxo-photolab` fits a camera calibration to that editor's exports. Calibration files gain an `editor` field (absent: Lightroom) and are kept as `<Make>_<Model>_<editor>.json`; calibrations of one camera pool their looks, and only a Lightroom calibration gives the camera its default look. (no PR)
