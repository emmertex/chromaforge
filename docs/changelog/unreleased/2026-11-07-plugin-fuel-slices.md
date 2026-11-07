## Fixes

- **Plug-in calls no longer fail at a fuel slice:** plug-in modules are compiled when loaded instead of function by function as they first run. wasmi charged a function's compilation to the call's fuel and couldn't resume an out-of-fuel during one, so a long call failed with "instruction budget" whenever a first run fell at the end of one of the host's fuel slices. Loading costs 1.2 to 3.4 ms more per plug-in (108 to 296 KB modules); calls cost nothing more.
