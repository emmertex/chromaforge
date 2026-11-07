### Windows: starting with a broken graphics driver
- When DirectX 12 can't draw the window, the app now falls back to OpenGL instead of Vulkan, whose driver can crash the app before any window opens on some machines. Vulkan is still used when you ask for it with `<PREFIX>_GPU_BACKEND=vulkan`.
- A window larger than 8192 pixels, on a big or scaled display, can draw again.
