### Windows: Vulkan first again, DX12 as the fallback

- On Windows the window and GPU rendering now use Vulkan by default, with DX12 as the fallback. DX12 builds the GPU kernels with a slow compiler, so a start with an empty shader cache could take over 30 seconds and end up rendering on the CPU (issue #934).
- A start that crashes in the Vulkan driver tries Vulkan last next time. `<PREFIX>_GPU_BACKEND=dx12` and Settings ▸ Performance ▸ Graphics interface still pick DX12.
