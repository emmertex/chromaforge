### Auto orient (AI)
- Photo ▸ Rotate & Flip ▸ Auto Orient (AI) finds selected photos shown sideways or upside down (scans, photos from messaging apps) and turns them upright, in one step you can undo. It uses the CLIP model from AI search and skips photos it isn't sure about.
- It needs an orientation classifier: fit one on a library of upright photos with `ml.orientation.fit`.
- On a graphics card, the CLIP model's image side now runs through ONNX Runtime where the desktop app has it, checked against the app's own code first.
