### AI search uses the graphics card on every system

- AI semantic search now runs its CLIP model on the graphics card on Linux and Windows as well as macOS (Vulkan, DirectX 12 or Metal), with the same code everywhere. Each model is checked against the processor's result before it is used, and the processor takes over on any GPU error.
