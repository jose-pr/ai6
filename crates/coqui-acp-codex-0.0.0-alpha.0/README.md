# coqui-acp-codex

[![Version](https://img.shields.io/crates/v/coqui-acp-codex.svg)](https://crates.io/crates/coqui-acp-codex)
[![Rust version](https://img.shields.io/crates/msrv/coqui-acp-codex.svg)](https://crates.io/crates/coqui-acp-codex)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/Boriken-Dev/coqui-acp/blob/main/LICENSE)
[![Docs](https://img.shields.io/docsrs/coqui-acp-codex)](https://docs.rs/coqui-acp-codex)
[![CI](https://img.shields.io/github/actions/workflow/status/Boriken-Dev/coqui-acp/test.yml)](https://github.com/Boriken-Dev/coqui-acp/actions/workflows/test.yml)

**Codex as an [Agent Client Protocol](https://agentclientprotocol.com) agent, in Rust, with no Node.** It drives the installed `codex`
over its own JSON-RPC protocol, `codex app-server` on standard input and output, and serves it as an ACP agent in version 1 or 2,
whichever the client asks for. A driver with no ACP in it is public too. Documentation: <https://boriken-dev.github.io/coqui-acp/>.

## Features

- `CodexLaunch`: model, reasoning effort, developer instructions, MCP servers, more directories (skill roots, writable in the
  sandbox), resume or load, approval policy and sandbox. Everything a thread is given goes in `thread/start` or `thread/resume` as
  JSON, never on the command line; the app server's own `--config` overrides are written as TOML by a real escaper.
- `CodexSession`: starts the app server through any `Spawn`, by default a process whose whole tree is ended on close (a job
  object on Windows, a process group on Unix); initializes it and reads its version; lists its models and their efforts; sets the
  skill roots; starts or resumes the thread, refusing one Codex resumed in place of the one asked for; sends turns with text,
  images, links and resources, with the model and effort chosen between turns; interrupts the running turn apart from closing.
- `Parser`: app-server messages as events: message and reasoning chunks, commands as tools with a terminal (command, output,
  exit code), file changes as edits with a git patch made from Codex's hunks, MCP and dynamic tool calls, web searches, plans,
  approvals with a responder, usage and context, quota windows, usage limits, compaction, notices and how a turn ended.
- Approvals are asked with Codex's own decisions: allow, allow for this session, always allow the command rule Codex proposed,
  always allow an MCP tool, reject; a question nobody answered is `cancel`. Any other request of the app server is refused.
- `CodexAgent`: the ACP agent: `initialize`, `session/new`, `session/load`, `session/resume` (version 2's with replay),
  `session/prompt`, `session/cancel`, `session/close`, `request_permission`, model and effort as config options.
- `coqui-acp-codex`, a command that serves it on standard input and output.
- A Codex version it was not tested with runs, with a warning.

## Installation

Not published: `publish = false` until a release is chosen. `cargo build -p coqui-acp-codex` from a checkout builds the library and
the command. There are no Cargo features.

## Quick start

```rust,no_run
use std::sync::mpsc;

use coqui_acp_codex::harness::event::{Content, Event, Outcome};
use coqui_acp_codex::{CodexLaunch, CodexSession, CodexSettings, PlainSpawn};

fn main() -> Result<(), coqui_acp_codex::Error> {
    let mut launch = CodexLaunch::new("/work/repo");
    launch.effort = Some("low".into());
    let (sender, events) = mpsc::channel();
    let session = CodexSession::start(&launch, &PlainSpawn, CodexSettings::new(), move |event| sender.send(event).is_ok())?;
    session.prompt(&[Content::Text("Say hello.".into())])?;
    for event in events {
        match event {
            Event::MessageChunk { text, .. } => print!("{text}"),
            Event::Permission(request) => {
                // `accept`, `accept_for_session`, `decline` and the others the request offers.
                let _ = request.responder.answer(Outcome::Selected("decline".into()));
            }
            Event::TurnEnd(_) => break,
            _ => {}
        }
    }
    session.close();
    Ok(())
}
```

```rust,no_run
use std::sync::Arc;

use coqui_acp_codex::CodexAgent;
use coqui_acp_codex::harness::{Settings, serve};

fn main() -> std::io::Result<()> {
    serve(Arc::new(CodexAgent::new()), Settings::new(), std::io::stdin(), std::io::stdout())
}
```

## Command line

```sh
coqui-acp-codex                                       # codex from PATH, its own defaults
coqui-acp-codex --effort low --approval-policy untrusted --sandbox workspace-write
```

| Flag | Default | What |
| --- | --- | --- |
| `--executable` (`COQUI_ACP_CODEX_EXECUTABLE`) | `codex` | The Codex program |
| `--model`, `--effort` | Codex's | What every session starts with |
| `--approval-policy` | Codex's | `untrusted`, `on-failure`, `on-request` or `never` |
| `--sandbox` | Codex's | `read-only`, `workspace-write` or `danger-full-access` |
| `--allow-bypass` | off | Offers the Bypass mode: never ask, no sandbox |
| `--max-acp-version` | 2 | The highest ACP version answered |
| `--max-sessions` | 8 | Sessions at once |
| `--working-root` | any | A directory a session's `cwd` and more directories must lie in, repeated for more |
| `--no-client-mcp-servers` | off | Refuse a session that a client gives MCP servers |
| `--allow-network-paths` | off | Take a network path as a session's `cwd` or more directories |

Exit status: 0 when the client closes the connection, 1 when reading it failed, 2 for a wrong invocation.

## API overview

| Item | Purpose |
| --- | --- |
| `CodexLaunch`, `ThreadStart`, `valid_config_key`, `toml_value`, `valid_thread_id` | What Codex is started with |
| `CodexSession`, `CodexSettings`, `Model`, `models_of`, `user_input`, `version_of` | A running app server and its thread |
| `Parser` (`parse`) | App-server messages as events |
| `CodexAgent` | The ACP agent |
| `harness` | `coqui-acp-harness`: events, processes and the ACP server |

## Origin

The usage-limit, quota and token-usage reading, the thread checks, and the cases its tests carry over come from AI6's Codex
handler, `crates/providers/agents/codex` of <https://github.com/jose-pr/ai6> at commit
`a40abe845ba0bab14e43e364d2da9a3bdba5e1d4`, MPL-2.0, by the same owner. AI6's tool check, sandbox profiles, gate, terminal
provider and policy are left out.

## Development

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p coqui-acp-codex
```

The tests run against `examples/replay_codex.rs`, which replays app-server traffic recorded from Codex 0.164.0 on Windows in
`tests/fixtures/codex-0.164.0/`: each `<name>.stdout` is the app server's standard output byte for byte but for its
remote-control status line, which names the machine and is left out; `<name>.steps.json` is what was written to it and when, and
`<name>.argv.json` its arguments, which `tests/argv.rs` compares with what the launch builds. `tools/record_harnesses/record.py` makes them. They were recorded against a loopback stand-in for the model, so no model was reached.
`codex_app_server_protocol.schemas.json` is what `codex app-server generate-json-schema` printed for 0.164.0. The tests also passed
against every case recorded from 0.166.0, of which `tests/fixtures/codex-0.166.0/` keeps `turn`.

## License

MPL-2.0; see [LICENSE](https://github.com/Boriken-Dev/coqui-acp/blob/main/LICENSE).
