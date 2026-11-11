### Validate DNG Files and Reset Export Status

- Library ▸ Validate DNG Files recomputes the raw-image checksum of the selected DNGs in the background (cancel from the activity row) and lists the damaged files, and the ones that carry no checksum to compare, in a result dialog.
- DNGs written by the app now carry a checksum of their raw data, so they can be validated later.
- Photos you export get an exported badge in the grid, and Photo ▸ Reset Export Status clears it for the selection. Catalogs from before open as usual and are upgraded to format 17 (older builds will not open them afterwards).
