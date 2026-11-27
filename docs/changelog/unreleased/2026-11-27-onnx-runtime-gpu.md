## Performance

- **ONNX Runtime on the graphics card (#491):** desktop downloads ship ONNX Runtime 1.32 and its WebGPU plug-in (Microsoft's builds, MIT; Dawn inside, BSD). AI Denoise and AI Super Resolution try it first on the card AI work goes to, checked against the CPU on a test tile, before the built-in wgpu code and the CPU. Without the library (the web, Android, riscv64, Intel Macs, or `<PREFIX>_ONNXRUNTIME=off`) nothing changes. `ml.status` reports `onnxRuntime`.
