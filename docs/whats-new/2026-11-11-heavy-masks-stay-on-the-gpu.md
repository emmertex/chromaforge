### Heavy masks stay on the graphics card

- Exporting a heavily-masked photo no longer falls back to the processor when the masks' planes together exceed the graphics card's 2,047 MiB storage-buffer limit: each mask gets its own buffer and the GPU reads them band by band, so such an export takes about 2.5 seconds instead of about 10.
