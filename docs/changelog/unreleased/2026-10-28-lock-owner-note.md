## Fixes

- **Lock notes:** closing a library whose folder was replaced while it was open (a share mounted over it, or another library moved to its name) no longer deletes the other library's `catalog.lock.owner`. Upstream issue #3240. (no PR)
