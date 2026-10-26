## Colour

- **Camera matrices for about 550 more models:** dcraw's `adobe_coeff` table (free for all uses) ships in the camera database as a lower-priority source. A raw without colour data of its own gets it after a failed or missing camera-JPEG fit, instead of the neutral fallback, and Calibrated camera colour can start from it for models without spectral data. Measured and spectral data still come first. (no PR)
