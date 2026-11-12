## Fixes

- **Lightroom import: a collection member whose `collection` is not an id joins no album.** It no longer lands in collection 0 or the truncated id; the import reports one warning (upstream #4098, issue #4095).
