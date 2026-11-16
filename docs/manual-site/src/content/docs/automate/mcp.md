---
title: MCP server
description: Let an AI agent edit and cull your library.
sidebar:
  order: 5
---

- **Start**: `<cli> mcp` speaks the Model Context Protocol (JSON-RPC over stdio). Add it to your AI client as a command.
- **Headless** (default): its own session. Options: `--demo`, `--library DIR` (edits are saved), plus files or folders to import.
- **Connect**: `<cli> mcp --connect [127.0.0.1:7980]` drives the running app, started with `--control 7980`. Adds screenshots, clicks and keys.
- **Tools**: helper tools (develop, render, select, rate…) plus one tool per command, or `run_command` for any id. `--compact` lists only the helpers.
- **Limits**: what an agent may do can be restricted; see `docs/mcp.md`.

```sh
<cli> mcp --library ~/lib ~/Photos/shoot
<cli> mcp --connect 127.0.0.1:7980
```

- Prefer a shell to MCP? Use [`<cli> run`](../cli/) for the same commands.
- Wiring into Claude Code, Codex and other clients: `docs/mcp.md` in the repository.
