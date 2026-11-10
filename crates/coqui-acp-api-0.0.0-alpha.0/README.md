# coqui-acp-api

[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/Boriken-Dev/coqui-acp/blob/main/LICENSE)
[![CI](https://img.shields.io/github/actions/workflow/status/Boriken-Dev/coqui-acp/test.yml)](https://github.com/Boriken-Dev/coqui-acp/actions/workflows/test.yml)

**Drive an [Agent Client Protocol](https://agentclientprotocol.com) agent from a provider written in any language [Guatiao](https://github.com/Boriken-Dev/guatiao) hosts.** The `acp_harness` kind starts one ACP connection over a Guatiao channel: the host is the client, the provider is the agent, and every channel message is one JSON-RPC message. The kind never models ACP, so a harness implements ACP itself, at whichever version `initialize` agrees. The host hands every start a spawner, so the harness's program runs wherever the host chooses.

## Features

- `AcpHarness`: `describe`, `accounts`, `start`, `refusal`, `program`, `import_session`, `export_session`, `login` and `renew`, which every harness answers; `models`, `session_record`, `usage` and `non_defaults`, whose defaults are true answers for a harness with nothing to say.
- Accounts: several accounts of one harness, chosen per call. A provider publishes the kinds it takes (settings schemas whose credentials are marked sensitive, typed endpoints, whether a kind rotates or renews) and the ways to sign in; `AccountGrant` carries the chosen account and its `Secret<Credentials>`; the host hears of a login the harness renewed or lost through an `AccountKeeper`, and relays a sign-in's steps through a `LoginPrompter`.
- `usage`: how much of each usage allowance the account has used, how it refills and when (`UsageReport`, its `Bucket`s with their coverage, unit and refill, a refused request's `LimitHit`, a session's `Spend`), answerable with no session and no turn, and the same model on the ACP extension `_coqui_acp/usage`.
- `ProcessSpawner` and `SpawnedProcess`: the host's spawner, here, over SSH or in a container: pipes or a terminal, reads that survive a dropped link, a kill that ends the whole tree, and files in a private directory the host grants.
- `StartRequest`: what a host adds to the provider's own configuration for one connection: a secret environment, working roots and requirements with their values.
- Sessions at the host's UUID: `import_session` seeds one from a history in the standard format (empty for a fresh one), ACP's `session/resume` opens it, `export_session` gives its history back, page by page; every session id a harness reports is a UUID.
- `Requirement` and its vocabulary (`read_only`, `no_commands`, `answer_only`, `no_project_settings`, `no_project_mcp`, `files_under`, `read_only_paths`, `denied_paths`, `network`, `sandboxed_scripts`, `only_skills`), which only narrow: a harness refuses a name it does not know.
- `WorkingRoots`: the one way a path is compared with the roots (no `..`, case on Windows, links followed).
- The kinds as data, `include/coqui_acp_api.kinds.json`, the models' schemas, `include/coqui_acp_api.schemas.json`, and a Python module, `python/coqui_acp_api_kinds.py`, for providers with no Rust.
- Feature `contract`: the suite every provider passes, as a module and as `acp-harness-check`, which runs it on a library or a source file in any language.
- No dependency but Guatiao, its channel kinds and serde by default; no `unsafe`.

## Installation

Not on crates.io yet:

```sh
cargo add --git https://github.com/Boriken-Dev/coqui-acp coqui-acp-api
```

`--features contract` adds the suite.

## Usage

A host starts a connection and speaks ACP over it; `coqui_acp_harness::LocalSpawner` runs programs on this machine:

```rust,no_run
use coqui_acp_api::guatiao::library::{Object, ProviderError};
use coqui_acp_api::{AccountKeeper, AcpHarness, ChannelSink, ProcessSpawner, StartRequest};

struct Host;

impl ChannelSink for Host {
    fn receive(&mut self, bytes: &[u8]) -> Result<(), ProviderError> {
        // One JSON-RPC message from the agent.
        let _ = bytes;
        Ok(())
    }

    fn closed(&mut self, status: u32, reason: &str) {
        let _ = (status, reason);
    }
}

fn connect(
    harness: &dyn AcpHarness,
    spawner: Object<dyn ProcessSpawner>,
    keeper: Object<dyn AccountKeeper>,
) -> Result<(), ProviderError> {
    let request = StartRequest::builder()
        .working_root("/work")
        .build();
    if let Some(why) = harness.refusal(request.requirements.clone(), request.environment.clone(), None)? {
        return Err(ProviderError::new(coqui_acp_api::guatiao::Status::GUATIAO_ERR_UNSUPPORTED, &why));
    }
    let channel = harness.start(request, spawner, Host.into_object(), keeper)?;
    channel.send(br#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":1}}"#)?;
    channel.close(0, "");
    Ok(())
}
```

`examples/echo_harness.rs` is a provider, and the API header `AGENTS.md` holds the whole contract.

## Licence

[MPL-2.0](https://github.com/Boriken-Dev/coqui-acp/blob/main/LICENSE).
