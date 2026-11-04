### Presets export as Lightroom XMP, masks included
- `preset.exportXmp` writes presets as Lightroom-style `.xmp` files: Subject, Sky, Background and People-part masks
  as AI masks Lightroom recomputes on each photo, gradients, luminance ranges, the mask sliders, and a mask's Curve
  and Point Color. What Lightroom has no equivalent for is listed; the whole preset is kept in the file too, so it
  imports back unchanged. Thanks @OneSlips.
- Lightroom presets bring their Background and People-part masks (and Landscape masks by name) as selections made
  again on each photo, instead of reporting them as not carried over.
