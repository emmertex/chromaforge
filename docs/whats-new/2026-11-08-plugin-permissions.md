### Plug-ins ask for permissions when they need them
- Installing a plug-in grants nothing: the install dialog lists what it may ask for, and the first time a command you started needs a permission, the app asks. Allow runs the command again; Deny is remembered.
- The Plug-in Manager's Permissions tab shows each permission as allowed, denied (with the date), not asked yet or refused, with Allow and Revoke.
- Only you can allow a permission: agents, scripts and the command line can revoke one, never grant it, and a plug-in can never be given the app's settings folder or the plug-ins folder.
