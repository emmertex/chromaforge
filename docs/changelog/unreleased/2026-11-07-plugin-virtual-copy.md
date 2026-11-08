## Plug-ins

- **A plug-in can make a virtual copy of a photo:** `photo.virtualCopy {photo, name?}` (permission and host feature `virtualCopy`) makes one as *Create Virtual Copy* does, stacked with the photo and in its albums, named `name` or the next "Copy N", as one History step, and returns the copy's id. With `develop.setCrop` a plug-in can split one capture into several photos, such as two half-frame exposures in one 35 mm scan. Prepare steps and jobs can't call it. The SDK has `host::virtual_copy`. PR #891.
