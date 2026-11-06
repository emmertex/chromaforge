## Fixes

- **Lens profiles in cropped image areas:** raws from a full-frame Nikon shot in 5:4, 1:1, 16:9 or DX get the lens-database correction for their part of the sensor, placed by where the maker note says the frame sits. The full-frame profile used to be stretched over the smaller frame: on a Z 7 in 5:4 with a Z 50 mm f/1.8 S at f/1.8, 0.46 EV too much vignetting correction and 16 px of distortion error at the corners. 3:2 photos are unchanged.
