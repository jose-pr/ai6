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

> Status: pre-alpha. In `m/`: the watchdog, terminal providers (including a Rust plugin),
> hooks and agent profiles, tested on Linux. macOS runs in CI; Windows is a scaffold.
> `moneypenny/` and `q/` are documented designs.

## Features

The work is split the way the Secret Intelligence Service splits it
([`docs/organization.md`](https://github.com/jose-pr/coagent/blob/main/docs/organization.md)
has the conventions):

| Division | Dir | What it owns | State |
| --- | --- | --- | --- |
| **M's Office** | `m/` | Stations, mission authorization, scheduling, the watchdog that kills a stalled or looping agent with every process it started, pluggable terminals, hooks, and assigning agent profiles to missions | watchdog, terminals, hooks, profiles, stations and brief-driven launch implemented; scheduler and worktrees designed |
| **00 Section** | `00/` | The agents that execute: the roster of codenamed profiles, clearance, one adapter per agent CLI | roster format implemented in M; adapters designed |
| **Q Branch** | `q/` | Gear issued per mission: credentials scoped to a station, sandboxes, tools that act (shell, file writes, git, deploy) | design |
| **I Branch** | `i/` | Intelligence and comms: mission briefs, the inbox, the contract user-supplied intake fills, the wire format, briefings and debriefs | briefs, inbox and intake contract implemented; wire, briefings, debriefs designed |
| **Moneypenny** | `moneypenny/` | M's front desk: a Kanban board of runs (*Working · Blocked · Stalled · Done*) per station, and a WebGL terminal attached live to any agent | design |

Projects are **Stations**: each registered project has an id (`G`, `GT`, `B2`), a name
and any cover names, and holds its own checkout, roster, clearance and budget.

- **Supervisor–worker (harness) pattern.** Agents are disposable workers. M is the only
  long-lived process that holds state.
- **Terminals are first-class.** An agent runs on a pseudo-terminal, so it behaves as it
  would for a person, and a person can take over without restarting it.
- **Bounded by default.** No output for 300 s, or 300 s of output with nothing new,
  ends the run. Nothing waits forever on a `[y/N]` nobody will see.
- **Any terminal, any OS, as a plugin.** A terminal comes from a provider: the built-in
  POSIX pty, plain pipes, a Windows pseudo-console, or a guatiao library written in any
  language. The first plugin is Rust on `portable-pty`, one codebase for every OS.
- **Profiles: M knows her agents.** A profile names a model configuration under a
  codename (`007`, alias `JamesBond`): which CLI to launch, which model and effort,
  what environment, what it is good for. M picks the best profile for a role.
- **Hooks, per scope.** Headquarters, station and agent hooks add environment, inject
  context, rewrite the command line, and see every event, in any language. A
  project's own hooks run only once it is registered as a station.
- **Bring your own intake.** No tracker is built in. Anything that can write a JSON
  file, or print one, can hand M a mission: a webhook receiver, a Jira poller, a skill,
  an overlay.

## Installation

Only `m/` is installable today: Python 3.9 or newer, and no runtime dependency beyond
`tomli` on 3.9 and 3.10.

```bash
git clone https://github.com/jose-pr/coagent.git
cd coagent/m
python3 -m venv .pyvenv/local && .pyvenv/local/bin/pip install -e ".[dev]"
```

Optional features/extras:

| Extra/flag | Adds | Needed for |
| --- | --- | --- |
| `guatiao` | `guatiao` (pinned to a commit) | terminal plugins in any language, such as the Rust `portable-pty` one |
| `conpty` | `pywinpty` (Windows) | the Windows pseudo-console provider, once implemented |

Terminal plugins also need the native library and the plugin, built from the
repository root:

```bash
cargo build -p coagent-native -p coagent-terminal-pty
export GUATIAO_LIBRARY=$PWD/target/debug/libcoagent_native.so   # .dylib on macOS
export COAGENT_TERMINAL_PLUGINS=$PWD/target/debug
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

See which terminals can run here, best first:

```bash
$ python -m coagent_m.watchdog providers
  30  coagent_terminal_pty_portable_pty ok
  20  conpty               unavailable: ConPTY exists only on Windows
  20  posix-pty            ok
   0  pipes                ok
```

Give M a roster in `~/.config/coagent/profiles.toml`, then launch by codename or by
role. The launcher's command is whatever your agent CLI takes; the placeholders come
from the profile:

```toml
[launcher.cli]
command = ["agent-cli", "--model", "{model}"]
prompt = "arg"

[profile.007]
aliases = ["JamesBond"]
launcher = "cli"
model = "<model-id>"
effort = "high"
roles = ["plan", "review"]
priority = 10
context = "You review for security first."

[profile.q-branch]
extends = "007"
effort = "medium"
roles = ["implement"]
```

```bash
python -m coagent_m.watchdog launch --profile JamesBond "plan the intake module"
python -m coagent_m.watchdog launch --role review/security --station G "audit the gateway"
```

Hand M a mission from anything that can write JSON, then let her take it:

```bash
echo '{"id": "GUA-12", "title": "Fix the flaky test", "station": "Kingston", "role": "implement"}' \
  | python -m coagent_i submit
python -m coagent_m.watchdog launch --next
```

Inject environment and context with hooks, for example from a tool that knows the
directory, in `~/.config/coagent/hooks.toml` (or a station's `.coagent/hooks.toml`):

```toml
[[hook]]
name = "dir-env"
command = ["dotagents", "env"]
output = "env"

[[hook]]
name = "dir-context"
command = ["dotagents", "context"]
output = "context"
```

## Command line

```bash
python -m coagent_m.watchdog run [--stall-timeout S] [--loop-timeout S] [--max-runtime S] [--terminal NAME] [--no-hooks] -- CMD [ARGS...]
python -m coagent_m.watchdog launch (--profile NAME | --role ROLE [--tag T]...) [--station S] [--no-hooks] [PROMPT]
python -m coagent_m.watchdog launch (--next [--inbox DIR] | --brief FILE) [--station S] [--no-hooks]
python -m coagent_m.watchdog providers
python -m coagent_m.watchdog profiles
python -m coagent_m.watchdog stations
python -m coagent_i [--inbox DIR] (submit [FILE|-] | list [--state S] | sources | poll)
python -m coagent_m.watchdog hooks
python -m coagent_m.watchdog demo [--timeout S] [--terminal NAME]
```

`run` and `launch` pass the agent's terminal output through and exit with the agent's
code, or `128 + signal` when the watchdog killed it.

## API overview

| Module | Purpose |
| --- | --- |
| `coagent_m.watchdog` | `Watchdog` (runs one agent), `ActivityMonitor` (stall and loop rules, no I/O), `CursorQueryResponder`, `Limits`, `AgentSpec`, `Event`, `Outcome`, `State`, `KillReason` |
| `coagent_m.terminal` | `Provider`, `Session`, `TerminalSpec`, `Registry`, `default_registry()`: where a terminal comes from |
| `coagent_m.hooks` | `HookContext`, `Preparation`, `CommandHook`, `HookSet`, `load_hooks()`: what runs around a session |
| `coagent_m.profiles` | `Launcher`, `Profile`, `ProfileSet`, `load_profiles()`: the model configurations M assigns |
| `coagent_m.stations` | `Station`, `Stations`, `CLEARANCES`, `local_config()`: the projects M runs missions for |
| `coagent_m.config` | `config_dir()`, `find_project()`, `Scope` |
| `coagent_i` | `Brief`, `Inbox`, `CommandSource`, `load_sources()`, `poll()`, `report()`: missions in, outcomes out |
| `coagent-terminal-kind` (Rust) | the `coagent_terminal` and `coagent_terminal_session` guatiao kinds |
| `coagent-terminal-pty` (Rust) | a terminal plugin on `portable-pty` |

The full contract is in [`m/src/coagent_m/AGENTS.md`](https://github.com/jose-pr/coagent/blob/main/m/src/coagent_m/AGENTS.md).

## Architecture

```text
  trackers, webhooks, skills (yours)       operators (browser)
          │ briefs (JSON)                        │ HTTPS + WSS
          ▼                                      ▼
┌──────────────────────────────┐    ┌──────────────────────────────┐
│ I  (i/)  I Branch            │    │ Moneypenny  (moneypenny/)    │
│  inbox · intake sources      │◄──►│  gateway · board · attach    │
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

### How the divisions decouple

No division depends on another, except on I Branch's formats: the brief, the wire
frames and the report. Each can be rewritten without touching the rest.

1. **Terminals are providers behind one contract in M.** A session exposes the same
   operations whatever runs it: *spawn, read output, write input, resize, kill the
   tree, exit status*. Built in are a POSIX pty, plain pipes and (as a scaffold) the
   Windows pseudo-console. Any other comes from the `coagent.terminals` entry-point
   group or as a guatiao library serving the `coagent_terminal` kind, in any language;
   the first is Rust on `portable-pty`. A `tmux` provider would give each agent a pane,
   so `ssh host -t tmux attach -r -t agent-42` keeps working beside the web view, and a
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
4. **I takes missions in from anything.** A brief is one JSON document; the inbox is a
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

### Stations

A station is a project M runs missions for, registered in the global `stations.toml`:

```toml
[station.G]
name = "guatiao"
aliases = ["Kingston"]
path = "~/devel/guatiao"
repos = ["https://github.com/Boriken-Dev/guatiao"]   # a station may span many repos
roster = ["007", "006"]                              # absent: every profile
clearance = "field"                                  # the highest a mission here may use
max_concurrent = 3
intake = { jira = "GUA" }                            # for intake sources; M ignores it
```

The id is any short code (a letter is the convention, not the limit), and a station
answers to its id, its name and every alias. Registering a project is what makes its
`.coagent/` (its hooks and profiles) readable: a directory no station holds is never
read. Configuration runs headquarters → station → agent, the narrowest winning.

### Missions

```text
source ─► brief ─► inbox/new ─► M claims it ─► station ─► profile ─► agent runs
                                                                         │
           source ◄─ report ◄── inbox/done|failed ◄── outcome ◄──────────┘
```

`python -m coagent_m.watchdog launch --next` takes the best brief, resolves its station
(id, name or alias), uses the profile it names or the best one for its role and tags
that the station accepts, runs it in the station's checkout, files the outcome, and
reports it to the brief's source. `launch --brief FILE` does the same for one file.

### Profiles and hooks

M does not hard-code which agent runs a task. A **profile** is a model configuration
under a codename: the launcher (a command template for one agent CLI), the model, its
effort, extra arguments and environment, the terminal provider, watchdog limits and
standing context, plus the `roles` and `tags` it suits. Profiles `extend` each other,
and M picks the best-ranked profile that suits a role.

**Hooks** run around every session: before it starts (add environment, add context,
rewrite the command line), on every state change, and once it ends. A hook is a Python
object or a program in any language reading JSON on stdin. They load in three scopes,
broadest first, so the narrowest wins: headquarters (`~/.config/coagent/`), station
(`<station>/.coagent/`) and agent (the profile, or code).

A project's `.coagent/` is read only when the project is a registered station (or with
`COAGENT_TRUST_PROJECT=1`, for a checkout under development), because its hooks and
launchers run inside the supervisor, which holds the credentials.

The context collected reaches the agent as a file named by `COAGENT_CONTEXT_FILE`.

### Terminal handshakes

A shell under a pty, and ConPTY at start-up, asks where the cursor is (`ESC[6n`) and
waits. With no emulator attached that looks exactly like a hung agent, so the watchdog
answers every query itself, always with row 24, column 1, and removes it from the
stream so an attached viewer's emulator does not answer a second time.

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
| [Boriken-Dev/guatiao](https://github.com/Boriken-Dev/guatiao) | Not prior art but a dependency: the wire format and framing for M ↔ gateway ↔ browser, from Rust, wasm and Python |

## Repository layout

| Path | Language | What |
| --- | --- | --- |
| `m/` | Python | M's Office: package `coagent_m` |
| `m/crates/` | Rust | The terminal kind, the `portable-pty` plugin and the native library M loads |
| `00/` | data, adapters | The 00 Section: the example roster; CLI adapters to come |
| `q/` | Python | Q Branch: credentials, sandboxes, acting tools, loadouts |
| `i/` | Python, Rust | I Branch: package `coagent_i` (briefs, inbox, intake); the wire crate to come |
| `moneypenny/` | Rust: the gateway, and the browser client compiled to wasm | The team viewport |

## Development

```bash
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build -p coagent-native -p coagent-terminal-pty

cd m
python3 -m venv .pyvenv/local && .pyvenv/local/bin/pip install -e ".[dev,guatiao]"
export GUATIAO_LIBRARY=$PWD/../target/debug/libcoagent_native.so COAGENT_TERMINAL_PLUGINS=$PWD/../target/debug
.pyvenv/local/bin/pytest -q
.pyvenv/local/bin/black --check src tests
```

```bash
cd i
python3 -m venv .pyvenv/local && .pyvenv/local/bin/pip install -e ".[dev]"
.pyvenv/local/bin/pytest -q
```

M's mission tests run when `coagent-i` is installed into M's environment too
(`pip install -e ../i`). Without the two variables the suite still runs, on the
built-in providers only. CI
(`test.yml`, on a `ci-*` tag or by hand) runs it on Linux and macOS, Python 3.9 and
3.14, with the plugin.

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
