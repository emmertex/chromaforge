### Point Color inside a mask
- A mask can carry its own Point Color samples: they change the sampled colours only inside the mask, weighted by
  it, on the CPU and the GPU alike. Under a mask that covers the whole photo they render exactly as the photo's own
  Point Color. Agents: `mask.pointColorPick`, `mask.pointColorSet`, `mask.pointColorDelete`. Thanks @OneSlips.
