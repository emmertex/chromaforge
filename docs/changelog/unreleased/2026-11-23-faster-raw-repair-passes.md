## Performance

- **Faster raw repair passes:** the candidate scan of Remove Hot Pixels and the quick test of Fix Hot and Dead Pixels read neighbours at fixed offsets and skip the median where the excess cannot pass, about 3.5 times faster per photo with identical output (#1377). A review of the change also made the black-level spread read exactly what the correction reads, so short per-position black-level arrays are handled the same way in both.
