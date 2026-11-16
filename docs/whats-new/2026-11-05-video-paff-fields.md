### Video fixes
- Clips with interlaced H.264 video in MPEG-TS files (many camcorder recordings, `.mts` / `.m2ts`) group their fields into one frame again, so they open with the right length, seek correctly and Capture Frame saves the frame you see.
- The video decoders pick up the latest FilmCraft hardening, so a malformed video file is rejected with an error instead of misbehaving.
