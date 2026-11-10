---
title: Install
description: Get Chromaforge running.
---

- Download from the project's **Releases** page.
- **macOS**: open the `.dmg`, drag the app to Applications. The app is not signed with an Apple developer certificate, so macOS warns first (see below).
- **Windows**: run the installer, or unpack the portable zip.
- **Linux**: AppImage, `.deb`, `.rpm`, Flatpak or a tarball.
- **Android**: install from [Google Play](https://play.google.com/store/apps/details?id=com.emmertex.chromaforge), or sideload the `.apk` from Releases. Play runs a little behind Releases because of Google's review process.
- **Web**: open the browser build; it runs locally, nothing is uploaded.
- Build from source: `cargo xtask install`.
- Updates: the app checks for new releases and tells you.

## macOS: "unknown app" or "may be malware"

The app is not signed with a paid Apple developer account, so Gatekeeper blocks it the first time. If you trust it:

1. Open **System Settings** from the Apple menu.
2. Click **Privacy & Security** in the sidebar.
3. Scroll to the **Security** section.
4. Find the message that the app was blocked and click **Open Anyway**.
5. Enter your administrator password or use Touch ID, then click **Open**.

- Steps vary a little between macOS versions.
- On the newest macOS, if it still won't open, remove the quarantine flag in Terminal, then open it again:

```bash
xattr -c path/to/file.dmg
```

- Search "macos gatekeeper allow app" for Apple's own background.
