## Fixes

- **Windows updates:** installing an update on an MSI-installed copy quit the app without running the installer, so the old version came back (#1549). The installer now runs, the app waits for it and restarts only when it succeeded; a failed or declined install is logged at the next start, with the installer's log kept beside the download. The always-greyed **Help ▸ Install Update and Relaunch** item was removed; the Updates dialog's button installs.
