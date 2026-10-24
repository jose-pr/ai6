# Architecture

```text
  trackers, webhooks, skills (yours)       operators (browser)
          │ briefs (JSON)                        │ HTTPS + WSS
          ▼                                      ▼
┌──────────────────────────────┐    ┌──────────────────────────────┐
│ I  (i/)  I Branch            │    │ Moneypenny  (moneypenny/)    │
│  registry · intake sources      │◄──►│  gateway · board · attach    │
│  wire frames · briefings     │    │  one swimlane per station    │
│  debriefs                    │    └──────────────────────────────┘
└──────────────────────────────┘
          │ briefs ▲ reports                     │ frames (I's wire format)
          ▼        │                             ▼
┌────────────────────────────────────────────────────────────────┐
│ M  (m/)  M's Office                                            │
│  stations · authorization · scheduling                         │
│  watchdog · terminals · hooks                                  │
│  profiles: who runs which mission                              │
└────────────────────────────────────────────────────────────────┘
          │ loadout                              │ spawn on a terminal
          ▼                                      ▼
┌──────────────────────────────┐    ┌──────────────────────────────┐
│ Q  (q/)  Q Branch            │    │ 00  (00/)  00 Section        │
│  credentials per station     │───►│  roster · CLI adapters       │
│  sandboxes · tools that act  │    │  run in a station's worktree │
└──────────────────────────────┘    └──────────────────────────────┘
```

## How the divisions decouple

No division depends on another, except on I Branch's formats: the brief, the wire
frames and the report. Each can be rewritten without touching the rest.

1. **Terminals are providers behind one contract in M.** A session exposes the same
   operations whatever runs it: *spawn, read output, write input, resize, kill the
   tree, exit status*. Built in are a POSIX pty, plain pipes and (as a scaffold) the
   Windows pseudo-console. Any other comes from the `coagent.terminals` entry-point
   group or as a guatiao library serving the `coagent_terminal` kind, in any language;
   the first is Rust on `portable-pty`. A `tmux` provider would give each agent a pane,
   so `ssh <theater> -t tmux attach -r -t agent-42` keeps working beside the web view, and a
   `systemd` one a transient unit with cgroup limits. The watchdog only ever sees bytes
   and timestamps, so its rules are the same for every provider.
2. **Moneypenny and M speak I's wire format, not each other.** M sends framed messages
   (`output`, `event`, `input`, `resize`, each with a session id) and the gateway fans
   one session out to any number of browsers, replays a scrollback ring to each new
   viewer, and forwards input only from the viewer holding the session's **write
   lease**. Browsers never reach M or a terminal provider directly.
3. **Q is consulted at spawn time and then gets out of the way.** M asks Q for a
   *loadout*: environment, credentials with a TTL, the MCP servers to start, and the
   sandbox. Q equips, I informs: tools that change things are Q's, sources that only
   inform (docs, search, a code index) are I's, and the loadout issues both. Secrets
   reach the agent's environment and never its worktree, the event log or the terminal
   stream.
4. **I takes missions in from anything.** A brief is one JSON document; the registry is a
   directory; a source is any program that prints briefs or any Python object in the
   `coagent.intake` group, and it can hear back how its brief ended.
5. **00 is data plus adapters.** M reads the roster to assign agents; an adapter knows
   one CLI's flags, context file and signals. Swapping agent CLIs touches only `00/`
   and the roster.

The frames are [guatiao](https://github.com/Boriken-Dev/guatiao) values on its wire
format, declared once in a Rust crate in `i/`, compiled natively into the gateway and to
wasm into the browser client, and read from Python through guatiao's bindings. No end
hand-rolls a codec, so no two ends can disagree about the protocol, and the same encoding
also runs over WebTransport. The board is Rust compiled to wasm. The terminal widget
stays xterm.js with its WebGL renderer, behind a small interop shim: it already draws on
the GPU, and its input handling (IME, selection, screen readers) is ahead of any Rust
terminal. So wasm buys a shared codec and a Rust UI, not a faster terminal.

The web side has its own security rules, borrowed from ttyd's model. Attach is
authorized per station, session and mode (`read` or `write`) with a short-lived token
minted by the gateway. WebSockets are same-origin and TLS-only. Operator input counts as
activity for the watchdog, so a person driving an agent is never killed for being slow.

## Terminal handshakes

A shell under a pty, and ConPTY at start-up, asks where the cursor is (`ESC[6n`) and
waits. With no emulator attached that looks exactly like a hung agent, so the watchdog
answers every query itself, always with row 24, column 1, and removes it from the
stream so an attached viewer's emulator does not answer a second time.

## Prior art

| Project | What we take from it |
| --- | --- |
| [BloopAI/vibe-kanban](https://github.com/BloopAI/vibe-kanban) | A Kanban board where a card is an agent run; one git worktree per task so agents work in parallel without colliding; process state shown on the card |
| [OrchestratorInc/agent-orchestrator](https://github.com/OrchestratorInc/agent-orchestrator) | Multi-agent task routing and lifecycle tracking on a shared board |
| [tsl0922/ttyd](https://github.com/tsl0922/ttyd) | One terminal, many browser viewers over a thin WebSocket protocol rendered by xterm.js; per-client auth; read-only by default |
| agent-tmux-web, webmux | tmux as the session layer under a web terminal, so SSH and browser attachments coexist |
| herdr | Many processes multiplexed into one view with their states visible at a glance |
| [tmux](https://github.com/tmux/tmux), [systemd-run](https://www.freedesktop.org/software/systemd/man/latest/systemd-run.html) | Session persistence and detach; transient units with cgroup limits |
| [Model Context Protocol](https://modelcontextprotocol.io/) | The tool interface Q arms agents with |
| [Boriken-Dev/guatiao](https://github.com/Boriken-Dev/guatiao) | Not prior art but a dependency: the wire format and framing for M ↔ gateway ↔ browser, from Rust, wasm and Python |

## Repository layout

| Path | Language | What |
| --- | --- | --- |
| `m/` | Python | M's Office: package `coagent_m` |
| `m/crates/` | Rust | The terminal kind, the `portable-pty` plugin and the native library M loads |
| `00/` | data, adapters | The 00 Section: the example roster; CLI adapters to come |
| `q/` | Python | Q Branch: credentials, sandboxes, acting tools, loadouts |
| `i/` | Python, Rust | I Branch: package `coagent_i` (briefs, registry, intake); the wire crate to come |
| `moneypenny/` | Rust: the gateway, and the browser client compiled to wasm | The team viewport |
