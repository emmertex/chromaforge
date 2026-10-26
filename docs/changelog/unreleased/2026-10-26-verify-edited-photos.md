## Colour

- **Migration check for edited photos:** `calibrate-lightroom verify --raws DIR… --exports DIR…` renders edited raws through the app's own import (XMP sidecar, or the `crs:` settings embedded in the export) and reports CIEDE2000 against each edited export, per photo and per group of controls used, with `--report` (JSON and a ΔE00 map per photo) and `--json`. (no PR)
