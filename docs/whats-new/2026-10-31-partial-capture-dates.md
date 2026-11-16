### A capture date of just a year or a month is kept as stated
- A photo dated only to the year (`1983`) or the month (`1983-04`), as a scanned print often is, keeps that date: a
  Lightroom catalog with such a date imports instead of failing, Edit Capture Time takes it (typed with several photos
  selected, each of them gets it) and starts from it, and the Metadata panel, the grid, the info overlay and the
  import window show "1983" or "April 1983". A shift or time-zone change leaves such a photo as it is and says it has
  no time to shift.
- It sorts, groups and matches date filters and smart-album rules as the start of its period, and copies, names and
  publishes files by it. Saved to XMP it goes in `photoshop:DateCreated` as stated, with no `exif:DateTimeOriginal`
  (whose form can't say a year alone; an older one in the sidecar is removed), and it reads back the same.
  Auto-stacking, burst detection and track-log tagging leave such photos out, since they have no time of day.
- A Lightroom capture time that isn't a date at all no longer fails the import: the photo comes in without it, with a
  warning naming the image.
