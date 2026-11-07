## Performance

- **AI Denoise on both graphics cards:** with a second graphics card, the built-in denoiser and installed models keep a device on each card, and a batch makes one photo on each card at once (the second card first; one photo at a time while you work). Each card is checked against the processor and given up on alone; a tile for the card for editing waits while you touch the interface. `denoise.status` lists both cards (`device.cards`) and both photos (`running.photos`), and each photo has its row under the AI Denoise batch row. Ported from OneSlips' dual-GPU work.
