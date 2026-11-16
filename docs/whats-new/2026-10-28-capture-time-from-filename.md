### Capture time from file names
- Edit Capture Time has a File name mode: each selected photo gets the date and time its file name states (`IMG_20260921_143000.jpg`, `2026-09-21 14.30.00.heic`, phone and WhatsApp names), and a time-zone shift can move them together. Photos whose name holds no date are left as they are and counted.
- Agents and scripts get it as `photo.setCaptureTime {fromFilename: true}`, with `dryRun` to see the new times before anything changes.
