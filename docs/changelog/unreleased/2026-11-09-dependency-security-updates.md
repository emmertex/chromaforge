## Security

- **Dependency security updates:** `pageant` (used by the web-gallery SSH upload) is updated to 0.2.5, which bounds the frame buffer a peer can make it allocate. The remaining open dependency alerts (`zip` 7 via the pinned `candle`, and `rustls-webpki` 0.102 via `rustls-rustcrypto`) have no pure-Rust fix yet and cannot be reached by the app; they stay documented in `deny.toml`.
