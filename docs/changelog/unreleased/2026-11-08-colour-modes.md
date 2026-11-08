## Develop

- **Two colour-grading modes (Eboda | Witchcraft):** Settings ▸ General grows a Grading Mode row beside Layout
  (`view.gradingMode`, Ctrl+Shift+G). **Eboda** is the Lightroom Classic panels unchanged; **Witchcraft**
  lays colour grading out the DaVinci Resolve way — a node strip of the grade's stages (click opens a panel, the
  number badge bypasses the stage: the section's eye or the mask's visibility), the Primaries palette with
  **Lift / Gamma / Gain / Offset** wheels and their master wheels in **Color** (wide overlap) and **Log** (narrow
  ranges with Low/High boundaries, `develop.primaries`) variants, and free-point **HSL Curves** (Hue vs Hue /
  Sat / Lum: points anywhere over the hue circle, `develop.hslCurves`). The new engine
  controls (`ctl:primaries.*`) apply per channel after the tone map on the CPU and GPU alike (equivalence-tested),
  travel in presets, history and `lc:settings` XMP, and are one undo step per drag. A presentation choice: both modes
  edit the same photo. Beyond the reference app (Resolve-style grading, read as prose from Blackmagic's Colorist
  Guide; the maths is our own).
