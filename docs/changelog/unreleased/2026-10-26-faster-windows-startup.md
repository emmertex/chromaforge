## Performance

- **Windows starts in half a second again:** the GPU histogram kernel no longer asks DX12 for zeroed workgroup memory
  it fills itself, which FXC compiled for about 8 seconds at every launch, and the window no longer waits for the
  GPU kernels to compile before it opens. (no PR)
