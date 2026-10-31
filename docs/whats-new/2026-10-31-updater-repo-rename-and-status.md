### In-app update check recognizes renamed repository and clarifies error states
- Update checks now recognize releases from both the new repository name and the legacy name when querying GitHub's Releases API, preventing false "no download for this system" errors when release candidate / prerelease checks are enabled.
- The update dialog no longer claims the app is on the newest version when a check fails with an error.
