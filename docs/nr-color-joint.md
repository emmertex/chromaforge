# Colour noise reduction: the joint filter

From process V8 (`docs/process-versions.md`) Chromaforge's colour noise reduction uses this method on the CPU
(`crates/pipeline/src/nrc_joint.rs`) and the GPU (`crates/gpu/src/wgsl/nrcj.wgsl`, in bands of rows whose halo covers
the filter's reach). Edits on V1 to V7 keep the blurred chromaticity and the speckle median. It is adapted from
upstream #1348, where the same method is upstream's process V6.

## Method

- Filter signed, linear `R-Y` and `B-Y`, with the original Rec.2020 luminance restored at each pixel. No channel
  clipping, root transform or Jensen correction is used in this filter.
- Estimate Y/chroma noise from median squared high-pass residuals at spacings 1, 2 and 4. Larger spacings account for
  positive correlations from demosaicing. Fit equal-count luminance bands with nonnegative coefficients, reject
  texture-dominated outliers and stop extrapolating beyond the bands supported by usable noise measurements.
- Speck brake (before the guide): a pixel whose chroma stands out from the median of the 8 pixels at distance 2 by
  more than a threshold (in noise variances), whose luminance step is small against that colour step, and whose colour
  shift does not reach the 16-pixel border of its 5 × 5 square at two places (a line or an edge through it always
  does) gets its chroma moved towards that median. Threshold 64 and at most half at Color 25, threshold 25 and fully
  at 100. Luminance is never changed, so a bright point (a star) keeps its brightness and, by the luminance test, its
  colour.
- Make a fixed, lightly denoised guide. At a clear directional edge, average along the smoother axis; this keeps thin
  isoluminant lines visible to the guide.
- Apply conservative, symmetric colour exchanges at increasing spatial scales. The two chroma components have
  identical neighbour weights. Subtract the expected noise from guide distances; luminance texture cannot simply
  increase the raw-pixel retention factor. Long taps also check the guide edges between their endpoints.
- Symmetric exchanges preserve channel sums on the full-resolution grid, including its borders. This does not promise
  identical means inside arbitrary cropped regions.
- Colour amount controls the radius and ramps the filtered contribution continuously from zero to the normal default
  of 25. Colour Detail changes edge tolerance; Smoothness changes radius (`local::nrc_params`: the coarse reach is
  three times `(2 + 10 · Color) · (1 + Smoothness)` px at full size, scaled with the output). The sliders are clamped
  to their ranges, and a value that is not a number counts as 0.
- A windowed render (a loupe tile, a zoomed view) of any bit depth requests extra surrounding pixels for the
  diffusion's reach (`joint_nr_reach`) and crops them off afterwards, so it matches the same pixels of the whole
  render (`a_deep_window_with_colour_noise_reduction_matches_the_whole_render`).
- The noise model belongs to the whole frame, not to the pixels a render holds (`lcc_pipeline::colour_noise`). A
  frame larger than 12 × 12 tiles of 128 px is measured on that grid of tiles spread over it (`nrc_joint::tiles`),
  resampled, tone-mapped, white-balanced and luminance-denoised like the frame (retouching and defringe are left
  out); a smaller frame rendered whole is measured on itself, a window of it on the whole frame. So a loupe window is
  filtered like the same pixels of the full render or the export
  (`joint_colour_nr_measures_the_noise_of_the_whole_frame`). The model is kept in the stage cache with the image
  before the noise reduction: moving a colour slider does not measure again. When a source plug-in filter runs on
  the photo, the tiles can't run it, so the render measures the pixels it holds instead.
- The stage cache keys the denoised image by the colour filter in use, so a change of process (Update to Current
  Process, undo) never reuses the other filter's result
  (`a_process_change_does_not_reuse_the_other_colour_filter`). Draft renders have no cheaper form of the joint filter:
  under V8 they run the full one.

## GPU bands

The GPU runs the filter in horizontal bands, each with a halo of `2 · Σ delta + max delta + 10` rows, so every band
sees the whole reach of the pixels it writes. The bands share a memory budget: half the device's memory as its
backend reports it (`lcc_gpumem`, `Gpu::vram`), less the renderer's own buffers in use and 384 MB for each render in
flight; 2 GB where the backend reports nothing. `<PREFIX>_NR_GPU_BAND_MB` overrides the budget. When not even a band
of a few rows fits, the GPU hands the image to the CPU filter, so the result is the same either way
(`joint_colour_nr_bands_match_one_piece_and_the_cpu`).

## Local checks

```sh
cargo test --release -p lcc-pipeline --lib nrc_joint
cargo test --release -p lcc-pipeline --lib probe_nrc -- --ignored --nocapture
cargo test --release -p lcc-gpu --test it equivalence
<PREFIX>_GPU=0 <cli> render INPUT.ARW -o OUTPUT.png --set detail.nrColor=100
```

`<PREFIX>_PROFILE=1` prints the filter's steps and times, the GPU band budget and the fitted Y/R-Y/B-Y model.

The regression tests cover signed dark means, luminance, chroma-noise reduction, leaf/black and isoluminant edges, a
two-pixel isoluminant line, structured highlights, spatially correlated noise, the speck brake (specks removed, stars
kept), hostile sizes and slider values (`joint_colour_nr_survives_hostile_input`) and the routing by process
(`colour_nr_follows_the_process`: every process before V8 renders exactly as before). `nrc_joint_reference.rs` keeps
an earlier version of the filter for bit-for-bit checks of later speedups.
