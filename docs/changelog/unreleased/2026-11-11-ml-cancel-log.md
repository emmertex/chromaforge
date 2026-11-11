## Fixes

- **Cancelling a large AI batch froze the app for a moment:** stopping tens of thousands of waiting AI jobs (the ✕ on the AI activity row, or Cancel all) wrote one log line per job while the UI waited, 48,946 lines and a half-second freeze in one report (#979). It now writes one line.
