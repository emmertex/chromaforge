### Culling stays responsive

- Going through photos no longer freezes while the background prepares them: entering a folder no longer freezes for over a second, the preparation renders on its own lowest-priority threads, and the loupe waits up to about a second for a preview it is already making instead of decoding the same raw twice.
- The loupe's first picture of a raw is its small camera JPEG before the full-size render, so the slowest first picture drops from about 1.3 s to about 0.4 s, and holding a step key makes thumbnails further ahead (30 photos instead of 5).
