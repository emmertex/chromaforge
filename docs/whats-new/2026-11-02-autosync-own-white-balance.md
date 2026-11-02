### Auto Sync applies each selected photo's own white balance
- With Auto Sync on, As Shot and Auto now apply to each selected photo's own white balance: As Shot gives every photo its own camera white, and Auto works out each photo's white on its own pixels, instead of copying the active photo's numbers.
- A typed or dragged Temp or Tint sets the same Kelvin on every photo edited in Kelvin while each photo keeps its own tint; a photo on another scale (a JPEG beside a raw) moves by as much as the active photo did, instead of taking the raw's Kelvin read on its own scale.
- The other photos' pixels for Auto are decoded in parallel, a core's worth at a time, and each photo's white is worked out before its decode can be pushed out of the cache.
