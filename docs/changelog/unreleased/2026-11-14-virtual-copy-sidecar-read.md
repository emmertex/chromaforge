## Fixes

- **Virtual copies keep their own metadata:** saving the original's metadata to its XMP sidecar no longer flags its virtual copies as changed on disk, so automatic reads no longer put the original's keywords, title and rating on a copy. A virtual copy has no sidecar of its own.
