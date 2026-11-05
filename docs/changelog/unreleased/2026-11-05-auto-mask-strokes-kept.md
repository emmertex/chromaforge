## Performance

- **Auto Mask strokes are kept and refined only where they reach:** each stroke's edge snap runs over the part of
  the photo it can touch, finished strokes are kept between renders on the GPU and the CPU, and the GPU works out
  one stroke at a time so the window keeps drawing. Twelve big strokes at 100 % of a 45 MP photo: the zoomed
  view's tiles finish in 9-10 s instead of 15 s, a render back from Fit with no stage cache takes 0.19-0.25 s
  instead of 0.9-1.3 s on the GPU, and the CPU takes 0.5 s instead of 11.7 s. Thanks @OneSlips. PR #n.
