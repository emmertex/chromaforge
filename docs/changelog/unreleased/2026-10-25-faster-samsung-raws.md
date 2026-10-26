## Performance

- **Samsung compressed raws decode 3x faster:** finding the colour-filter layout from the camera's embedded JPEG converted every preview pixel with a power function; the 256 byte values are now converted once into a table, with the same result. NX500 raw decode 1434 → 406 ms. Thanks @Kaan88. (fork #503)
