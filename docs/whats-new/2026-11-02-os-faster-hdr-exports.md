### Faster HDR gain map exports
- An HDR JPEG with a gain map decodes and develops its photo once: the HDR rendition starts from the SDR render's
  decoded original and shared stages instead of decoding the raw again, and the gain map is measured on every core.
  Thanks @OneSlips.
