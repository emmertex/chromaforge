## Library

- **Reload from Disk reports unread files:** files it can't read (gone, unreadable, on a drive that isn't responding) are counted in its message instead of being skipped silently; `photo.reload` returns them as `failed: [{id, error}]`.
