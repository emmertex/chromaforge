### Smoother HDR on Wayland with NVIDIA
- With HDR output on a Wayland desktop and an NVIDIA GPU, the window now presents with Mailbox where the driver offers it, so frames rendered at low GPU clocks no longer wait a whole extra refresh (about 95 instead of 144 fps on a 144 Hz display). Other systems keep vsync; `{env}_PRESENT=fifo` restores it everywhere.
