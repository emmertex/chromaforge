---
title: Get help with a problem
description: What to include when you ask for help with a Chromaforge bug, and where to post it.
order: 30
verified: 0.7.1
---

Open an issue in the [issue tracker](https://github.com/emmertex/chromaforge/issues). Include your version, your system, what you did, and what you expected.

## What to include

1. **Your version.** Choose **Help** > **About Chromaforge** (on macOS, **Chromaforge** > **About Chromaforge**).
2. **Your system.** For example Windows 11, 64-bit. **Help** > **System Info…** shows details about your computer.
3. **The steps.** What you clicked, in order, and what happened.
4. **The camera and file type**, if the problem is with a photo.
5. **A sample file**, if a raw or other photo renders wrong. Maintainers ask for the original file, not an export. A link to a shared folder works. Uploading raws to [raw.pixls.us](https://raw.pixls.us) also helps, because every file there becomes a permanent test sample for that camera.
6. **A screenshot** of Chromaforge next to the camera JPEG or the other app, for colour problems.
7. **The log file.** Choose **Help** > **Open Log Folder** and attach `lightcraft-classic.log`.

## Where the log is

Chromaforge writes its log to `logs/lightcraft-classic.log` in its settings folder. On Windows that is `%APPDATA%\LightCraft Classic\logs\`. Each start moves the previous log to `lightcraft-classic.1.log`, and that one to `lightcraft-classic.2.log`, so the log of a run that crashed survives the next start.

## Still stuck?

Open an issue in the [issue tracker](https://github.com/emmertex/chromaforge/issues).
