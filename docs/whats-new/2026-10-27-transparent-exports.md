### Transparent images stay transparent in PNG, TIFF and WebP exports
- Exporting a PNG logo or cut-out with transparency as PNG, TIFF or WebP keeps its transparency, instead of
  flattening it onto white. Edits apply to the colour only; crop, rotation, lens and Transform corrections and
  resizing move the transparency with the picture. PNG and WebP store it as straight alpha, TIFF as associated
  (premultiplied) alpha, as image editors do. JPEG, and the other formats without transparency in this export, still
  composite onto white, and the grid and loupe still show transparent areas on white.
