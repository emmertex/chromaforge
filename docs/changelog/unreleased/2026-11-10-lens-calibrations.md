## Camera colour
- Calibrations can be keyed on the lens model (#1015): a lens-specific calibration is used only for photos taken with that lens, a lens-less one for every other photo of the camera, and another lens's calibration never. `calibrate-lightroom fit`/`add` take `--lens`, and `add` refuses to overwrite a calibration for the same camera, lens and editor without `--replace`.
