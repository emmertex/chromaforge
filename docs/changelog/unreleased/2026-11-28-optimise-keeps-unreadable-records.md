## Library

- **Optimise keeps records it couldn't read:** a catalog record the library couldn't decode when it opened was left out, counted only in the log, and deleted for good by the next Optimise (from the menu, a script or the scheduled backup) or by the whole rewrite after another program changed the store. Such records are now said at open (and in `library.info` → `load.unreadable`), Optimise and that rewrite are refused while there are any, and the scheduled backup is made without its optimise.
