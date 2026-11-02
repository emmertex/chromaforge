## Develop and masking

- **Point Color per mask:** masks carry up to eight Point Color samples of their own (`Mask::point_colors`), applied
  after the photo's colour tools within the mask (CPU and GPU), with `mask.pointColorPick` / `mask.pointColorSet` /
  `mask.pointColorDelete`. Thanks @OneSlips. (no PR)
