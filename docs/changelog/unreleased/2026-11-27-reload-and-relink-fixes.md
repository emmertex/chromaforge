## Fixes

- **Reload from Disk and relinking:** a Reload that finds only the checksum new no longer adds an undo step (which
  cleared the redo history), and an undo no longer drops the checksum of the file on disk; Reload leaves Immich
  link-only photos alone (it re-read the server's preview as if it were the original); relinking a virtual copy or
  its master to another file no longer gives the rest of the family that file's size and content hash; reading
  originals fills in an empty camera serial number and lens range; undoing a step that recorded a checksum keeps the
  photo's device-sync record with it.
