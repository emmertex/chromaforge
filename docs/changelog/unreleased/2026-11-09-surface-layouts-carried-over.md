## Fixes

- **Control-surface layouts carried over:** on start, when Chromaforge's settings folder has no `control-surfaces.json`, it is copied from the first previous name's settings folder that has one, also when the settings folder already existed. An existing file is never overwritten, and a failed copy is logged.
