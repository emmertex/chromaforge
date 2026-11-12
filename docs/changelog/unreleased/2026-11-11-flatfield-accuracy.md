## Fixes

- **Flat-Field Correction follows a steep fall-off:** the gain maps are measured on a lattice of about one point every 86 pixels that reaches the frame's edges (a plane fit around each point, mildly smoothed) instead of a degree-4 surface through 17 × 17 points, which left the corners of wide lenses wide open 0.2 to 0.7 EV too dark (and up to 0.16 EV even at 105 mm). Gains may now reach 4 EV.
