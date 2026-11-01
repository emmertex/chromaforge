## Fixed
- Malformed capture times (one-digit UTC offset minutes or ISO clock fields, impossible UTC offsets in date rules, an IPTC Time Created with trailing text) are refused instead of being read as a nearby valid time, so a valid fallback applies.
- Shutter fractions with a zero or negative part, a fractional XMP orientation and a `.cube` file with a non-numeric value are refused instead of being half-read.
- RW2 files read ISO and other missing Exif from their embedded JPEG; D1X files with square resolution tags take their pixel shape from the preview.
- Decomposed Korean and Vietnamese text now draws whole in exports; Settings > Performance no longer waits for the preview cache's size; adding a gradient to a mask disarms an armed brush, object tool or eyedropper.
