## Raw processing

- **Raw decoder per camera:** Settings ▸ Import ▸ Raw decoder chooses Auto, the built-in decoder (lcc-raw) or LibRaw for each camera; `library.preferences` takes `rawDecoder: {camera, decoder}` and reports `rawDecoders` and the `libraw` build. Auto reads with the built-in decoder and lets LibRaw read the variants it can't; LibRaw falls back to the built-in decoder when it fails. Demosaic and development stay in the app. LibRaw is linked into desktop builds made with `LIBRAW_DIR` (or `LIBRAW_SYSTEM=1`); the web and Android builds have none and grey the choice out.
