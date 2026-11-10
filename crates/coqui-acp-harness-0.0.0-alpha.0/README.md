# coqui-acp-harness

[![Version](https://img.shields.io/crates/v/coqui-acp-harness.svg)](https://crates.io/crates/coqui-acp-harness)
[![Rust version](https://img.shields.io/crates/msrv/coqui-acp-harness.svg)](https://crates.io/crates/coqui-acp-harness)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/Boriken-Dev/coqui-acp/blob/main/LICENSE)
[![Docs](https://img.shields.io/docsrs/coqui-acp-harness)](https://docs.rs/coqui-acp-harness)
[![CI](https://img.shields.io/github/actions/workflow/status/Boriken-Dev/coqui-acp/test.yml)](https://github.com/Boriken-Dev/coqui-acp/actions/workflows/test.yml)

**Serve a coding agent's own command line as an [Agent Client Protocol](https://agentclientprotocol.com) agent, with no Node and no
SDK of the agent's.** It holds what every such translator needs and none of what is the agent's: starting the process and ending it
with everything it started, reading its lines within bounds, events in no protocol's terms, and an ACP agent that speaks version 1
or version 2, whichever the client asks for. [`coqui-acp-claude`](https://github.com/Boriken-Dev/coqui-acp/tree/main/crates/coqui-acp-claude)
and [`coqui-acp-codex`](https://github.com/Boriken-Dev/coqui-acp/tree/main/crates/coqui-acp-codex) are built on it. Documentation:
<https://boriken-dev.github.io/coqui-acp/>.

## Features

- `Launch`, `Spawn` and `PlainSpawn`: a process whose `kill` ends its whole tree, a job object on Windows and a process group on
  Unix (with every descendant on Linux), and whose tree ends with the host. Implement `Spawn` to run the harness elsewhere.
- Feature `kind`: `LocalSpawner`, the `acp_harness` kind's spawner on this machine, and `ThroughSpawner`, `Spawn` through any
  host's spawner.
- Feature `antigravity`: `antigravity::AgyQuota`, which reads Antigravity's quota from the loopback port `agy acp` serves it on and gives it as `reported` usage buckets, and `loopback_listeners`, the loopback ports a set of processes listens on.
- `lines`: a harness's output, one JSON value per line, with a limit on a line, and the last lines that were not protocol.
- `event`: messages and thoughts, tool calls with diffs, locations and terminals, plans, permission requests with a responder, usage
  and context, usage buckets and limits, commands, notices, compaction, and how a turn ended.
- `acp::serve`: a `Harness` as an ACP agent over any reader and writer, in version 1's shapes or version 2's. Every limit is a
  `Settings` field with a default.
- `version::untested`: the warning for a harness version a driver was not tested with. Nothing is refused for it.

## Installation

Not published: `publish = false` until a release is chosen. Depend on it by path or by git revision. Feature `kind` adds the spawner.

## Quick start

```rust,no_run
use std::sync::Arc;

use coqui_acp_harness::{AgentInfo, Conversation, EventSink, Harness, HarnessError, OpenRequest, Settings, Takes, serve};

struct Mine;

impl Harness for Mine {
    fn info(&self) -> AgentInfo {
        AgentInfo::new("mine", "Mine", "0.1.0")
    }
    fn takes(&self) -> Takes {
        Takes::new().images(true)
    }
    fn open(&self, _request: OpenRequest, _events: EventSink) -> Result<Box<dyn Conversation>, HarnessError> {
        Err(HarnessError::Failed("not written yet".into()))
    }
}

fn main() -> std::io::Result<()> {
    serve(Arc::new(Mine), Settings::default(), std::io::stdin(), std::io::stdout())
}
```

## API overview

| Module | Purpose |
| --- | --- |
| `process` | `Launch`, `Spawn`, `Spawned`, `Process`, `PlainSpawn`, `Exit` |
| `lines` | `read_lines`, `Line`, `Tail`, `cut` |
| `event` | `Event` and what it carries, `Responder`, `Outcome` |
| `acp` | `Harness`, `Conversation`, `EventSink`, `OpenRequest`, `Settings`, `serve` |
| `version` | `untested` |

## Versions

Version 1 and version 2 differ on the wire, and a client gets the shapes of the version it asked for: version 2's tool calls are
upserts (`tool_call_update` only), its chunks carry the id of their message and a message ends with a whole-message update, a turn
is answered at once with the user message's id and ends with an `idle` state update, permission requests name a `subject`, options
are named `configId`, and it has notices, terminals of the agent's own, compaction and plans with ids. Version 1 gets none of
version 2's kinds.

## Development

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p coqui-acp-harness
```

## License

MPL-2.0; see [LICENSE](https://github.com/Boriken-Dev/coqui-acp/blob/main/LICENSE).
