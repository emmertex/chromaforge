## Plug-ins

- **Plug-ins carry their own icon and header:** a plug-in's manifest may carry `icon` and `header` SVGs as a pixel filter's does (same size limit and untrusted drawing). The Plug-in Manager shows the icon in its list and on the plug-in's page, the header in place of the plug-in's name, and its command dialogs show the header above their title. `plugin.list` shows both as booleans and `plugin.picture` serves them for plug-ins too. PR #912.
