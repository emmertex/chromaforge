## Library

- **Auto orient:** Photo ▸ Rotate & Flip ▸ Auto Orient (AI) (`photo.autoOrient`) turns selected photos that are shown sideways or upside down upright, as one undoable library edit, leaving photos it is unsure of alone. It runs the CLIP ViT-B/32 model on four turns of each photo and a fitted 4-way classifier (`ml.orientation.fit`, `ml.orientation.status`) (#490).
- **ONNX Runtime for CLIP:** the CLIP vision tower is written as an ONNX graph from the installed weights and runs through ONNX Runtime on the graphics card where the desktop build has it, checked against the CPU encoder, before the wgpu kernels (#491).
