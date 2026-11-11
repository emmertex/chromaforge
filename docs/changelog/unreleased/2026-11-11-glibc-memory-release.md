## Fixes

- **Memory (Linux):** freed allocator pages (`malloc_trim`) are returned to the system after a background preview build, so the resident size no longer stays near its peak (upstream #4054).
