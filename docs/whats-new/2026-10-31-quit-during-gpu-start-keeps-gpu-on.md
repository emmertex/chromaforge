### Quitting while the GPU starts no longer turns GPU rendering off
- Closing the app a few seconds after launch, before the graphics driver had finished starting, used to make the next launch report a crash and switch GPU rendering off. An orderly quit now clears the start-up marker, so the GPU stays on; a real crash inside the driver is still reported.
