## Import

- **Lightroom catalog edits win over a raw's own XMP:** catalog import gives the photos it adds, and browsed photos
  unchanged since browsing, the catalog's develop settings, history and snapshots, instead of the settings saved in
  the file (embedded in a DNG or in a `.xmp` sidecar), which dropped every edit made in Lightroom since the file was
  last written. Settings the catalog leaves out start from the camera's defaults. (⇠ #3435)
