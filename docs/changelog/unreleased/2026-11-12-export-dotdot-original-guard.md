## Fixes

- **Exports can't reach an original through `..`.** A destination whose `..` runs through a folder that doesn't exist yet is refused instead of being checked as a free name (upstream #4125; the rest of that PR was already taken from #2634).
