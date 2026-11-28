## Import

- **An import says when its file reader stopped:** files the I/O pool never started because its scheduler stopped (an internal error) are counted as "not imported: the app's file reader stopped; import them again" instead of "not readable"; `library.import` lists them in `interrupted` with `why`: `schedulerStopped` (besides `failed`).
