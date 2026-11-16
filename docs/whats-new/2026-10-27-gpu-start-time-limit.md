### A stuck graphics driver no longer freezes rendering
- When the graphics driver or display server never answered while the GPU was being set up, every render waited
  forever. Setting up the GPU now gives up after 30 seconds (`<PREFIX>_GPU_INIT_TIMEOUT` changes it), renders carry
  on on the CPU, and the next launch starts on the CPU.
