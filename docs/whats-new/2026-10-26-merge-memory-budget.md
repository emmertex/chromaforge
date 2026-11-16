### Photo Merge checks memory first

- Photo Merge estimates how much memory a merge needs before it reads any photo, and refuses a set that would not fit
  with the estimate and a suggestion, instead of running the computer out of memory.
- Panoramas scale down further when their full size would not fit in memory.
