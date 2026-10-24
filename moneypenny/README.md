# Moneypenny

The team viewport of [C.O.A.G.E.N.T.](https://github.com/jose-pr/coagent): a Kanban
board of agent runs and a **live, interactive terminal into any agent's real session**.

Design only; nothing here builds yet. Planned layout, a Cargo workspace:

| Path | What |
| --- | --- |
| `crates/moneypenny-proto/` | the frame types shared with M: `output`, `input`, `resize`, `event`, each with a session id; `no_std`-friendly, so it builds for the gateway and for `wasm32-unknown-unknown` |
| `crates/moneypenny-gateway/` | the server (tokio + axum): reads M's Unix socket, fans each session out to WebSocket viewers, keeps a scrollback ring per session for replay on attach, grants one write lease per session, mints short-lived attach tokens, serves the board API and the client |
| `crates/moneypenny-web/` | the browser client in Rust, compiled to wasm: the board (*Working · Blocked · Stalled · Done*) and the attach flow; the terminal itself is xterm.js with its WebGL renderer, driven through a small interop shim |

Rules the gateway keeps:

- Browsers talk only to the gateway, never to M or a session backend.
- Attach is per session and per mode. A `read` attach is the default; `write` needs the
  lease, and only one viewer holds it at a time.
- WebSockets are same-origin and TLS-only. Attach tokens are single-use and short-lived.
- Secrets never pass through: Q injects them into the agent's environment, not its
  terminal.
