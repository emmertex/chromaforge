---
title: CLI, control and MCP
description: Everything the UI does, scriptable.
---

- Every action is a command with an id; the UI, CLI, control channel and MCP all run the same ones.
- **CLI**: render, export and run commands without a window.
- **Control channel**: JSON lines over a local port; drive a running app.
- **MCP server**: let an AI agent work on your library, headless or in the running app.
- Details: the project's `docs/control-protocol.md` and `docs/mcp.md`.
