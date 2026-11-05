## Fixes

- **A keyword thousands of levels deep no longer takes gigabytes:** the keyword index kept a copy of nearly the whole keyword for every level of it; it now stops at the 128 levels the keyword tree keeps (one 100,000-level keyword took more than 20 GB of memory).
