### AI on the graphics card through ONNX Runtime
- The Windows, Linux and Apple silicon downloads now include ONNX Runtime with its WebGPU plug-in. AI Denoise and AI Super Resolution use it on the graphics card first (DirectX 12 on Windows, Vulkan on Linux, Core ML or Metal on a Mac, CUDA where installed), checked against the CPU result, then the built-in GPU code, then the CPU.
- Settings ▸ AI Denoise names what runs, for example "ONNX Runtime (WebGPU)". `ml.status` reports `onnxRuntime`.
- On Windows, a WebGPU plug-in newer than the tested one (which can end the app there) is skipped with a reason in `ml.status`, unless `<PREFIX>_ONNXRUNTIME_WEBGPU_UNTESTED=1` is set.
