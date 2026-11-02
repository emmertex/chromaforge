## Immich

- **XMP sidecars both ways (IMM-SIDECARS):** pull the sidecar Immich stores for a linked asset (`GET /asset-files`,
  v3.2.0+) into the catalog and the local file — metadata plus the `lc:settings` develop settings another instance
  published — and push the catalog's: whole sidecars for external-library folders, and the rating, description,
  capture time, place and tags Immich keeps for managed assets. Photos edited here after the asset changed are
  skipped unless overwritten. For managed assets an opt-in **full** mode reads the sidecar Immich holds, merges the
  catalog's metadata and develop settings into it (other apps' data stays), and copies it back through a tiny
  temporary asset (`PUT /assets/copy`), removing the temporary asset afterwards — the original is never deleted or
  re-uploaded. Publishing an original also carries its whole XMP (docs/immich.md).
- **Publishing an original now carries its XMP sidecar:** the whole XMP (metadata and develop settings) goes with an
  uploaded original, and Immich no longer rejects that upload ("Unsupported file type"): the multipart file parts are
  sent before the text fields, which is what its upload endpoint expects.
