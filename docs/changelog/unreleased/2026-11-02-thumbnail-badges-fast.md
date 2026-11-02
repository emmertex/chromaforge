## Performance

- **Faster grid with thumbnail badges:** enabling View Options ▸ Show Thumbnail Badges no longer freezes scrolling, selecting and hovering on large libraries — the badges' sidecar naming was worked out per visible photo, per frame (a pass over the whole catalog each time, ~33 ms at 100,000 photos); the owner table is now built once per catalog change and reused (issue #742).
