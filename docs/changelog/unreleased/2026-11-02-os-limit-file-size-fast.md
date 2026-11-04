## Performance

- **Faster "Limit file size":** the quality that fits is predicted from a small mosaic of the photo, so a limited JPEG takes at most two full encodes instead of a binary search, and a gain map JPEG encodes its map once. Thanks @OneSlips. (no PR)
