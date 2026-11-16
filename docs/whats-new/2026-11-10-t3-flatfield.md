### Flat-field correction

- Library ▸ Flat-Field Correction… fits a smooth per-colour gain map from a flat-field reference raw (shot through a
  diffuser with the same lens and aperture; make it the active photo) and writes each other selected raw as
  `<name>-FlatField.dng` with that map as a DNG GainMap, stacked on its original. Vignetting and lens colour cast
  are corrected; photos from a different sensor size or colour filter layout are refused.
