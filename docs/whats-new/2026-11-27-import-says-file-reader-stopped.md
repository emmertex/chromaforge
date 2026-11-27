### An import says when its file reader stopped
- If the app's file reader stops on an internal error during an import, the files it hadn't read yet are counted as "not imported: the app's file reader stopped; import them again" instead of as not readable: the files are fine, and importing them again reads them.
