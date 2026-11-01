### Masks with an id at the 32-bit limit
- A photo with a mask id at the 32-bit limit (from a damaged or hand-edited sidecar, a paste or an agent) no longer
  breaks the masks added after it: new masks, duplicates and the masks of a preset each get an id no other mask has.
  Before, the next id wrapped around to 0, so a second new mask shared the first one's id (debug builds crashed).