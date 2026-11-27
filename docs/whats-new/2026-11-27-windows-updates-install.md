### Windows updates install again
- Installing an update on a Windows copy set up with the installer works again: the app used to quit and nothing was installed, so the next start was still the old version (#1549).
- If the installer fails or you decline Windows' permission prompt, the app's log says why on the next start, and the installer's own log is kept beside the download (`update-install.log`).
- **Help ▸ Install Update and Relaunch** is gone; install from **Help ▸ Check for Updates…**, whose **Install and Relaunch** button does the same.
