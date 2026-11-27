---
title: Photo merge
description: HDR, panorama, HDR panorama and focus stack.
sidebar:
  order: 17
---

- **Photo ▸ Photo Merge**: select the photos, then **HDR…** (`Ctrl+H`), **Panorama…** (`Ctrl+M`), **HDR Panorama…** or **Focus Stack…** (beyond Lightroom).
- **Dialog** previews at a small size and updates when an option changes; click the preview to zoom, drag to pan.
- **HDR**: Auto Align, Auto Tone, Deghost amount. Result `<name>-HDR.dng`. HDR is partial: local shifts such as parallax are only handled in part.
- **Panorama**: Spherical, Cylindrical, Perspective or Auto projection; **Boundary Warp**, **Fill Edges**, **Auto Crop** (set as the crop, so it stays adjustable). Result `<name>-Pano.dng`.
- **HDR Panorama**: brackets grouped by EXIF, merged one set at a time.
- **Focus Stack**: photos focused at different distances become one sharp DNG (partial).
- **HDR for Each Bracket Set** merges many brackets in one go. **Merge with last settings** skips the dialog.
- Runs in the background: a row in the activity stack with a stop button. The merged DNG is imported and selected; started while viewing an album, it is added to that album too (not to a smart album).
- **Pixel Shift** (Nikon Z 8, 16-shot series): select all the frames of one series and choose **Photo ▸ Photo Merge ▸ Pixel Shift**. The frames are merged on the raw mosaics into one image with full colour at twice the resolution, written next to frame 01 as `<name>-PixelShift.dng` and stacked on its frames. A selection that is not one complete series is refused with the reason (shown on the window), A series that already has a merge is merged again: the new result gets its own name (`-PixelShift.dng` registered, `-PixelShift-Plain.dng` plain, then `-2`, `-3`…), nothing is replaced and all results sit in the one stack. Choosing another merge kind on a complete series asks whether to merge it as Pixel Shift instead. **Photo ▸ Photo Merge ▸ Delete Merge Sources** moves the merged photo's frames other than frame 01 to Recently Deleted. By default the frames are registered: their positions are measured on the data and the image is reconstructed from them, which removes the softness and colour fringes of a series shot on an unsteady tripod. **Photo ▸ Photo Merge ▸ Pixel Shift, Plain Merge** places the frames on the camera's own pattern instead, without measuring (seconds instead of a minute or two). A series that cannot be measured (too little texture) or whose measured positions are less reliable than the camera's pattern is merged plainly and a message says why. The registered merge takes about a minute and a half and 2.6 GB for a 45-megapixel series, and a subject that moves between the frames can look worse than in the plain merge. At ISO 1600 and above the app hints that the plain merge may look cleaner.
