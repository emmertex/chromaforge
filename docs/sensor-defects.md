# Sensor defects: hot, dead and stuck pixels, and sensor dust

> **Last updated:** 2026-10-14 · **Change:** new (LR-PIX-MAP, LR-DUST-AUTO)

Chromaforge fixes two things a camera's sensor leaves on photos, and remembers both per camera body:

- **Hot, dead and stuck pixels** — single photosites that read too bright (hot: grows with exposure time and heat),
  always dark (dead) or always at full scale (stuck). Fixed automatically on the raw data, before demosaic.
- **Sensor dust** — soft, low-contrast, roughly round shadows from dust on the sensor's cover glass. Most visible on
  smooth areas (sky, walls, backdrops) at small apertures. Removed with ordinary heal spots on request
  (**Remove Dust**), or after every import.

## Hot, dead and stuck pixels

**On by default, for raw photos.** Every decode (thumbnails, previews, 1:1, export, CLI, MCP) corrects

1. the camera body's **confirmed** pixels (its map, below), and
2. the clear, isolated outliers **this photo** shows.

A corrected sample is replaced by the median of its nearest same-colour neighbours on the mosaic (Bayer, X-Trans;
per channel on linear DNGs; the 8 neighbours on monochrome data), so previews and exports match.

**Turning it off:** Edit ▸ Detail ▸ Sensor ▸ *Fix hot and dead pixels*, or the develop control `sensor.pixelFix`
(0/1). It is kept in the photo's settings (and its XMP sidecar, `lc:settings`), and the photo is decoded again as the
camera recorded it.

### How one photo is judged (`crates/raw/src/defects.rs`)

On black-subtracted values (1 = white), each sample is compared with the up to 8 nearest samples of its colour:

| | condition (photo correction / map learning) |
|---|---|
| hot | above the brightest neighbour by > max(12 σ, 0.35 × their median, 0.02) / max(7 σ, 0.25 × median, 0.008) |
| stuck | hot at the clip level while the neighbours sit below 0.6 (no median term) |
| dead | neighbours lit (median > 0.08 / 0.05) and the sample below 0.15 / 0.25 of their median |

σ is the photo's own noise per colour (median absolute difference of same-colour samples along rows), raised on
texture by the neighbours' spread, so thresholds follow ISO and exposure. Then two guards keep real light:

- **Isolation:** the 8 adjacent samples are compared with the background 3–4 pixels away. A star, a catchlight, a
  glint or a small highlight lifts them (optics, low-pass filter, diffraction); a hot pixel doesn't. Two adjacent
  defects (a pair) are both accepted.
- **Context:** glints of sun on sand or water, glitter and sequins make fields of single-pixel sparkles; a candidate
  with another sample standing out by a third as much within 16 pixels is left alone.

More than 20 000 candidates in one photo (50 000 for learning) means the detector is reading the scene; nothing is
corrected or learnt from that photo.

### Camera body maps (`crates/engine/src/sensor_maps.rs`)

- **Body:** the body serial number in the file; else `file:XX`, the first two characters of the file name after any
  `_` (`_71_1377.CR3` → `file:71`). A map also records the camera model and raw size and is used only for files that
  match. (The same body rule as camera profiles per body.)
- **Learning:** every decode records the photo's candidates once (a shot fingerprint: samples, capture time,
  exposure, ISO). A pixel found in **3 photos from 3 different 5-minute periods** is **confirmed** and from then on
  corrected on every photo of the body — including the faint ones a single photo would not touch. A burst or a
  tripod series of one scene can never confirm a highlight.
- **Growth and expiry:** sensors develop new hot pixels, so the map keeps growing. A one-off candidate not seen again
  in 300 analysed photos is dropped. A confirmed pixel is missed only in photos taken at an exposure time and ISO
  inside the range it was seen at; after `max(150, 20 × its hits)` such misses it expires (remapped by a camera
  service, or recovered). Each entry keeps the exposure and ISO range and first/last capture time.
- **Storage:** one JSON file per body in the settings folder, `<config>/sensor-maps/<body>.json`
  (`<PREFIX>_SENSOR_MAPS` overrides the folder). Settings, not the catalog: a sensor belongs to a camera body, not a
  library — the map survives new libraries, re-imports and removed folders, and one body shooting into several
  libraries builds one map. Damaged files are renamed `.bad` and a new map starts. No image content is stored.
- **Commands:** `camera.pixelMap.status {body?|"all", list?}`, `camera.pixelMap.rebuild {body?, ids?, clear?, wait?}`
  (reads the photos in the background, activity row *Mapping hot pixels*), `camera.pixelMap.learn`,
  `camera.pixelMap.clear {body}`, `photo.pixelDefects {id?}` (what a photo's decode corrects, read-only).
  Photo ▸ Camera Sensor ▸ Rebuild Hot Pixel Map.

### Dark frames (`crates/raw/src/darkframe.rs`)

A raw shot with the lens cap on (and the viewfinder covered), at the exposure and ISO you use, shows every hot
pixel at once. `camera.pixelMap.calibrate {paths: [...], body?}` measures each frame and adds its hot pixels to the
body's map as confirmed (they never expire by misses; `clear` removes them). It refuses a frame that isn't dark.

Each frame's record is kept as `<config>/sensor-maps/dark-frames/<body>-<time>-iso<N>.json` for other tools (dark-frame
noise calibration for AI denoise can read the same figures). Format version 1 (`DarkFrame`, camelCase JSON):

| field | meaning |
|---|---|
| `version` | 1 |
| `make`, `model`, `serial` | camera, from the file |
| `sensor` `[w, h]`, `active` `[x, y, w, h]`, `cfa` | data size the hot list's coordinates are in, active area, CFA layout name |
| `iso`, `exposure` (s), `captured` (ISO 8601) | capture settings |
| `range` | white − black in stored units (to turn the normalised figures back into raw numbers) |
| `offset` `[r, g, b]` | median level above the file's black level (the true black offset) |
| `noise` `[r, g, b]` | read noise sigma (1.4826 × median absolute deviation) |
| `rowNoise`, `columnNoise` | sigma of the row and column means (green): banding |
| `hot` `[{x, y, s, kind, level}]` | samples above `offset + max(10 σ, 0.004)`, strongest first (`kind`: hot / stuck) |

All levels are normalised: 0 = the file's black level, 1 = its white level.

## Sensor dust

**Remove Dust** (Remove panel; Photo ▸ Camera Sensor ▸ Remove Dust; `spot.findDust`) on the active photo, or on a
selection in the background (activity row *Removing dust*):

1. The photo is rendered uncropped (≈1600 px) without its earlier auto-dust spots, and the dust detector
   (`crates/pipeline/src/dust.rs`) looks for small, soft, round darkenings on smooth areas, with a confidence (smooth
   surroundings, a clear dip, a round blob).
2. The camera body's **dust map** adds its spots for this photo's capture time and aperture — each re-checked on this
   photo: only where the place is smooth (never on subject detail); a spot seen only twice must show here.
3. Each spot becomes an **Even Out** spot tagged *auto dust* (a flat-field lift: the spot keeps its own pixels and
   is divided by the shadow's estimated transmission — sand grains, pebbles and grain under the shadow stay as they
   were), or a **Heal** spot when evening out would leave it more than three times further from its surroundings
   than healing (both are tried on the detection image). Healing copies other pixels in: on the real files it brought
   a pebble into a dust spot on wet sand, which is why Even Out is the default. A spot covers 2.8 sigma of the
   shadow, its soft edge included.
4. One undo step per photo. Running it again replaces the earlier auto-dust spots; a spot you move or resize becomes
   yours and stays.

In the Remove tool, auto-dust spots are outlined **amber** (the camera's known dust) or **yellow** (found in this
photo alone); with Visualize Spots on they are drawn bolder, to review them against the high-pass view.

### Dust maps per body

- Positions are kept in **sensor coordinates**: the developed area before the file's orientation and the user's
  rotation, 0..1. A portrait shot or a photo turned in Chromaforge gets its spots at the right place; crops never
  matter (spots live in the uncropped frame). Lens-correction warps are not undone (a few pixels at most).
- **Confirmed** after 3 photos over 2 different 5-minute periods within one time window.
- **Time windows:** dust moves when lenses change or the sensor cleans itself. A spot's window extends while it keeps
  being seen; a photo where the place is *testable* (smooth, bright, aperture at least as small as the widest it was
  seen at) and it is *not* there counts a miss, and 3 misses close the window. A later sighting opens a new window.
  A photo is covered by a window when it was taken between its first and last sighting (±1 hour), or after the last
  window while nothing has shown the dust gone.
- **Aperture:** each spot keeps its radius and darkening per f-number (⅓ stop). At an aperture it wasn't seen at, the
  nearest one is scaled: wider apertures blur the shadow wider and fainter (radius ∝ √(N_ref / N)).
- **Learning:** every Remove Dust teaches the map; `camera.dustMap.rebuild {body?, ids?, wait?}` (activity row *Mapping
  sensor dust*) and `camera.dustMap.learn` go through photos oldest first. `camera.dustMap.status {body?, list?}`,
  `camera.dustMap.clear {body}`.
- **Remove dust on import:** Settings ▸ Import. Off by default: a map from a few photos can still suggest spots that
  aren't there on a new card after a sensor clean, and an import should never change photos unasked. Turn it on once
  a body's map is built.

### Compared with other editors (behaviour only)

- Lightroom fixes hot pixels silently with no switch and no memory; dust is manual (Spot Removal with Visualize Spots)
  and syncing spots copies positions blindly to other photos.
- Capture One's LCC (a reference shot through a diffuser per lens and aperture) removes dust by flat-field division,
  exactly but only with a calibration shot; it also has single-pixel noise sliders.
- Chromaforge learns both maps from ordinary photos (or a dark frame), re-checks every remembered dust spot on each
  photo instead of copying it, and offers a flat-field style removal per spot.

## Limits

- Detection costs about 0.1–0.3 s per 45 MP decode on a laptop CPU, on top of a ~1.2 s decode.
- A sensor without a low-pass filter can record a very faint, very sharp star on one sample; such a star may be
  learnt as a candidate (never confirmed unless it sits in the same place in photos of different scenes).
- The AI-denoised picture (`crates/engine/src/denoise.rs`) is made from a corrected mosaic whatever the photo's
  switch says (it is made once per photo).
- Dust detection works on the rendered photo: very dark or very textured places can't tell (those spots are skipped).
