### XMP sidecars with Immich, both ways

- Settings → Connections → **XMP Sidecars** on an Immich server can now **Pull** the XMP sidecars Immich stores for
  the linked photos: ratings, descriptions, keywords, GPS and the develop settings another Chromaforge instance
  published come back into the catalog and beside the original files. Photos edited here after the asset changed are
  left alone unless you choose to overwrite them.
- **Push** writes the whole sidecar for photos in a mapped external-library folder, and sends the rating,
  description, capture time, place and tags of Immich-managed photos, which Immich merges into the sidecar it keeps.
- **Full sidecars for managed photos** (opt-in): when an asset was uploaded without an XMP sidecar, Push can still
  give it one. It reads the sidecar Immich holds, merges your metadata and develop settings into it (other apps'
  data is kept), and copies it back through a tiny temporary asset, which is then removed. The original is never
  deleted or re-uploaded; it needs the key's `assetFile.read`, `asset.upload`, `asset.copy` and `asset.delete`
  permissions.
- Pulling needs Immich 3.2 or later and the key's `assetFile.read` permission; pushing the supported fields works on
  every supported server. An uploaded original also carries its whole XMP now (Immich used to reject an upload with
  a sidecar).
