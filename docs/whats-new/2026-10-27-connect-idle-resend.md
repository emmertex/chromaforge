### A command sent just as an idle connection closes is no longer lost
- Agents driving the app with `--connect` sometimes got an "idle … closing the connection" error for a command sent
  right as the app closed an idle connection. The app never ran that command, so it is now sent again once on a new
  connection.
