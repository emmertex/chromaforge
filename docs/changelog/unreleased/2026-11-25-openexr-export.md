## Export

- **OpenEXR export:** a new export format writes the rendered photo scene-linear as 16-bit half or 32-bit float OpenEXR (lossless ZIP), in linear Rec. 2020 or ACEScg primaries recorded in the file's `chromaticities` attribute. Agents use `app.export` with `format: "exr"`, `bitDepth` 16 or 32 and `exrPrimaries` (`rec2020` or `acescg`).
