## Develop and masking

- **Color Mixer per mask:** masks carry an 8-band Color Mixer of their own (`Mask::mixer`), applied within the mask
  before its Point Color (CPU and GPU), with a flyout under the mask's Color group, `mask.mixer` and
  `mask.mixerTargeted`. Thanks @OneSlips. (no PR)
