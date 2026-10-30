## Performance

- **Faster raw demosaic:** the default AHD demosaic takes its Lab cube roots with two Halley steps instead of the library `cbrt`, the most expensive call in it. (no PR)
