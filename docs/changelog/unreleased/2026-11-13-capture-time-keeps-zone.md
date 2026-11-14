## Library

- **Edited capture times keep their time zone:** Edit Capture Time (`photo.setCaptureTime`) keeps a UTC offset typed with the time (`…+01:00`) instead of dropping it, so the corrected time exports with `OffsetTimeOriginal`; one photo gets the time as typed.
