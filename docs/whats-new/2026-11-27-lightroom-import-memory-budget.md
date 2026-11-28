### Large Lightroom catalogs are no longer refused at a million photos
- A Lightroom catalog import no longer stops at a million rows in a table: it may use three quarters of the memory free when it starts, and says so plainly when a catalog needs more, importing nothing (#1361).
- Set `<PREFIX>_LIGHTROOM_IMPORT_MEMORY` to the GiB the import may use, to allow more or keep room for other apps.
- Reading a catalog takes about half the memory it did: a synthetic 200,000-photo catalog peaked at 3.3 GB instead of 6.3 GB while it was read.
