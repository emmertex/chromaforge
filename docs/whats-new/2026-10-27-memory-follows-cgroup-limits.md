### Memory use follows container limits on Linux
- In a container, a Flatpak or a systemd scope with a memory limit, the app sized its caches, merges and exports
  from the whole machine's RAM and could be killed for going over the limit. It now counts the limit as the RAM.
