# Moneypenny

The team viewport of [C.O.A.G.E.N.T.](https://github.com/jose-pr/coagent): a Kanban
board of agent runs and a **live, interactive terminal into any agent's real session**.

Design only; nothing here builds yet. The frames it speaks with M (`output`, `input`,
`resize`, `event`, each with a session id) are I Branch's wire format, a crate in `i/`
that both sides depend on, so Moneypenny and M never depend on each other. Planned
layout, in the repository's Cargo workspace:

| Path | What |
| --- | --- |
| `crates/moneypenny-gateway/` | the server (tokio + axum): reads M's Unix socket, fans each session out to WebSocket (and later WebTransport) viewers, keeps a scrollback ring per session for replay on attach, grants one write lease per session, mints short-lived attach tokens, serves the board API and the client |
| `crates/moneypenny-web/` | the browser client in Rust, compiled to wasm: the board (*Working · Blocked · Stalled · Done*, one swimlane per station) and the attach flow; the terminal itself is xterm.js with its WebGL renderer, driven through a small interop shim |

Rules the gateway keeps:

- Browsers talk only to the gateway, never to M or a terminal provider.
- Terminal bytes travel in binary WebSocket frames (they are not always valid UTF-8);
  control travels as JSON text frames.
- Attach rights are per station.
- Attach is per session and per mode. A `read` attach is the default; `write` needs the
  lease, and only one viewer holds it at a time.
- WebSockets are same-origin and TLS-only. Attach tokens are single-use and short-lived.
- Secrets never pass through: Q injects them into the agent's environment, not its
  terminal.
