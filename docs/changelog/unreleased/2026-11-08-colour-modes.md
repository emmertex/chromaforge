## Develop

- **Two colour-grading modes (Eboda | Witchcraft):** Settings ▸ General grows a Grading Mode row beside Layout
  (`view.gradingMode`, Ctrl+Shift+G). **Witchcraft** — now the default — lays colour grading out the DaVinci
  Resolve way: after Basic, the Primaries palette with **two sets of wheels that both render at once** — the Color
  wheels (Lift / Gamma / Gain / Offset, wide overlap) and the Log wheels (Shadow / Midtone / Highlight / Offset,
  `primaries.logWheels.*`, narrow ranges placed by `primaries.low`/`high`; complimentary as in Resolve, the Color |
  Log tabs only pick which set is edited), each wheel at its own tuned strength (the Color set on the published
  ASC CDL shapes), then free-point **HSL Curves** (Hue vs Hue / Sat / Lum: points anywhere over the hue circle,
  `develop.hslCurves`); both sections carry a small Witchcraft pill beside their titles, and the Modern layout's
  Colour ▸ Color Grading card follows the mode. **Eboda** is the Lightroom Classic panels unchanged. The new engine
  controls (`ctl:primaries.*`) apply per channel after the tone map on the CPU and GPU alike (equivalence-tested),
  travel in presets, history and `lc:settings` XMP, and are one undo step per drag. A presentation choice: both modes
  edit the same photo. Beyond the reference app (Resolve-style grading, read as prose from Blackmagic's Colorist
  Guide; the maths is our own).
