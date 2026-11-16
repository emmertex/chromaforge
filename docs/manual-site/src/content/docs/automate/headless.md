---
title: Headless render and snapshots
description: Develop, export and screenshot with no window and no GPU.
sidebar:
  order: 3
---

- **Render**: `<cli> render IN -o OUT` develops one file and writes it with the same encoder as the Export dialog. The format follows the extension (`.jpg`, `.png`, `.tif`, `.webp`, `.avif`, `.jxl`, `.psd`, `.dng`; a video clip exports as `.mp4` / `.mov`).
- **Options**: `--set CONTROL=VALUE` (repeatable), `--settings FILE` (JSON, `.lcpreset` or XMP preset), `--preset ID`, `--size N` (long edge), `--quality Q`, `--trim IN,OUT` for clips.
- **Any export option**: `--opt KEY=VALUE`, e.g. `colorSpace=displayP3`, `bitDepth=16`, `percent=50`, `shortEdge=1080`, `metadata=none`.
- **Snapshot**: `<cli> snapshot --demo --script tour.jsonl -o out.png` runs the whole app UI headless and writes PNGs. Scripts are control-channel requests, one per line.
- Snapshots are CPU-rendered; use them on servers, in CI or when the display is asleep.

```sh
<cli> render in.dng -o out.jpg --set light.exposure=0.5 --set light.contrast=20 --size 2048
<cli> render in.dng -o out.tif --opt colorSpace=displayP3 --opt bitDepth=16
<cli> snapshot --demo --script tour.jsonl -o out.png --size 1600x1000
```

- The manual's own screenshots are made this way; see `docs/testing-sil.md` in the repository for the scripted-UI test suite.
