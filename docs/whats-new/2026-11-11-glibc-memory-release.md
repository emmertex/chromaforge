### Memory returned after preview builds on Linux

- After a background preview build, the app now hands the memory the allocator kept back to the system on glibc Linux, so the resident size drops back instead of staying near the build's peak.
