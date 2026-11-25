## Performance

- **Faster Nikon High Efficiency decoding:** the inverse Star-Tetrix adds fixed-offset slices away from the image edges instead of calling a bounds-checked neighbour lookup per sample, its lifting steps update their plane in place, and each precinct's coefficients are written straight into the final bands instead of per-slice copies gathered serially; HE and HE★ raws decode 23 to 31 % faster with bit-identical output (#1377).
