## Development

- **Diagnostics recorder:** off unless `<PREFIX>_DIAG` is set; records an import's probe, commit and library saves in an in-flight file, a bounded ring and aggregate timings under the settings folder's `diag/`, so a hang or a kill leaves a record of what was running. Nothing is sent anywhere (#1363).
