## Performance

- **Exports take turns at the GPU, and a lost GPU comes back:** export renders share the graphics card in turns sized by its memory, behind the window's work; a readback waits only for its own work (a busy GPU sends one photo to the CPU instead of turning the GPU off), and a lost device is replaced after 3 s, 6 s, 12 s … (at most five times). Thanks @OneSlips. (no PR)
