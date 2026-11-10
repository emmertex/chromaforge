---
title: Control channel
description: Drive a running app with JSON lines over a local port.
sidebar:
  order: 4
---

- **Start**: `<binary> --control 7980` opens a JSON-lines server on `127.0.0.1:7980` (loopback only). Without a port the channel is off.
- **Protocol**: one request per line, one reply per line, in order. Methods include `engine.execute` (run any command), `engine.commands`, `ui.inspect`, `ui.screenshot`, `ui.menu.tree`, `ui.key`, `ui.pointer`, `ui.clickWidget`, `ui.widgets`.
- **Token**: the first request on each connection must be `auth` with the run's token. The app writes it to a `control-token-<port>` file in its settings folder; `<cli> run --connect` and `<cli> mcp --connect` read it for you.
- **Safe by default**: other computers cannot reach it, but any program on this one with access to your settings folder can. Enable it only while you need it.
- **Not a window**: the same requests run in [headless snapshots](../headless/), so scripts work with or without a display.

```text
→ {"id": 1, "method": "auth", "params": {"token": "<64 hex digits>"}}
→ {"id": 2, "method": "engine.execute", "params": {"command": "photo.rate", "params": {"rating": 4}}}
← {"id": 2, "ok": true, "result": null}
```

- Complete method list, authentication and limits: `docs/control-protocol.md` in the repository.
