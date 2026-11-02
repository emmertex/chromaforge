## Develop and masking

- **Masks under geometry:** mask coordinates live on the photo before lens corrections, Upright and Transform
  (`Mask::on_photo`, `lcc_pipeline::mask_space`); Rotate turns them with the photo; CPU and GPU agree. Older masks
  are attached at their next edit. Thanks @OneSlips. (no PR)
