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

## Format (version 2)

Version 1 files are still read. Version 2 adds the `natural-cubic-spline` interpolation and the `encode` and `decode`
steps; a version 1 file may not use them.

```json
{
  "format": "base-curve",
  "version": 2,
  "name": "free text",
  "camera": "optional camera model",
  "input":  { "encoding": "linear", "gain": 0.9, "white": 1.0 },
  "output": { "encoding": "display-linear" },
  "extension": "clamp",
  "curves": {
    "a": { "interpolation": "natural-cubic-spline", "points": [[0, 0], [0.1, 0.3], [0.5, 0.75], [1, 1]] }
  },
  "apply": [
    { "op": "curve", "curve": "a", "on": "luminance" },
    { "op": "encode", "kind": "power-segment", "gamma": 2.2, "offset": 0.08 },
    { "op": "decode", "kind": "power-toe", "gamma": 1.8, "knee": 0.008 }
  ],
  "notes": {}
}
```

| Field | Meaning |
|---|---|
| `format`, `version` | `"base-curve"`, `1` or `2`. A higher version is not read. |
| `camera` | Optional free text: the camera model the curve is meant for (`Nikon Z 8`). The selector lists the curves of the photo's camera first (spaces, case and punctuation are ignored when comparing, and the photo's make may come first); rendering ignores it. |
| `input` | How the scene value `v` becomes the first x: `x = gain · v / white`. `v` is the photo's scene-linear luminance relative to the sensor's white (see below). `encoding`: `linear`. `gain`, `white`: numbers above 0 (default 1). A `gain` below 1 is headroom: the sensor's white sits at x = `gain`, and x = 1 is reached only above it. |
| `output` | What the last step returns: `display-linear` (a linear value, 1 = SDR white). |
| `extension` | What happens above x = 1: `clamp` (hold the last value, default), `linear` (continue with the last segment's slope), `rolloff` (close the gap to 1 exponentially at the last segment's slope). |
| `curves` | 1 to 8 named curves (letters, digits, `-` and `_`, up to 32 characters). Each has `interpolation` (`linear`, `monotone-cubic`, or from version 2 `natural-cubic-spline`) and 2 to 256 `points` `[x, y]`: x strictly increasing from 0 to 1, y non-decreasing within 0 to 1. |
| `apply` | 1 to 8 steps run in order; each step's output is the next step's x. |
| `notes` | Free-form; ignored. Unknown fields anywhere are ignored. |

Interpolations:
- `linear`: straight lines between the points.
- `monotone-cubic`: a cubic that never overshoots between points (Fritsch-Carlson tangents).
- `natural-cubic-spline` (version 2): the C2 cubic spline through the points with natural ends (second derivative 0
  at both ends), the result clamped to 0 to 1. It is smooth, but not monotone by construction: with few points and
  sharp bends it can wiggle between them. Two points give a straight line.

Steps:
- `{"op": "curve", "curve": "<name>", "on": "luminance"}`: the curve acts on the colour's luminance; the colour follows by
  ratio (hue and saturation keep their linear ratios).
- `{"op": "curve", "curve": "<name>", "on": "each-channel"}`: the curve acts on each colour channel on its own.
- `{"op": "mix", "a": "<name>", "b": "<name>", "t": 0.5}`: the weighted mean of two curves.
- `{"op": "encode", "kind": "power-segment", "gamma": 2.2, "offset": 0.08}` (version 2): maps x in 0 to 1 (clamped) to an
  encoded value e in 0 to 1: `e = (1 + offset) · x^(1/gamma) − offset` above a knee and a straight line below it, the two
  joined with the same value and slope, so `e(0) = 0` and `e(1) = 1`. `gamma` is above 1 and at most 4; `offset` is 0 to 1
  and below `gamma − 1` (so that the knee lies below 1). Offset 0 is the plain power `x^(1/gamma)`. The knee is
  `(offset / ((1 + offset) · (1 − 1/gamma)))^gamma` and the slope below it `(1 + offset) / gamma · knee^(1/gamma − 1)`.
- `{"op": "decode", "kind": "power-toe", "gamma": 1.8, "knee": 0.008}` (version 2): maps an encoded value e in 0 to 1
  (clamped) to a display-linear one: `e^gamma` from `knee` up, `knee^(gamma − 1) · e` below it (continuous in value).
  `gamma` is 1 to 4, `knee` 0 up to below 1. Knee 0 is the plain power.

Together `curve`, `encode` and `decode` describe a whole neutral transfer, for example a film-like curve followed by
the encoding and decoding that a rendering path applies around it; a file may use any of the steps and any order.

This version of the app renders files whose steps are all `curve` on `luminance`, `encode` and `decode` (each of them
a one-dimensional function of the luminance: the app composes the whole list into the tone table, so there is no
per-pixel cost). A file with an `each-channel` or `mix` step loads and is listed, marked as not supported, and its
photos render with the default base tone and a warning.

## What the curve receives

The first x is `gain · v / white`, where `v` is the photo's scene-linear luminance (Rec. 2020 luminance) in the
tone-map stage: after white balance, the colour model and the baseline exposure, then the Exposure gain, Highlights and
Shadows, Clarity, Dehaze and the local tone steps, and divided by the **white scale of the source's colour model**
(`SourceInfo::white_scale`: what a white-balanced neutral pixel at the sensor's white reads as luminance once the colour
model has run, so a neutral sensor-white pixel is 1.0 whatever gain the matrix, a fitted look or the baseline exposure
carry; a smart preview does not keep it, so the app works it out from the original file when a base curve needs it, and reads 1.0 when it cannot). The base curve replaces the base tone, so the camera look's tone and chroma
curve are not applied; the colour model itself (matrix, hue and saturation tables) still is.

Highlights that the sensor clipped are rebuilt before the colour model (neutral where all channels clipped, at the
brightest white-balanced channel's clip level), so a fully clipped highlight arrives above the sensor's white: with a
`gain` that puts the sensor's white below x = 1, those highlights are at or above x = 1, where a clamped curve holds
its last value (1 for a curve that ends at 1) and the file's transfer gives white.

Contrast, Whites and Blacks act on the whole transfer's output (display values), with no shoulder of their own and
exactly the identity at 0. For an HDR edit the highlights are lifted to the headroom above the curve as for any base
tone: at or below 1.5 EV over grey the HDR render equals the SDR one. Brightness, Levels and the Tone Curve come
after, as for every base tone.

A base curve only replaces the base tone of raw photos; a JPEG or TIFF keeps its display-referred tone.

The evaluation of a file's steps is the file's business: which curve of a camera's rendering a file carries, and with
what `gain`, `encode` and `decode`, is for the person who makes the file to decide; the app applies what the file says.

## Limits

1 MB per file, 8 curves, 256 points per curve, 8 steps. There are no references to other files.
