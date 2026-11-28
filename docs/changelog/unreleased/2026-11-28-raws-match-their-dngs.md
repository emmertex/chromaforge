## Develop

- **Camera calibrations:** `calibrate-lightroom fit` measures, from a set with both converted DNGs and raws, the baseline exposure Lightroom gives the camera's raws (what a converted DNG carries as `BaselineExposure`), and writes it as the calibration's `raw_baseline_exposure`. A raw without colour tags of its own gets it under that calibration, so it renders as bright as its DNG (#1485).
