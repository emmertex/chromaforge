### Pixel Shift registration
- **Photo ▸ Photo Merge ▸ Pixel Shift** now measures where each of the 16 frames really was and builds the image from those positions, so a series shot on a tripod that is not quite rigid, or with the camera nudged between frames, comes out sharp instead of soft and fringed. It takes about a minute and a half for a 45-megapixel series.
- **Pixel Shift, Plain Merge** places the frames on the camera's own pattern, as before: seconds, no measuring.
- A series that already has a merge can be merged again, registered or plain: each result is its own file (`-PixelShift.dng`, `-PixelShift-Plain.dng`, then `-2`…) and photo, stacked with the frames, and nothing is replaced.
- A series that cannot be measured, or whose measured positions are less reliable than the camera's own pattern, is merged plainly, and a message says why. Cancelling at any stage leaves no file behind.
