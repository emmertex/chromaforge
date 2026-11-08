## Plug-ins

- **A plug-in can look at a photo and set its crop:** `photo.pixels` (permission and host feature `pixels`) gives a downsized copy of the uncropped frame as 8-bit sRGB, and `develop.setCrop` (permission and host feature `crop`) sets the crop and straighten angle in that frame as one History step. A job may read pixels but not set a crop. The SDK has `host::photo_pixels` and `host::set_crop`, and decodes base64 about five times faster. PR #891.
