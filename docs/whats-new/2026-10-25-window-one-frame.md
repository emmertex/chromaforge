### Drags reach the screen a frame sooner
- The window queues one frame ahead of the display again instead of two, as eframe does by default (since #136 it
  used egui-wgpu's high-throughput setting). Sliders, crop handles, brushes and every other drag show up one display
  frame sooner: 8.3 ms at 120 Hz, 16.7 ms at 60 Hz. On a MacBook Pro at 120 Hz, a heavy develop drag went from
  about 18 ms to about 10 ms between a frame being handed to macOS and macOS compositing it, with the same 120 frames
  a second. On a Mac the window also holds one drawable less: its width × height × 4 bytes, 15–22 MB for a
  1200–1500-point-wide window on a Retina display.
