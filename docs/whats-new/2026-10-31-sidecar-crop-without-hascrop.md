### Crops in sidecars that omit HasCrop
- An XMP sidecar that lists crop edges or a straighten angle without `HasCrop` now imports its crop, as a Lightroom
  catalog already did. Presets still need `HasCrop`, so applying a preset never changes a photo's crop by accident.
