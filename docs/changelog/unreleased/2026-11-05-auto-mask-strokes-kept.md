## Performance

- **Auto Mask strokes are kept and refined only where they reach:** each stroke's edge snap runs over the part of
  the photo it can touch, and finished strokes are kept between renders on the GPU and the CPU. On the GPU they
  are kept in blocks of the whole photo, so the tiles of the zoomed view share them. The GPU also works out one
  stroke at a time, so the window keeps drawing. Twelve big strokes at 100 % of a 45 MP photo: the zoomed view's
  tiles finish in 6.4 s instead of 15 s, and in 4.4 s instead of 15 s after going back from Fit. On the CPU, a
  render back from Fit takes 0.5 s instead of 11.7 s. Thanks @OneSlips. PR #824.
