## Library

- **Immich locked folders:** photos in Immich's locked folders can now be synced, imported and linked. The account card's Locked Folders section shows whether the app's server session is elevated, and Unlock… sends the account's PIN (4–6 digits, as `pinCode`) or password once (`immich.unlockSession`, never journaled); the unlock holds until Immich locks the session again. Sync no longer skips `locked` assets (they are simply absent from a locked session's listings). Issue #1023.
