### Camera calibrations per lens
- A built-in calibration can now be for one lens of a camera, such as one of a phone's cameras that share a model name. A photo uses its own lens's calibration when there is one, otherwise the camera's general calibration, and never one made for a different lens.
- `calibrate-lightroom fit` and `add` take `--lens MODEL`. `add` no longer overwrites a calibration for the same camera and lens; pass `--replace` to replace it.
