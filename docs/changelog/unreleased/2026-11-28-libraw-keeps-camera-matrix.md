## Fixes

- **LibRaw no longer overrides the camera table:** a raw read through LibRaw (a camera set to LibRaw, or a file lcc-raw refuses) took LibRaw's colour matrix as the file's own, so a body the app has measured matrices for rendered with LibRaw's instead of the app's. LibRaw's matrix is now used only when neither the file nor the app's camera table has one.
