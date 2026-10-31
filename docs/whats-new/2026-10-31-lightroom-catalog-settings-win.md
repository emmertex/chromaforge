### A Lightroom catalog's edits win over the settings saved in its raws
- Importing a Lightroom Classic catalog gives every photo it adds, and every photo only browsed and not changed
  since, the catalog's develop settings and Lightroom history. Before, a raw whose file held XMP settings (embedded
  in a DNG, or in a `.xmp` sidecar) kept those instead and lost its history, so any edit made in Lightroom since its
  metadata was last saved to the file was lost, and the import report counted the photo as one whose existing
  edits it had preserved. A photo edited in the library still keeps its settings, and so does a browsed one changed
  in any way since browsing (a rating or a keyword counts), unless `library.importLightroom` is run with
  `updateExisting`.
