### HDR Merge can write its aligned frames as layers
- Photo Merge ▸ HDR has a new option, Also Write Aligned Layers (PSD). Besides the merged DNG it writes `<name>-HDR-Layers.psd`: one layer per bracket frame, aligned exactly as the merge aligned it, ready to blend by hand with masks in an image editor.
- Each layer is named after its photo and exposure step (for example `IMG_0001 (EV −2)`). Pixels the alignment moved in from outside the frame are transparent.
- Include Merged Layer puts the automatic result at the bottom. From the command line, `merge.hdr` also takes `layersFormat` (psd or psb), `bitDepth` (16 or 8), `colorSpace` and `deghostMask`.
- The file is added to the library and stacked under the merged DNG. Very large brackets are refused with a message naming the limit.
