## Performance

- **Drafts drawn straight from the GPU, two graphics cards:** the desktop app offers the window's GPU device to the renderer, so a slider drag's drafts stay on the graphics card as textures (no readback, conversion or re-upload; `<PREFIX>_GPU_WINDOW=0` opts out). Settings ▸ Performance chooses the graphics card for editing (Automatic: the screen's) and a second graphics card that takes exports, previews built ahead and thumbnails, with a gate and memory budget of its own; its loss leaves the editing card alone (`app.gpu` `card`, `helper`, `adapters`). Thanks @OneSlips. (no PR)
