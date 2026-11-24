# Base curve files

A **base curve** replaces the app's base tone (the camera look's tone, or the generic raw curve) for a raw photo
that selects it. Nothing is bundled with the app: base curves are plain JSON files that you put in a folder, and the
Base curve selector of the Profile section lists what it finds there.

This is not the Tone Curve tool. The Tone Curve is a point curve you edit on display values after the base tone; a
base curve is the base tone itself.

## Where the files go

- The folder `base-curves` inside the settings folder (`%APPDATA%\<app>\base-curves` on Windows;
  `<exe folder>\<app>Data\base-curves` in a portable install), or the folder named by `<PREFIX>_BASE_CURVES`.
- One curve per file, `<name>.json`. The photo stores the id `base:<name>` (the file name without `.json`).
- Files over 1 MB, files that are not valid, and files with a newer format version are ignored with a warning. A
  photo whose selected curve is not in the folder keeps the id, renders with the default base tone and shows a
  warning (also on export); the curve is used again as soon as the file is back.
- A curve file's *content* is part of the photo's settings hash: replacing the file refreshes previews.

## Format (version 1)

```json
{
  "format": "base-curve",
  "version": 1,
  "name": "free text",
  "input":  { "encoding": "linear", "gain": 1.0, "white": 1.0 },
  "output": { "encoding": "display-linear" },
  "extension": "clamp",
  "curves": {
    "a": { "interpolation": "monotone-cubic", "points": [[0, 0], [0.1, 0.3], [1, 1]] }
  },
  "apply": [ { "op": "curve", "curve": "a", "on": "luminance" } ],
  "notes": {}
}
```

| Field | Meaning |
|---|---|
| `format`, `version` | `"base-curve"`, `1`. A higher version is not read. |
| `input` | How the scene value `v` becomes the curve's x: `x = gain · v / white`. `v` is the photo's scene-linear luminance relative to the sensor's white (see below). `encoding`: `linear`. `gain`, `white`: numbers above 0 (default 1). |
| `output` | What the curve returns: `display-linear` (a linear value, 1 = SDR white). |
| `extension` | What happens above x = 1: `clamp` (hold the last value, default), `linear` (continue with the last segment's slope), `rolloff` (close the gap to 1 exponentially at the last segment's slope). |
| `curves` | 1 to 8 named curves (letters, digits, `-` and `_`, up to 32 characters). Each has `interpolation` (`linear` or `monotone-cubic`) and 2 to 256 `points` `[x, y]`: x strictly increasing from 0 to 1, y non-decreasing within 0 to 1. |
| `apply` | 1 to 8 steps run in order; each step's output is the next step's x. |
| `notes` | Free-form; ignored. Unknown fields anywhere are ignored. |

Steps:
- `{"op": "curve", "curve": "<name>", "on": "luminance"}`: the curve acts on the colour's luminance; the colour follows by
  ratio (hue and saturation keep their linear ratios).
- `{"op": "curve", "curve": "<name>", "on": "each-channel"}`: the curve acts on each colour channel on its own.
- `{"op": "mix", "a": "<name>", "b": "<name>", "t": 0.5}`: the weighted mean of two curves.

This version of the app renders files whose steps are all `curve` on `luminance`. A file with an `each-channel` or
`mix` step loads and is listed, marked as not supported, and its photos render with the default base tone and a
warning.

## What the curve receives

The scene value `v` is the photo's scene-linear luminance after the Exposure gain and the local tone steps, divided by
the white scale of the source's colour model, so that a neutral sensor-white pixel is 1.0 (a clipped sensor
channel, white-balanced, reads 1.0 on the neutral axis). Contrast, Whites and Blacks act on the curve's output (display
values), and Brightness, Levels and the Tone Curve come after, as for every base tone.

## Limits

1 MB per file, 8 curves, 256 points per curve, 8 steps. There are no references to other files.
