### DNG export, CR2 and download hardening
- Float DNGs the app writes (HDR merge, Render to DNG) now store the white level in a form other DNG readers accept.
- Canon CR2 files with an impossible slice layout are refused instead of showing scrambled pixels.
- A face index larger than 1 GiB is loaded again instead of being rebuilt.
- Closing a library no longer removes the lock note of another library that took its place on the same path.
