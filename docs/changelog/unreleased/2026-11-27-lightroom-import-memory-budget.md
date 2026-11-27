## Library

- **Lightroom import:** no more one-million-row limit per table; the read keeps within a memory budget taken from the computer (`<PREFIX>_LIGHTROOM_IMPORT_MEMORY` to change it), never skips a table for lack of memory, and keeps about half what it did (#1361).
