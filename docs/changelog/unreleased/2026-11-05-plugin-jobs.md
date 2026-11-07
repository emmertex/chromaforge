## Plug-ins

- **Plug-in jobs:** a plug-in command declared as a job (`"job": true`, host feature `jobs`) runs on a worker with a row in the activity stack. It reports its progress, what it is working on and the photos done or failed (`job.*` host calls), stops between calls when the row's ✕ is clicked, and continues past one call's budget with `{continue: state}`. A job may read the catalog but not change it yet. `plugin.run {wait: true}` runs one to its end from the CLI and MCP. The SDK has `job::*` helpers and a `job-tools` example.
