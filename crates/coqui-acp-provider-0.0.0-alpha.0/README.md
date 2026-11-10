# coqui-acp-provider

[![Version](https://img.shields.io/crates/v/coqui-acp-provider.svg)](https://crates.io/crates/coqui-acp-provider)
[![Rust version](https://img.shields.io/crates/msrv/coqui-acp-provider.svg)](https://crates.io/crates/coqui-acp-provider)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/Boriken-Dev/coqui-acp/blob/main/LICENSE)
[![Docs](https://img.shields.io/docsrs/coqui-acp-provider)](https://docs.rs/coqui-acp-provider)
[![CI](https://img.shields.io/github/actions/workflow/status/Boriken-Dev/coqui-acp/test.yml)](https://github.com/Boriken-Dev/coqui-acp/actions/workflows/test.yml)

**Write an agent once, in any language [Guatiao](https://github.com/Boriken-Dev/guatiao) hosts, and serve it over the [Agent Client Protocol](https://agentclientprotocol.com).** A provider starts sessions and reports what happens as events; `AcpFacade` turns that into ACP for any client, over any transport. Guatiao stays in this crate, so `coqui-acp` itself needs none of it. Documentation: <https://boriken-dev.github.io/coqui-acp/provider/>.

## Features

- `AgentProvider` and `AgentSession`: a Guatiao provider kind and object kind, described as data for languages with no Rust. A provider owns its own process.
- `AgentEvent`: one string vocabulary (`text`, `tool`, `blocked`, `completed`, `failed`, `cancelled`, `limit`, `denied`), crossing as a Guatiao value and as JSON.
- `AcpFacade`: `session/new`, `session/prompt`, `session/cancel`, `session/resume`, `session/close`, optional authentication. One provider session per ACP session, and every session ends with its connection.
- `StartOption`: the models, efforts and modes a host lets a client choose, as ACP's config options and modes, each value from the host's list.
- A provider that blocks, fails or panics costs one session, never the connection. Every limit is a `Settings` field with a safe default.

ACP version 1.

## Installation

Not on crates.io yet:

```sh
cargo add --git https://github.com/Boriken-Dev/coqui-acp coqui-acp-provider
```

There are no Cargo features.

## Quick start

A provider in Rust, served to a client in the same process:

```rust
use std::sync::Arc;
use std::sync::Mutex;

use coqui_acp_provider::guatiao::library::{Object, ProviderError};
use coqui_acp_provider::{
    AcpFacade, AgentEvent, AgentMessage, AgentProvider, AgentSession, COMPLETED_EVENT, StartRequest,
    TEXT_EVENT, sdk,
};

struct Echo(Mutex<Vec<AgentEvent>>);

impl AgentSession for Echo {
    fn session_id(&self) -> Result<Option<String>, ProviderError> {
        Ok(None)
    }
    fn send_message(&self, message: AgentMessage) -> Result<(), ProviderError> {
        self.0.lock().unwrap().extend(answer(&message.text));
        Ok(())
    }
    fn next_event(&self, timeout_ms: i64) -> Result<Option<AgentEvent>, ProviderError> {
        let mut events = self.0.lock().unwrap();
        if events.is_empty() {
            drop(events);
            std::thread::sleep(std::time::Duration::from_millis(timeout_ms as u64));
            return Ok(None);
        }
        Ok(Some(events.remove(0)))
    }
    fn interrupt(&self) -> Result<(), ProviderError> {
        self.0.lock().unwrap().push(AgentEvent::new(COMPLETED_EVENT, None));
        Ok(())
    }
    fn close(&self) -> Result<(), ProviderError> {
        Ok(())
    }
}

fn answer(text: &str) -> [AgentEvent; 2] {
    [
        AgentEvent::new(TEXT_EVENT, Some(format!("echo: {text}"))),
        AgentEvent::new(COMPLETED_EVENT, None),
    ]
}

struct EchoProvider;

impl AgentProvider for EchoProvider {
    fn start(&self, request: StartRequest) -> Result<Object<dyn AgentSession>, ProviderError> {
        Ok(Echo(Mutex::new(answer(&request.prompt).to_vec())).into_object())
    }
}

let (agent, _client) = sdk::Channel::duplex();
let connection = AcpFacade::new(Arc::new(EchoProvider)).serve(agent)?;
drop(_client);
connection.join().expect("the connection's thread did not panic")?;
# Ok::<(), coqui_acp_provider::Error>(())
```

A provider in Python implements the same kinds, from `python/coqui_acp_provider_kinds.py`:

```python
from coqui_acp_provider_kinds import AgentProvider, AgentSession
from guatiao import Ref, provide

__version__ = "1.0.0"
__all__ = ["Echo"]

class Conversation(AgentSession):
    def session_id(self): return None
    def send_message(self, message: Ref): ...
    def next_event(self, timeout_ms: int): ...   # a dict, such as {"kind": "text", "text": "hi"}, or None
    def interrupt(self): ...
    def close(self): ...

class Echo(provide.Provider, AgentProvider):
    def start(self, request: Ref) -> Conversation: ...   # request.to_python() is the StartRequest as a dict
```

A host in Rust loads either through a Guatiao `Registry` and hands `offer.shared()` to `AcpFacade::new`. `examples/provider-python` in the repository is all of it, over standard input and output.

## API overview

Everything is at the crate root.

| Item | What it is |
| --- | --- |
| `AgentProvider`, `AgentSession` | the kinds a provider serves |
| `StartRequest`, `ResumeRequest`, `AgentMessage` | what goes in |
| `AgentEvent` (with `coqui-acp-api`'s `LimitHit` and `UsageReport`), the `*_EVENT` constants, `TURN_CANCELLED` | what comes out |
| `AcpFacade`, `Settings`, `FacadeAuth`, `Credential`, `Error` | a provider as an ACP agent |
| `StartOption`, `OptionValue`, `OptionTarget` | what a client may choose before a session starts |
| `LAUNCH_META_KEY`, `EVENT_NOTIFICATION`, `EVENTS_CAPABILITY`, `SESSION_OPTIONS_METHOD`, `SESSION_OPTIONS_CAPABILITY` | what the facade adds to ACP |
| `sdk`, `guatiao` | the protocol's own crate and Guatiao, re-exported so a provider names one version of each |

`AGENTS.md` beside this file is the whole contract.

## Development

```sh
cargo test -p coqui-acp-provider
cargo test -p provider-python   # needs a Python with its shared library; ignored without one
```

Changing the kinds changes the hashes every built provider is checked against: regenerate the description with `COQUI_ACP_PROVIDER_WRITE_KINDS=1 cargo test -p coqui-acp-provider --test kinds` and the Python module with `python/write_kinds.py`, with Guatiao's Python binding on `PYTHONPATH`.

## License

[MPL-2.0](https://github.com/Boriken-Dev/coqui-acp/blob/main/LICENSE).
