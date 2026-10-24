# coagent

[![Version](https://img.shields.io/badge/version-0.1.0.dev0-blue.svg)](https://github.com/jose-pr/coagent/blob/main/CHANGELOG.md)
[![Python versions](https://img.shields.io/badge/python-3.9%20%E2%80%93%203.14-blue.svg)](https://github.com/jose-pr/coagent/blob/main/m/pyproject.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/jose-pr/coagent/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://jose-pr.github.io/coagent/)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/coagent/test.yml)](https://github.com/jose-pr/coagent/actions/workflows/test.yml)

**C.O.A.G.E.N.T.** — **C**ontrol plane for **O**rchestrated, **A**utonomous
**G**lobal **E**xecution & **N**etwork **T**erminals.

A **self-hosted supervisor for fleets of coding agents, where every agent is a real
terminal a human can step into**. A daemon takes tasks from your tracker, gives each
sub-agent its own git worktree and sandbox, and kills any agent that hangs at a prompt
or loops. A team web board shows every run as a card, and any card opens a live,
interactive terminal attached to that agent's actual session. The board never knows
whether that session is a pty, a tmux pane or a systemd unit.

> Status: pre-alpha scaffolding. The watchdog in `m/` is the only working code.
> `moneypenny/` and `q/` are documented designs.

## Features

| Component | What you get | State |
| --- | --- | --- |
| **M** — supervisor | One worktree and sandbox per task; a stdout watchdog that SIGKILLs a stalled or looping agent and its whole process group; lifecycle events for the board | watchdog implemented and tested |
| **Moneypenny** — viewport | A Kanban board of runs (*Working · Blocked · Stalled · Done*); a WebGL terminal attached live to any agent, read-only or with a write lease | design |
| **Q** — armory | Scoped, short-lived credentials injected at spawn and never written to disk; sandbox profiles; MCP server "loadouts" (filesystem, shell, database) per agent | design |

- **Supervisor–worker (harness) pattern.** Agents are disposable workers. M is the only
  long-lived process that holds state.
- **Terminals are first-class.** An agent runs on a pseudo-terminal, so it behaves as it
  would for a person, and a person can take over without restarting it.
- **Bounded by default.** No output for 300 s, or 300 s of output with nothing new,
  ends the run. Nothing waits forever on a `[y/N]` nobody will see.

## Installation

Only `m/` is installable today: POSIX, Python 3.9 or newer, and no runtime
dependencies.

```bash
git clone https://github.com/jose-pr/coagent.git
cd coagent/m
python3 -m venv .pyvenv/local && .pyvenv/local/bin/pip install -e ".[dev]"
```

## Quick start

Watch the watchdog handle three mock agents: a healthy one, one stuck in a retry loop,
and one hung at a prompt. A 3-second window stands in for the 300-second default:

```bash
.pyvenv/local/bin/python -m coagent_m.watchdog demo --timeout 3
```

```text
05:26:25 work     working  pid 1083
05:26:25 loop     working  pid 1085
05:26:25 hang     working  pid 1087
05:26:26 work     done     exit 0
05:26:26 hang     blocked  waiting at a prompt
05:26:28 loop     stalled  loop
05:26:28 hang     stalled  input_hang
```

Supervise any command with the production defaults:

```bash
.pyvenv/local/bin/python -m coagent_m.watchdog run -- claude -p "fix the flaky test"
```

## Command line

```bash
python -m coagent_m.watchdog run [--stall-timeout S] [--loop-timeout S] [--max-runtime S] -- CMD [ARGS...]
python -m coagent_m.watchdog demo [--timeout S]
```

`run` passes the agent's terminal output through and exits with the agent's code, or
`128 + signal` when the watchdog killed it.

## API overview

| Module | Purpose |
| --- | --- |
| `coagent_m.watchdog` | `Watchdog` (runs one agent on a pty), `ActivityMonitor` (stall and loop rules, no I/O), `Limits`, `AgentSpec`, `Event`, `Outcome`, `State`, `KillReason` |

The full contract is in [`m/src/coagent_m/AGENTS.md`](https://github.com/jose-pr/coagent/blob/main/m/src/coagent_m/AGENTS.md).

## Architecture

```text
            Jira JQL poll / webhook                        operators (browser)
                     │                                             │  HTTPS + WSS
                     ▼                                             ▼
┌──────────────────────────────────────┐      ┌───────────────────────────────────────┐
│ M  (m/)  supervisor daemon           │      │ Moneypenny  (moneypenny/)             │
│  task intake ─► scheduler            │      │  gateway (Rust): auth, fan-out,       │
│  worktree + sandbox per task         │ unix │  write lease, scrollback replay       │
│  session backend: pty│tmux│systemd   │◄────►│  board API (cards = run states)       │
│  watchdog: stall / loop ─► SIGKILL   │ sock │  client: Rust→wasm board + xterm.js   │
│  event log (state transitions)       │      └───────────────────────────────────────┘
└──────────────┬───────────────────────┘
               │ spawn(spec, loadout)       ┌───────────────────────────────────────┐
               ├───────────────────────────►│ Q  (q/)  armory                       │
               │   env + MCP config         │  secret broker (scoped, short-lived)  │
               ▼                            │  sandbox profiles                     │
     ┌───────────────────┐                  │  MCP loadouts: fs, shell, db          │
     │ sub-agent (pty)   │ ◄─── tools ───── └───────────────────────────────────────┘
     │ in its worktree   │
     └───────────────────┘
```

### How the three decouple

The boundary that matters is between **where a session runs** and **who is watching
it**. Three seams keep them apart:

1. **Session backends sit behind one interface in M.** A session exposes the same five
   operations whatever runs it: *spawn, read output, write input, resize, kill*. The
   `pty` backend is what `watchdog.py` uses today. A `tmux` backend gives each agent a
   pane, so `ssh host -t tmux attach -r -t agent-42` keeps working beside the web view.
   A `systemd` backend runs each agent as a transient unit (`systemd-run --scope`) for
   cgroup limits and accounting. The watchdog only ever sees bytes and timestamps, so
   its rules are the same for all three.
2. **Moneypenny speaks a terminal protocol, not a backend.** M sends the gateway framed
   messages over a local Unix socket: `output`, `event`, `input`, `resize`, each with a
   session id. The gateway fans one session's output out to any number of browsers and
   replays a scrollback ring buffer to each new viewer. It forwards input from only the
   viewer that holds the session's **write lease**, so two people never type into one
   agent at once. Browsers never reach M or a backend directly.
3. **Q is consulted at spawn time and then gets out of the way.** M asks Q for a
   *loadout*: environment, credentials with a TTL, the MCP servers to start, and the
   sandbox profile. M passes it to the backend. Secrets reach the agent's environment
   and never its worktree, the event log or the terminal stream.

The frame types are one Rust crate, compiled natively into the gateway and to wasm into
the browser client, so the two ends cannot disagree about the protocol. The board is
Rust compiled to wasm. The terminal widget stays xterm.js with its WebGL renderer,
behind a small interop shim: it already draws on the GPU, and its input handling (IME,
selection, screen readers) is ahead of any Rust terminal. So wasm buys a shared codec
and a Rust UI, not a faster terminal.

The web side has its own security rules, borrowed from ttyd's model. Attach is
authorized per session and mode (`read` or `write`) with a short-lived token minted by
the gateway. WebSockets are same-origin and TLS-only. Operator input counts as activity
for the watchdog, so a person driving an agent is never killed for being slow.

### Lifecycle

```text
pending ─► working ⇄ blocked ─► done | failed
              │         │
              └────┬────┘
                   ▼
                stalled   (SIGKILL: input_hang · silent · loop · max_runtime)
```

*Blocked* means the output ends in something shaped like a prompt (`? `, `: `,
`[y/N]`, `password`) and has been quiet for `blocked_after`. It is a warning on the
board, not a kill: an operator can attach and answer before `stall_timeout` runs out.
*Loop* means output keeps arriving but no line has been new for `loop_timeout`. Lines
are compared after stripping ANSI escapes, and `\r` counts as a line break, so a
spinner redrawing in place counts as repetition.

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

## Repository layout

| Path | Language | What |
| --- | --- | --- |
| `m/` | Python | The supervisor daemon: package `coagent_m` |
| `moneypenny/` | Rust: the gateway, and the browser client compiled to wasm | The team viewport |
| `q/` | Python | Credentials, sandbox profiles and MCP loadouts |

The components share no code. They talk through the socket protocol and the loadout
format, so each can be rewritten without touching the others.

## Development

```bash
cd m
python3 -m venv .pyvenv/local && .pyvenv/local/bin/pip install -e ".[dev]"
.pyvenv/local/bin/pytest -q
.pyvenv/local/bin/black --check src tests
```

### Releasing

This project follows [Semantic Versioning](https://semver.org/) and keeps a
[`CHANGELOG.md`](https://github.com/jose-pr/coagent/blob/main/CHANGELOG.md). Nothing is
released yet.

## License

**Mozilla Public License 2.0** — see
[`LICENSE`](https://github.com/jose-pr/coagent/blob/main/LICENSE). Its copyleft is per
file: a modified file of this project must stay under the MPL 2.0, while your own files
beside it, such as agent scripts, loadouts and integrations, are yours under any licence.
Contributions need a one-time signature of the
[contributor licence agreement](https://github.com/jose-pr/coagent/blob/main/CLA.md); see
[`CONTRIBUTING.md`](https://github.com/jose-pr/coagent/blob/main/CONTRIBUTING.md).
