### Safer reading of damaged image files

- A small damaged TIFF, PNG or JPEG XL file can no longer make the app try to allocate gigabytes of memory: decodes have a byte budget, TIFFs with an absurd number of samples a pixel are refused, and JPEG XL sizes are read without rendering the picture.
- Images larger than 512 megapixels (the largest Lightroom Classic opens) are refused with a message instead of risking running out of memory. Based on work from the upstream project.
