### Large Lightroom catalogs keep their history
- Develop history from a Lightroom Classic catalog is no longer cut off at a fixed 32 MiB. The amount read grows with the catalog and your computer's memory, and when a catalog still holds more, every photo keeps its newest steps first instead of the first photos taking all of it (#1312).
- The recovery archive of a catalog import is no longer skipped past 32 MiB: it may take up to a quarter of the free space on the library's drive.
