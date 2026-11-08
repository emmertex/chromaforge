## Performance

- **AI semantic search on the GPU on every OS (issue #896):** CLIP's transformer blocks run on wgpu kernels (layer norm, linear with GELU and residual fused, multi-head attention) on Vulkan, DX12 and Metal, replacing the macOS-only candle Metal path. A tower is checked against the CPU on a test input before use, and any GPU error moves the job back to the CPU with the reason shown.
