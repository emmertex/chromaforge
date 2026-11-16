### Measure Lightroom's sliders with probe images
- The command-line tool's `calibrate-lightroom probe` writes synthetic test images for your camera: colour patches,
  and copies with one Lightroom setting each (Exposure, Contrast, the colour sliders…). Render them in Lightroom
  and `probe-check` shows, for every slider value, which {app} value gives the same result, and how far the default
  looks differ. The base probe also helps a camera calibration with colours natural photos rarely have.
