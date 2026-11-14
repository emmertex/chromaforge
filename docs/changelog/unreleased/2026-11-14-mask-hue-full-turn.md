## Develop and masking

- **Mask Hue can reach ±180°:** the mask Hue is now stored as an angle in degrees, and the new Settings ▸ Interface ▸ Expanded sliders option (off by default) widens its slider from about ±34° to ±180°. Existing edits render at the same angle; XMP keeps `crs:LocalHue` clamped to Lightroom's range and adds `lc:LocalHueDegrees` only for a value past it. Issue #1145.
