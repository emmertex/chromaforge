## Library

- **Capture dates of a year or a month alone:** a photo dated only `1983` or `1983-04` (a scanned print's
  `photoshop:DateCreated`) keeps that date as stated instead of failing a Lightroom catalog import: it shows as "1983" or
  "April 1983", sorts and groups at the start of its period, can be typed in Edit Capture Time, and is written to XMP
  `photoshop:DateCreated` only. A Lightroom capture time that isn't a date imports the photo without it, with a warning.
