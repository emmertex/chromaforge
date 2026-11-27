### Large imports are saved in pieces
- A step as large as a Lightroom catalog import is written to the library's log as many records of about 4 MiB instead of one of gigabytes, so saving it takes about 5 KB less memory per photo and a crash while it is saved keeps what was already written (#1361).
