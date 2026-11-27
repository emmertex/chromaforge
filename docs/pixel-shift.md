# Pixel Shift Merge (Nikon)

> Names in angle brackets (`<app>`, `<cli>`, …) are the values set in [`brand.toml`](../brand.toml); see the README.

Merges the frames of a Nikon pixel-shift series into one image with full colour at every pixel, without NX Studio.
The merge runs on the raw mosaics, **before** demosaicing, and writes a LinearRaw DNG that is then developed like any
raw. By default the frames are **registered**: their positions are measured on the data and the image is reconstructed
from them (see [Registration](#registration)); the **plain merge** places them on the camera's own pattern, as the merge
was made before registration. Measured on a Z 8 (16-shot series, FX 3:2 and 1:1 image areas; the 4-, 8- and 32-shot modes
are recognised and refused by name); other bodies and shot counts are refused until they have been measured.

## Using it

- **App:** select all the frames of one series, then *Photo ▸ Photo Merge ▸ Pixel Shift* (registered) or *Pixel Shift,
  Plain Merge* (the camera's own pattern; the same entry is in the grid's context menu and the command palette). The merge
  runs in the background (a row in the activity stack that names the stage: reading frames, measuring positions,
  reconstructing, writing the DNG; ✕ stops it at any stage and leaves no file and no photo behind). The result `<frame 01 name>-PixelShift.dng` is written
  next to frame 01, imported, selected, and stacked on top of all the frames. It opens like a freshly imported NEF: the
  dialog's Auto Settings and Create Stack options belong to the other merges and are not used (`merge::parse_dialog`).
- A selection that is not one complete series is refused with the reason, in a message on the window and before any
  frame is decoded: a frame missing or twice, a file that is not a pixel-shift frame, frames of another shot or another
  camera, different exposure settings or image sizes, frames that were not shot one right after the other, or a shot count
  that is not supported yet. A failure while the merge runs (decoding, writing, a full disk) and a cancel are said the same
  way.
- **A result that exists:** a merge of a series that already has one is made, never refused, and never replaces a file or
  a photo (**a change from the first version of the Pixel Shift merge, which refused when `<frame 01 name>-PixelShift.dng`
  was already there**). The registered merge is `<frame 01 name>-PixelShift.dng`, the plain one
  `<frame 01 name>-PixelShift-Plain.dng`, and a name that is taken gets the next number (`-PixelShift-2.dng`,
  `-PixelShift-Plain-2.dng`). A series that was asked to be registered and was merged plainly takes the plain name. Every
  result is imported and stacked with the frames, the newest on top, with its own provenance (the time it was made is part
  of it, so two merges are never the same file). Delete Merge Sources works on any of them: it takes the frames that are
  still there and leaves the other results and frame 01.
- **Other merge kinds:** choosing HDR…, Panorama…, HDR Panorama… or Focus Stack… with exactly one complete series selected
  asks "This selection is a complete pixel-shift series. Merge as Pixel Shift instead?": *Pixel Shift* merges it with
  Pixel Shift's own defaults, *Continue with …* opens the chosen merge as usual, *Cancel* does nothing. Any other selection
  is not asked.
- **Delete Merge Sources** (*Photo ▸ Photo Merge*, the grid's context menu, the command palette): on a merged photo it
  moves its frames other than frame 01 to Recently Deleted like any deleted photo (the *Confirm before deleting* setting
  applies); frame 01 and the merged photo stay stacked. Frames that are already gone are counted in the message. It is
  greyed out on any other photo and when no frame but frame 01 is left. Command: `merge.deleteSources {id?}`.
- **When the series is not registered:** a series that cannot be measured (too little texture, frames that cannot be
  matched) or whose measured positions predict the frames worse than the camera's own pattern is merged plainly, and a
  message after the merge says why in plain words ("Pixel shift merge added, but not registered: not enough texture to
  measure the frame positions"). A registered merge that works gets no extra message. When the registered merge would not
  fit in the memory the merge may use, the plain merge is made and the message says that.
- **High ISO:** from ISO 1600 a registered merge starts with a hint that the plain merge may look cleaner. The hint blocks
  nothing; no series between ISO 64 and 3200 has been measured, so it is a hint, not a rule.
- **Command:** `merge.pixelShift {ids?, method=registered|plain, autoSettings=false, stack=true, outputDir?}`; the result
  carries `method`, `registration` (verdict, reason, gains, tiles) and, when the series was not registered, `fallback`
  (`reason` code and `message`). **CLI:** `<cli> merge pixel-shift [--plain] [--output-dir DIR] FILES…` (registered unless
  `--plain`; a fallback is noted on stderr).

## How a series is recognised

There is no series id in the files. The Nikon maker note of each frame has the pixel-shift mode (`0x005e` byte 4: 1, 2, 3, 4 for
a series of 4, 8, 16, 32 shots; 0 for a lone frame), the frame's number in the series (`0x0056` byte 8, 1-based), its
placement slot (`0x005e` byte 5: the number, except 17 to 20 for frames 5 to 8 of an 8-shot series), a per-exposure
counter (`0x00a7`) and the sensor position of the frame (`0x0054`, reported only). Tag `0x0034` says 16 in every mode and
for lone frames, so it is not the shot count and is only reported. These meanings were checked on every series of a Z 8 test
set of all four modes, with lone frames. A frame whose slot does not match its number for its
mode has no number. The selection's mode is judged first: a series of 4, 8 or 32 shots is refused by name ("n-shot
pixel-shift series are not supported yet"), whichever of its frames are chosen, and frames of different modes are refused as
different shot counts. A 16-shot selection is one series when its frames have the same camera, lens,
exposure (shutter, aperture, ISO, exposure compensation, white balance), raw size and image area, the numbers 1..N once
each, consecutive counters, and capture times spaced evenly relative to the series' own median interval (the camera's
interval setting runs from none, about 0.12 s, to many seconds). Only the first 512 KB of each file is read; the whole
file when the start does not hold the tags. Code: `crates/raw/src/pixel_shift.rs`.

## What the plain merge does

`crates/merge/src/pixelshift.rs`. The camera moves the sensor by one pixel between the four frames of a *set* and by half a
pixel between sets (16 shots = 4 sets). One set sees every scene point through a red, two green and a blue filter. A
fixed table by frame number says where each frame's samples land on a grid twice as fine as the sensor; each merged
pixel takes its red from one sample, its blue from one and its green as the mean (rounded down) of two. Nothing is
interpolated, nothing is registered, and the maker-note positions are not used to place anything (they are good to
about 0.1 px). Black and white levels stay the camera's (1008 / 16383 on the Z 8): the DNG's samples are the camera's
values. Frames are decoded, placed and dropped one at a time; peak memory is the merged image plus one frame.

The merged DNG covers the frames' whole active area; its default crop is the frames' crop at twice the size, which is the
area NX Studio's merge covers (16512 × 11008 for a Z 8 FX frame, 11008 × 11008 for the 1:1 area). The geometry comes from
the raw size, active area and crop tags, never from full-frame constants.

## The DNG

16-bit LinearRaw, three samples per pixel, tiled, lossless JPEG. It carries frame 01's make and model (so the file is a
Nikon Z 8 file for the colour look, camera profiles and white balance), its colour as DNG tags (fitted as for the other merges) and
its as-shot white balance, exposure data, capture time, lens data, Nikon distortion correction (as `OpcodeList3`; the
optical-centre offset is relative to the image, so it is the same in merged pixels), and a provenance block in the XMP.
The merged photo starts its colour as frame 01's NEF does (`Photo::colour_format`): a body whose colour matrices are known,
the Z 8 among them, edits white balance in Kelvin (temperature and tint) from the as-shot white balance; only a body
without matrices edits it relative to the camera's as-shot look.

Sizes for a 16-shot Z 8 FX series (16560 × 11040 samples):

| packing | size | write | decode |
|---|---|---|---|
| lossless JPEG (used) | 569 MB | 2.7 s | 0.8 s |
| deflate with prediction | 750 MB | 2.5 s | 0.5 s |
| uncompressed | 1097 MB | 4.2 s | 0.4 s |

Previews and thumbnails bin the three samples as they do for a NEFX (`crates/raw/src/binned.rs`); the DNG is recognised by
its provenance block.

### Provenance block (`lcps`)

XMP namespace `<project homepage>/ns/pixelshift/1.0/` (like the project's other own namespaces) in the DNG's own packet (`crates/raw/src/merge_provenance.rs`):
`Engine` (version of the merge engine: 1 for the plain merge, 2 for a registered one), `ShotCount`, `Reference` (the frame the file belongs to: always 1), `Scale`
(merged pixels per sensor pixel), the stored frames' place on the sensor (`SensorWidth/Height`, `WindowX/Y/Width/Height`,
for a later Vignetting Calibration), and `Frames`: for each frame its `Index`, `FileName`, `Captured` and `Counter`. How the
image was merged is written beside them, additively (a reader that does not know the attributes ignores them): `Method`
(`registered` or `plain`), `MergedAt` (when it was made, UTC), `GateVerdict` (`better`, `equal` or `worse`), `Reason` (why the plain merge was used:
`noTexture`, `tooFewTiles`, `noMatch`, `measuredWorse` or `memory`), `GainsApplied` (the frames' brightness differed by
more than noise, 0.5 % standard deviation, and was corrected), `Tiles` (the tiles the positions rest on) and `Models`
(each frame's shift, rotation and scale against frame 1: 16 × 4 numbers, under 1 KB, so a later tool can reproduce the
placement without measuring again). The catalog keeps the same without the models (`Photo.merge_sources`, catalog
format 19: the frames' photos and the reference; format 22: `registration`, a field older builds would drop silently,
hence the new format number: a library opened by this build is upgraded and older builds refuse it). When a merged DNG is
imported on its own (a rebuilt catalog, a re-imported folder) the catalog list is restored from this block: each frame is
found in the library by file name and capture time; a frame that is not there stays in the list as not found, and the
exposure counter is the file's own record of it. The block is untrusted input, so a frame is found in the library
only when everything agrees. The photo is in the merged file's folder, has the frame's file name and capture time and is not
a merge itself; its own maker note says it is that frame (the number and exposure counter the block lists, the block's shot
count); and all the photos found together are frames of one series (same camera, exposure and geometry, counters and capture
times that follow the frame numbers). Otherwise none is found: they stay in the list as not found, and Delete Merge Sources
has nothing to act on. Several merges of one stack may list the same frames; each is checked on its own, and Delete Merge
Sources acts only on photos that are still there and still agree with the recorded file name, so after the first one a
second finds the frames missing and touches nothing else.

A frame's header is read from the first 512 KB of its file; a note that is truncated, has tags of the wrong type or count,
offsets outside the file or loops, whose frame-number tags disagree, or whose counters wrap gives a header with no frame
number or a clean refusal, in bounded work (a date is bounded before it is used, all arithmetic on times is saturating,
counters wrap). Seeded mutation tests of both readers run in the normal test suite (`crates/raw/tests/it/hostile_pixel_shift.rs`,
`merged_dng.rs`; `LCC_FUZZ_ITERS` makes them longer).

**A file is untrusted input.** The block is read from files that may be crafted, so it is bounded: a packet over
1 MiB is not read; a block is used only when it is well formed and within its limits (a shot count of 4, 8, 16 or 32; at
most that many frames, each numbered 1 to the shot count and none twice; a reference frame within the shot count; a scale
of 1 to 8; file names of at most 512 bytes and capture times of at most 64; a sensor window inside its sensor). Anything
else, and anything malformed, is no block, and the DNG reads as the normal DNG it is.

**Forward compatibility.** Only those structural limits make a block invalid. A value a newer version may have added or
changed (the `Method`, `GateVerdict` or `Reason` text, the `Engine` number) that this version does not know reads as
unknown or none and never makes the block, the file or the library unreadable: the frames are still found. Concretely:
a `Method` other than `registered` or `plain` means how it was made is unknown; a verdict or reason it does not know is
none; an `Engine` it does not know is kept as a number and a missing or unreadable one is 0; `Tiles` above 100 000 is 0;
`GainsApplied` is true only when it says `true`; a `MergedAt` that is not a short ISO 8601 time is none; and `Models` is
used only when it holds at most one entry per shot, each of four finite numbers within range (a shift of at most 1000
sites, a rotation and a scale of at most 0.1), else it is dropped whole.

## Known differences from NX Studio's merge

Measured on two 16-shot series, against NX Studio output with chromatic-aberration correction off:

- Red and green are bit-identical over the whole frame. Blue is identical except at blue sites of every twelfth sensor
  row (rows ≡ 5 mod 12), where about half of NX Studio's values differ from ours by about the noise. The camera already
  stores smoothed values in those rows (their noise is a third lower than the other blue rows'); we keep the stored samples. No
  pattern shows on real images.
- **Lens correction starts on.** A merged DNG carries the lens data (Nikon distortion correction as `OpcodeList3`), and a
  DNG with lens data opens with Enable Profile Corrections on, as every such DNG does; a NEF or NEFX opens with it off.
  This is kept on purpose: the pixels are merged on the sensor grid and the correction is the lens's own, so the merged
  file looks right without a step. Switch it off in the Lens Corrections panel like any other setting.
- **Restoring a deleted frame does not put it back into the stack.** This is how every deleted photo behaves, not
  something of the merge: deleting a photo removes it from its stack (`remove_from_stacks_ops`) and Restore only clears its
  deleted flag. A restored frame is a normal photo again; stack it with the merged photo by hand (Photo ▸ Stacking). A
  general "restore stack membership" fix would help every stack and is left for a follow-up.
- No chromatic-aberration correction. The registered merge differs from NX Studio's by design: it measures where the
  frames really were and reconstructs from them, so a series shot on an unsteady tripod, which the plain merge shows as
  softness and colour fringes at fine detail (drift of up to about 0.6 px over the 16 shots), comes out sharp.

## Metadata of the merged file

The merged DNG gets frame 01's capture time with its sub-seconds and time-zone offset, in its Exif (`DateTimeOriginal`,
`SubSecTimeOriginal`, `OffsetTimeOriginal`, with the exposure, focal length and lens) and in its XMP, so the library dates
it with its series. Keywords are the union of the catalog keywords of all the frames (hierarchy kept, each once whatever its
case), written as a sidecar writes them. Creator, rights, title, caption, the IPTC descriptive fields, location and GPS are
frame 01's as the catalog has them. Rating, colour label, flag and edit settings are not carried.

## Registration

`crates/merge/src/pixelshift_registered/`. The camera's table says where each of the 16 frames should sit. Measured on
series from a Z 8, the frames' image positions do not match that table even on a steady tripod: the horizontal step between the frames
of a set is about 0.7 to 1.0 pixel instead of 1, depending on the orientation and the series, so the plain merge's
placement is off in almost every series, not only on a shaky setup; on a tripod that is not rigid the positions also
differ by up to a few pixels, and by tens of pixels when the camera moved. The registered merge measures the positions and
reconstructs the image from them.

1. **Brightness.** A global gain per frame and colour is estimated from the frames' means; when the frames' brightness
   varies by more than noise (0.5 % standard deviation) the green planes are put on a common level before anything is
   measured (a flickering light). The gains the reconstruction applies are measured per region and robustly afterwards.
2. **Positions.** Tiles of about 180 × 180 pixels with enough texture (at most two per 2 × 2 cells of a 16 × 12 grid) are
   measured on the green samples: every frame is fitted, by Lucas–Kanade iterations with gain and offset free, to a
   reference reconstructed from the other 15 frames, round after round. A quick check on the camera's table decides
   whether to start from it (a tripod series) or from phase correlation and a wide first stage (a series that drifted).
   A tile that does not settle is dropped. The tiles' positions are fitted by one similarity model per frame (shift,
   rotation, scale about the middle).
3. **Hold-out test.** Some tiles are held out; the positions fitted from the other tiles and the camera's table each
   predict the held-out tiles' samples, and a one-sided test (95 %, at least 1 % less error) says whether the measured
   positions are *better*, *equal* or *worse* than the table. *Better* and *equal* register (the reconstruction removes the
   camera pattern's aliasing even where the positions are about nominal); *worse*, or fewer than 8 converged tiles, merges
   plainly with the reason.
4. **Reconstruction.** Every sample is placed by its frame's model on a grid twice as fine as the sensor and the image is
   computed by local linear kernel regression with kernels that follow edges; green takes the narrowest kernel that holds
   enough samples, red and blue the smoothed ratios to green, so a sample that sits on a node passes through unchanged.
   The blue samples of the rows the camera fills itself (rows ≡ 5 mod 12) are kept, as the plain merge does: excluding
   them was tried and was not better.

Cost, measured on 16-shot Z 8 series of 45 megapixels: about 55 to 95 s with 24 threads and 85 to 125 s with 8 (reading,
measuring, reconstructing), about 4 s more to write the DNG, and 2.6 GB at the peak; the plain merge takes seconds and one
frame plus the image. The registered merge keeps all 16 frames in memory; when that does not fit, the plain merge is made.

### Limitations

- **High ISO.** The measurement and the reconstruction rest on the noise of the samples; at ISO 3200 the registered merge
  shows more noise in red and blue than the plain one in some series. The app hints from ISO 1600; the plain merge is one
  menu entry away.
- **Local motion.** Only the frames' common movement is measured and corrected; a subject that moves between frames (water,
  leaves, people) is placed as the plain merge places it, and can look worse registered than plain.
- **No texture.** A series without luminance texture in the green samples (a clear sky, fine red or blue detail without
  green contrast) cannot be measured and is merged plainly.
- **Large movement.** A drift of up to about 35 pixels over the series was followed in the tests; larger movement is
  untested (a series whose frames cannot be matched is merged plainly).
- **Time and memory.** About a minute and a half and 2.6 GB for a 45-megapixel series, against seconds for the plain merge.
- Not covered: the 4-, 8- and 32-shot modes (refused by name) and bodies other than the Z 8.

### Method and provenance

The method rests on published work; the code was written from these papers and from our own measurements, and no code was
read, copied or ported from any other raw converter or pixel-shift tool. The Nikon pixel-shift geometry was measured by us.

- Lucas, B. D., Kanade, T.: An iterative image registration technique with an application to stereo vision. IJCAI 1981;
  Baker, S., Matthews, I.: Lucas–Kanade 20 years on. Int. J. Computer Vision 56(3), 2004 (gain and offset in the fit).
- Kuglin, C. D., Hines, D. C.: The phase correlation image alignment method. Proc. IEEE Int. Conf. on Cybernetics and
  Society, 1975 (coarse start); Guizar-Sicairos, M., Thurman, S. T., Fienup, J. R.: Efficient subpixel image registration
  algorithms. Optics Letters 33(2), 2008.
- Takeda, H., Farsiu, S., Milanfar, P.: Kernel regression for image processing and reconstruction. IEEE Trans. Image
  Processing 16(2), 2007 (local linear regression, steering kernels); Wronski, B. et al.: Handheld multi-frame
  super-resolution. ACM Trans. Graphics 38(4), 2019 (kernel shape from the local structure).
- Farsiu, S., Robinson, M. D., Elad, M., Milanfar, P.: Fast and robust multiframe super resolution. IEEE TIP 13(10), 2004;
  Farsiu, S., Elad, M., Milanfar, P.: Multiframe demosaicing and super-resolution of color images. IEEE TIP 15(1), 2006.
- Gunturk, B. K., Altunbasak, Y., Mersereau, R. M.: Color plane interpolation using alternating projections. IEEE TIP
  11(9), 2002 (colour-plane correlation).
- Keys, R. G.: Cubic convolution interpolation for digital image processing. IEEE Trans. Acoustics, Speech, Signal
  Processing 29(6), 1981; Cooley, J. W., Tukey, J. W.: An algorithm for the machine calculation of complex Fourier series.
  Math. Computation 19, 1965 (the FFT); Student (Gosset, W. S.): The probable error of a mean. Biometrika 6, 1908 (the
  hold-out statistic).
- Dependency: `rustfft` (MIT or Apache-2.0) with its helper crates `primal-check`, `strength_reduce` and `transpose` (the
  same licences), used unmodified through Cargo; the `NOTICE` file names it.
