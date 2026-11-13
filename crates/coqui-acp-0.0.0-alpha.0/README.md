# coqui-acp

[![Version](https://img.shields.io/crates/v/coqui-acp.svg)](https://crates.io/crates/coqui-acp)
[![Rust version](https://img.shields.io/crates/msrv/coqui-acp.svg)](https://crates.io/crates/coqui-acp)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/Boriken-Dev/coqui-acp/blob/main/LICENSE)
[![Docs](https://img.shields.io/docsrs/coqui-acp)](https://docs.rs/coqui-acp)
[![CI](https://img.shields.io/github/actions/workflow/status/Boriken-Dev/coqui-acp/test.yml)](https://github.com/Boriken-Dev/coqui-acp/actions/workflows/test.yml)

**Drive any [Agent Client Protocol](https://agentclientprotocol.com) agent from plain blocking code.** The protocol's own SDK gives you the
messages; this adds what every client otherwise writes itself: the connect, authenticate, resume and turn state machine, permission
deciding, and line transports. It knows nothing of the application above it. Documentation: <https://boriken-dev.github.io/coqui-acp/>.

## Features

- `Client::builder`: new, resume (falling back to load, its replay dropped) or load.
- `Event`: one stream of text, tools, cost, permissions and turn endings.
- `Decider`: answers permission requests, on a thread of its own. The default refuses everything.
- `Elicitor`: puts an agent's questions (`elicitation/create`, form mode) to the person, and sends an answer only when it fits the form.
- Refused tool kinds: refused without asking, and a tool of such a kind that runs unasked ends the session.
- Extensions: `_meta`, `_`-prefixed notifications both ways, and `turn_end_by` for an agent with its own turn events.
- `secret` (feature): an agent asks the person for a secret, or shows them one, and the value travels sealed (HPKE) to a key only its receiver holds, bound to an identity the collector knows.
- `Exchange::describe`: an agent's start options, modes and sessions before any session, each known, unknown with a reason, or not supported.
- No async runtime required, and a finite default on every wait.

ACP version 1. A v2 agent selects version 1 when asked for it.

## Installation

```sh
cargo add coqui-acp
```

One Cargo feature, `secret`, off by default: `coqui_acp::secret`, the messages and sealing of the [secret-prompt extension](https://boriken-dev.github.io/coqui-acp/secrets/). The SDK turns on `serde_json`'s `preserve_order` for the whole build.

## Quick start

```rust,no_run
use std::process::{Command, Stdio};
use std::time::Duration;
use coqui_acp::{Client, Event, lines};

let mut agent = Command::new("gemini").arg("--acp").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()?;
let (input, output) = (agent.stdin.take().unwrap(), agent.stdout.take().unwrap());
let (transport, _diagnostics) = lines::over_io(output, input, 20, lines::DEFAULT_MAX_LINE)?;

let session = Client::builder("/work").prompt("Say hello").connect(transport)?;
while let Some(event) = session.next_event(Duration::from_secs(60)) {
    match event {
        Event::Text(text) => println!("{text}"),
        Event::Completed { .. } | Event::Failed(_) | Event::Cancelled => break,
        _ => {}
    }
}
session.close();
# Ok::<(), Box<dyn std::error::Error>>(())
```

## API overview

| Item | Purpose |
| --- | --- |
| `Client`, `Builder`, `Session` | open and drive a session |
| `Event`, `Error` | what happened, and why it could not |
| `Decider`, `Decision`, `PermissionRequest` | answer an agent asking leave |
| `Elicitor`, `Elicitation`, `Field`, `FieldKind`, `ChoiceOption`, `ElicitationAnswer` | answer an agent's question, as a form |
| `Exchange` | calls that are not a session |
| `Description`, `Answer`, `SessionList`, `DescribeLimits` | what an agent offers before any session: `Exchange::describe`, and what it keeps |
| `lines` | a transport over a process's pipes |
| `sdk` | the protocol's own crate, for its transports |

## Development

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features
```

### Releasing

Not released: `publish = false` until a release is chosen.

## License

MPL-2.0; see [LICENSE](https://github.com/Boriken-Dev/coqui-acp/blob/main/LICENSE).
