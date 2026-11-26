## Performance

- **Originals are read once after a catalog import:** reading the originals of a Lightroom catalog import (and
  Reload from Disk, the preview-only recheck and relinking a never-read photo) now records the SHA-1 the same read
  computes, so the background checksum pass no longer reads every file a second time; Immich lookups and device sync
  get the checksum as soon as the file is read.
