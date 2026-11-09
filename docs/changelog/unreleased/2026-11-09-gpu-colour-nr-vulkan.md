## Fixes

- **GPU colour noise reduction on Windows matches the CPU again:** on photos edited with an older process version (before V8), Color noise reduction on the graphics card picked the wrong 3 × 3 median on Vulkan, which Windows uses by default since issue #934. The result differed from the CPU render by up to 81 8-bit levels in colourful areas. The median now uses a comparator network that gives the same result on every backend.
