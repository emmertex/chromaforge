## Fixes

- **Lightroom import and collection-name rules:** a smart collection's Collection rule matches ordinary collections only, as in Lightroom Classic, so a rule matching a smart collection's name no longer makes the whole catalog import fail; a smart collection that would still include itself falls back to a plain collection on its own.
