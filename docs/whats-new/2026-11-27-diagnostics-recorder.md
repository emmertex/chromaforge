### A diagnostics recorder for hangs during imports
- Off by default. When turned on with an environment variable, {app} keeps a record of an import's steps in a folder next to its settings; after a hang or a forced quit, it says which step was running and how far it had got.
- It records step names, counts and timings only, never file names or paths, and sends nothing anywhere (issue #1363).
