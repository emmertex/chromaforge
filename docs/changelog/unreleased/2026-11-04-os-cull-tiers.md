## Performance

- **Camera previews first while culling:** the embedded camera JPEG is the first picture of every raw in the grid, filmstrip and loupe (a stand-in for edited raws); stand-ins get two render workers of their own, background thumbnail renders run at most two at once, and background work waits while a picture on screen is missing. Ported from @OneSlips (PERF-CULL-TIERS). (no PR)
