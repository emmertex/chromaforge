## Masking

- **People masks on the people models:** `mask.add {kind: "people", person?, parts?, who?}`; the selection is computed per photo by MediaPipe Selfie Multiclass + Face Landmarker (Apache-2.0) on the AI worker, one analysis per photo shared by every part, and stored with the mask (presets and Auto Sync carry the recipe, never the bitmap). Falls back to the heuristic without the models. Thanks @OneSlips. (no PR)
