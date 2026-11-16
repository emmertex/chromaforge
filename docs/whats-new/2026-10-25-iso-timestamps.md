### Incomplete timestamps no longer become plausible photo times
- An ISO date ending in `T`, a clock with extra components, or a malformed fractional second is no longer read as a
  different valid time. XMP keeps the original property available and can use the next valid capture-time field;
  GPX treats an invalid point time as untimed, so it cannot tag a photo at a fabricated time.
