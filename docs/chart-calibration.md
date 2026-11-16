# Calibrating a camera with a colour chart

`<cli> calibrate-chart` measures a camera's colour from raw photos of an **X-Rite ColorChecker Classic** (the 24-patch
chart, also sold as Calibrite ColorChecker Classic). The result is measured colour for that camera model. The app
then uses it before any estimate: the camera-JPEG fit, the spectral table and dcraw's matrices.

## Shooting the chart

- Light the chart evenly, with no glare: daylight from the side, or one lamp at 45°. Fill most of the frame and keep
  the chart square on.
- Expose so the white patch is bright but not clipped. The tool refuses a shot whose grey patches clip, and leaves
  out any other clipped patch.
- One shot in daylight is enough for a usable profile. A second one under tungsten (incandescent or halogen) gives
  the camera's colour at both ends of the white-balance range, as a DNG profile has.
- Shoot raw. Any white balance setting works: the tool reads the chart's grey patches itself.

## Running it

Open each shot in any viewer and note where the centres of the chart's **four corner patches** are, as fractions of
the picture's width and height. For example, a corner patch 1200 px from the left of a 6000 px wide photo is at
`0.2`. Give them in any order round the chart; the tool works out which corner is which.

```
<cli> calibrate-chart \
  --shot DSC_0001.NEF --corners "0.21,0.30 0.78,0.29 0.79,0.70 0.20,0.71" --light daylight \
  --shot DSC_0002.NEF --corners "0.22,0.31 0.77,0.30 0.78,0.69 0.21,0.70" --light tungsten \
  --preview chart-check --dcp my-camera.dcp
```

- `--light`: `daylight` (D55, the default), `d65`, `d50`, `shade` (D75), `tungsten` (Illuminant A) or a temperature
  in kelvin (`3200`). The matrices are filed under the nearest DNG standard light.
- `--chart 2005`: a chart made before November 2014. X-Rite changed the pigments then, and the default reference
  values are the current ones.
- `--preview DIR`: one picture per shot with the box read in each patch. A red box is a clipped patch. Check that
  every box sits inside its patch.
- `--dcp FILE`: the same colour as a camera profile, to try at once (Camera ▸ Import DCP, then choose it per
  photo).
- `--report FILE.json`: the full report.

The tool prints the colour error (CIEDE2000) of every patch, before (the colour the app had for the camera) and
after. It then prints a camera database entry: a few lines of TOML with the camera's make, model, colour and
forward matrices for each light, and where they came from.

## Adding it to the app

Put the printed entry into `crates/camdb/data/cameras/<maker>.toml`, or send it in an issue to be added. Its
provenance is `chart`, which ranks it as measured data for that model. It gives raws without matrices of their own
(every vendor format) their colour and their Kelvin white balance.

## What it measures, and what it doesn't

- For each light, the tool finds a 3 × 3 forward matrix, from white-balanced camera RGB to XYZ D50, that keeps white
  exactly. It fits the 23 usable patches (black is too noisy) by least squares, then by Levenberg–Marquardt on
  L*a*b* error, refining the red and blue balance at the same time, since the chart's greys aren't perfectly
  neutral. The colour matrix then follows exactly from the light's white and the camera's measured neutral.
- 24 patches support a matrix, not a hue/saturation table. The result is colorimetric: it reproduces the chart, not
  a camera maker's or an editor's look. For an editor's look, see [calibrate-lightroom](calibrate-lightroom.md).
- Uneven light and glare show up as error on the grey patches, so check the report.

Reference values: X-Rite's 2016 ColorChecker Classic data (CIE L*a*b*, ICC D50) for charts made before and after
November 2014, as collected in colour-science (BSD-3-Clause). Sources: own design; DNG specification 1.7 chapter 6;
CIEDE2000 (Sharma, Wu, Dalal 2005).
