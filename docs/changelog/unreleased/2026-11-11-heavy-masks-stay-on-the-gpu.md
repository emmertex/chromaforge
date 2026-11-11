## Performance

- **Masks over the storage-buffer limit stay on the GPU:** a 45 MP export with 12 radial masks and a Subject mask needed one 2,220 MiB buffer for the mask planes, over the 2,047 MiB storage-buffer limit, and fell back to the CPU (about 10 s per photo instead of about 1.6 s). Masks that don't fit together now each get their own buffer and the per-pixel finish pass reads them band by band, so heavily-masked exports stay on the GPU and take about 2.5 s instead of about 10 s. Thanks @OneSlips. PR #1054.
