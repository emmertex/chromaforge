## Fixed
- Video: MPEG-TS clips with interlaced (PAFF) H.264 group their two field access units into one frame sample again, so frame counts, seeking and Capture Frame work as before the FilmCraft access-unit change (issue #727).
## Changed
- Video decoders and muxers pick up FilmCraft's latest upstream work: H.264/HEVC decode hardening (overlapping slices, absurd picture counts, overlapping macroblock slices), AV1 tile module split with cheaper dequantisation and a DC-only inverse DCT, AVI `idx1` recovery, MXF open-window and AES3 fixes, branch-free quantisation and byte-wise CABAC renormalisation in the H.264 encoder, and single-pass VBR/CBR overshoot payback.
