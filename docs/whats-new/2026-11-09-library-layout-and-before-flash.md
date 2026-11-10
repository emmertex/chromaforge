### Library safety and Before view
- A library saved in a layout this version does not read (an unrecognised top-level key in the catalog snapshot, or a `segments` folder beside it) is refused with the "written by a newer version" message and left untouched, instead of opening as an empty catalog that the next save would overwrite.
- With Before on, switching to an edited photo no longer flashes its edits in the Before view while the Before render is made.
