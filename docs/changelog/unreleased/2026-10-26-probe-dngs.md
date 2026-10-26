## Colour

- **Probe DNGs and slider sweeps:** `calibrate-lightroom probe` writes synthetic probe DNGs for a camera (384 patches through camera RGB, the camera's make, model and database matrix) plus one sweep step per Lightroom slider value with the setting embedded; `probe-check` reads Lightroom's exports back by position and reports the default render's ΔE00 and, for each step, the app value whose change matches Lightroom's best next to the value the import uses. The base probe pairs into `fit` like any raw. (no PR)
