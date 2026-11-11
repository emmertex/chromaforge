## Fixes

- **Background preparation:** a build of the previews that did not run to its end is owed to the library (`prepUnfinished` in its preferences) and is taken up again for the view shown once it has been still for 3 seconds, from the grid and after a restart; switching previews off and on again no longer counts as the user stopping the build, and a build left running by the library before is stopped when another library opens (upstream #4093, issue #3947).
