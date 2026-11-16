### XMP sidecars and imports on an SMB share (macOS)
- On a Mac, XMP sidecars weren't written next to photos on an SMB share, and an import that copies photos to a share
  imported nothing ("Operation not supported"). These writes now fall back to a plain flush there, as the library's own
  files do (upstream issue 1407). Moving photos to a share, sidecar backups and slideshow exports get the same fix.
