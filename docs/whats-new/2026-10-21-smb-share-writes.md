### Libraries and sidecars on an SMB share
- On macOS, the catalog, XMP sidecars, smart previews and DNG conversions are written again on network shares that refuse the full flush to disk; the app falls back to a plain flush instead of failing the write.
