### No more Vulkan driver stalls at 100 % zoom

- Drags at 100 % zoom no longer stutter on NVIDIA's Vulkan driver: the buffers that carry pictures to and from the graphics card are reused from pools instead of being created and freed for every render, so frames no longer wait on the driver's global allocation lock. Nine drags at 100 % on 1:1 tiles went from 20 frames over 100 ms to none (worst frame 36 ms instead of 576 ms).
- A graphics card's buffer pool may hold up to a third of its own memory while it works; it is emptied when the app is idle, and every failure falls back to the old behaviour.
