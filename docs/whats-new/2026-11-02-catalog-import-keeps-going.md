### Catalogs with a print inside a collection import their collections
- Lightroom Classic can keep a saved print, book, slideshow or web gallery inside a plain collection, but an
  album sits only inside a folder. Importing such a catalog could stop at the end with "parent must be an
  existing folder" and bring over no collections, losing photos whose originals were offline. Such a
  collection now joins the nearest collection set above it, or the top level, and the import result names it
  under warnings; so does one whose parent is system-only or gone from the catalog. Duplicate collection
  listings import once, and a conflicting duplicate is skipped with a warning instead of stopping the import.
