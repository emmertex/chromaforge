### A failed window resize no longer drops rendering to the CPU

- When the window's own swap chain fails to reconfigure once (for example when switching virtual desktops under load), the GPU renderer now keeps running and the window configures it again on the next frame. Other GPU errors still stop it.
