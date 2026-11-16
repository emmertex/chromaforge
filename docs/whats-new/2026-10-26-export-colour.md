### Take your colour to other editors
- Export as DNG now carries the camera profile a photo uses (an imported `.dcp` or a calibrated look), so the DNG
  opens with the same colour in other raw editors.
- New commands export a photo's colour: `camera.exportDcp` writes the camera colour it renders with as a `.dcp`
  camera profile for Lightroom, Camera Raw or RawTherapee, and `develop.exportLut` writes its look (tone, colour,
  grading) as a `.cube` LUT for Capture One, darktable, Photoshop or video tools. Both run from the command-line
  tool or MCP.
