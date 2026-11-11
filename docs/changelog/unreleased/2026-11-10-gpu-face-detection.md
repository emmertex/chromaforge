## Library

- Face detection (MediaPipe Face Landmarker: BlazeFace and FaceMesh) runs on the GPU through the face recognisers' wgpu kernels when a background check finds it matches the CPU and is faster; all of a photo's detector windows go to the GPU in one submission. Settings ▸ Faces shows the device; the CPU-only switch, lost-card fallback and activity row work as for recognition (#996).
