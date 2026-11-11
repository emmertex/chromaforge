## Fixes

- **Lightroom import: records that name no photo are left out.** A develop settings, metadata, keyword, history, snapshot or album-member record whose `image` is missing, NULL, text or a fraction no longer goes to photo 0 (or 2.5 to photo 2); each table reports one warning (upstream #4090).
