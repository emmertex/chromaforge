## Performance

- **Graphics interface setting (Windows):** Settings ▸ Performance ▸ Graphics interface chooses DirectX 12 or Vulkan for GPU rendering, saved with the app's settings. On some laptops DirectX 12's shader compiler made every start take about 15 seconds with the GPU on; Vulkan starts at normal speed. The window keeps DirectX 12, and `<PREFIX>_GPU_BACKEND` / `WGPU_BACKEND` still win over the setting. (no PR)
