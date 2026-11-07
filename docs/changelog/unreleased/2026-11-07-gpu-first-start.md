## Performance

- **The first start after an install or a driver update runs on the GPU:** building the graphics kernels with an empty driver shader cache took about 30 s on an RTX 4070 SUPER and an RTX 5070, as long as the start-up limit, so the app gave up on the card and edited on the CPU until the next start. The limit now counts from the last kernel built: a start that keeps building kernels gets the time it needs (up to 5 minutes), while a graphics driver that stops answering is still given up on after 30 s.
