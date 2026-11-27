## Fixes

- **Start-up hang (Windows, Vulkan):** with an implicit Vulkan layer installed (RivaTuner, OBS's capture hook), some starts deadlocked: the window's swapchain set-up on the UI thread and the GPU renderer's device creation on a background thread each waited on the other inside the Vulkan loader. The renderer's device is now made only after the window has drawn its first frames.
