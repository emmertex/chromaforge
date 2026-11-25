## Fixes

- **Dialog sections stay closed:** a dialog body laid out directly while it fits and in a scroll area once it doesn't gave its widgets a separate state in each layout, so closing a section that took the body from too tall to short enough switched layouts, found the section open again and alternated on every frame, dropping clicks (What's New with a long newest date). The body's contents now keep one state in both layouts.
