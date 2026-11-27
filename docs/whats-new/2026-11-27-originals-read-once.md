### Originals are read once
- Reading the originals of a Lightroom catalog import records each file's checksum too, so the background checksum pass no longer reads every file a second time.
- Reload from Disk no longer adds an undo step when only the checksum is new, and leaves Immich link-only photos alone.
- Relinking a virtual copy or its master to another file no longer gives the rest of the family that file's size and content hash.
- Reading originals fills in an empty camera serial number and lens range.
