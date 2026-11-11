## Fixes

- **Lightroom import: an image with a broken file reference is skipped.** An image whose `rootFile`, `folder` or `rootFolder` reference is missing, NULL, text or a fraction no longer resolves to record 0 (or 1 for 1.5) and merges into another photo; it is skipped with the existing missing-file/folder/root reasons (upstream #4097, issue #4094).
