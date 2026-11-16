### Even output sharpening in every format
- Floating-point exports (32-bit TIFF, EXR) are now sharpened as much as 8- and 16-bit ones, instead of more in the shadows and less in the highlights.
- Output sharpening weighs red, green and blue by the export's own colour space (Display P3, Adobe RGB, ProPhoto RGB, Rec.2020), so edges in saturated colours get the right amount.
