## Library and culling

- **Eye focus strip:** each person's sharper eye at 100 % across the top of the loupe (View ▸ Eye Focus Strip), judged sharp / soft / blurry with closed eyes and sunglasses marked, kept in the preview cache; `eyes.analyze` prepares a folder and `photo.rejectEyes` rejects closed or blurry eyes (activity row, one undo step). Runs on the MediaPipe Face Landmarker (Apache-2.0), downloaded on request. Thanks @OneSlips. (no PR)
