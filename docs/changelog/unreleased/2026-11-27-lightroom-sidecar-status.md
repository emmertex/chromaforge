## Import

- **Lightroom imports say which sidecars disagree:** imported photos start in step with their XMP sidecars, or as a metadata conflict (counted in the result) where a sidecar disagrees with the catalog, instead of all showing "changed in the catalog"; records past 64 MiB and XMP packets past 16 MiB are read within the catalog's and the memory's bounds, with oversized packets' colour lookup tables left out (#1312). Thanks @blwfish for the measurements.
