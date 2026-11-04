## Performance

- **Faster HDR gain map exports:** the HDR rendition reuses the SDR render's decoded original and stages, and the gain map is computed on every core. Thanks @OneSlips. (no PR)
