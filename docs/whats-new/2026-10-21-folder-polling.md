### Notice photos other apps add to your folders

- Settings ▸ Library ▸ Folder Polling: {app} can check the library's folders for new photos when the library opens, when you come back to it and every few minutes. It is off by default, since listing folders costs time on a VPN or a slow network.
- Only folders whose contents changed are scanned again, so a large library stays quick to check.
- New photos are shown as “N new” on the folder in Folders, with a notice; right-click the folder ▸ Add New Photos… to add them. Or choose Add automatically to have them added at once.
- The command-line tool's `sync-folder FOLDER` and `open-url` synchronize a folder from a script, in the running app (`--connect`) or on a library; starting {app} with a `sync` link opens Synchronize Folder for that folder (issue #123).
