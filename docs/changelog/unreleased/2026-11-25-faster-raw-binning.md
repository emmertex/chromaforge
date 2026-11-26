## Performance

- **Faster binning for reduced-size raw development:** one scratch buffer per worker thread, and each colour of a row of blocks summed once when the blocks of the row share a pattern phase, with bit-identical output (#1377).
