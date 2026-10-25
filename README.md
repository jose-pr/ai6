# AI6

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/jose-pr/ai6/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/jose-pr/ai6/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/jose-pr/ai6/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://jose-pr.github.io/ai6/)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/ai6/test.yml)](https://github.com/jose-pr/ai6/actions/workflows/test.yml)

AI6 is a self-hosted control plane for coding agents. **Register your projects as
Operations, hand AI6 Missions, and its Stations post an Agent to each one and run it as
a Claude Code or Codex Session in its own Git worktree, under a watchdog** — while
operators follow and steer the work from the Moneypenny app. Read the
[documentation](https://jose-pr.github.io/ai6/).

## Features

- **Stations** - each node runs the `ai6` daemon: it leases Missions from a shared
  Registry, posts the best available Agent, and reattaches to running Sessions after a
  restart. Control Stations elect a Headquarters Station.
- **Agents and roles** - an Agent is a codename that owns its harness, model and effort;
  roles describe the job. When an Agent's harness is unavailable, the next qualified Agent
  is its backup.
- **Safe houses** - every Mission gets an isolated Git worktree; its commits are audited
  against the Operation's cover policy.
- **Cables** - permission-scoped agent-to-agent messages through per-Session MCP tools.
- **Universal Exports** - the authenticated public API: pluggable authentication and
  directories, per-Operation authorization, read-first terminal attachment, audit.
- **Moneypenny** - the Flutter desktop and web operator app; `ai6ctl` for the terminal.
- **Cross-platform terminal** - the `pty` provider uses openpty on Unix and ConPTY with
  Job Objects on Windows.

## Installation

AI6 is built from source (Rust 1.89 or later):

```bash
git clone https://github.com/jose-pr/ai6.git
cd ai6
cargo build --workspace --all-features
```

The Moneypenny app needs the Flutter `main` SDK; see
[its README](https://github.com/jose-pr/ai6/blob/main/clients/moneypenny/README.md).

## Quick start

`examples/config` registers Operation `G` (this repository) and Agents `007`, its backup
`008`, and `Tanner`. Start a Station with it; the Claude handler needs Claude Code on
`PATH`:

```bash
cargo run -p ai6 --features cli -- serve \
  --config-dir ./examples/config \
  --plugin-path ./target/debug
```

Submit a Mission to the Station's private API from another terminal:

```bash
curl -f -X POST http://127.0.0.1:7081/internal/missions \
  -H 'content-type: application/json' \
  -d '{"mission_id":"mission-quickstart","operation_id":"G","prompt":"Summarize README.md","role":"case-officer"}'
```

For a clickable board with sample data and no gateway, run the Moneypenny web preview
from `clients/moneypenny`:

```bash
flutter run -d web-server --web-hostname 127.0.0.1 --web-port 7357 --release \
  --dart-define=MONEYPENNY_DEMO=true
```

## Command line

```text
ai6 serve [--listen ADDRESS] [--config-dir PATH] [--plugin-path PATH]...
          [--station ID] [--role control|execution]... [--label KEY=VALUE]...
          [--terminal ID]
ai6 identity
ai6 peer waiting
ai6 peer accept FINGERPRINT

universal-exports --public-origin HTTPS_ORIGIN [--port PORT] [--plugin-path PATH]...

ai6ctl health
ai6ctl stations
ai6ctl station STATION_ID
ai6ctl agent-roles --operation OPERATION
ai6ctl missions [--operation OPERATION]
ai6ctl mission MISSION_ID [--operation OPERATION]
ai6ctl sessions [--operation OPERATION] [--mission MISSION_ID] [--state STATE]
ai6ctl session SESSION_ID
```

`ai6ctl` reads `AI6_EXPORTS_URL` for the Universal Exports origin and a user bearer from
`AI6_ACCESS_TOKEN`. A Station publishes to Universal Exports when `AI6_EXPORTS_URL` and
`AI6_EXPORTS_STATION_TOKEN` are both set.

## API overview

| Crate or module | Purpose |
| --- | --- |
| `ai6::directorates::m` | Roster (Operations, Agents, roles), Missions, identities, cover, access kinds |
| `ai6::directorates::registry` | The Registry (briefs, leases, check-ins, audit) and the Cable relay |
| `ai6::directorates::q` | Terminals, loadouts, secret leases, MCP config, sandbox arguments |
| `ai6::directorates::section_00` | The AgentHandler contract |
| `ai6::directorates::medical` | The Session watchdog |
| `ai6::station` | The Station: server, provider host, safe houses, publisher, mTLS identity |
| `universal-exports` | The public API gateway: authentication, authorization, attachment, audit |
| `ai6-diptel` | The wire contract for Session lifecycle and terminal frames |
| `ai6-agent-claude`, `ai6-agent-codex` | AgentHandler providers |
| `ai6-terminal-pty` | The `pty` terminal provider |
| `coqui-access`, `coqui-roles` (shared) | Access kinds, the role authorizer and the access providers AI6 loads |
| `coqui-peer-tls` (shared) | Station mutual TLS |

## Architecture

The work and the infrastructure are separate shapes:

```text
Work            Operation ──► Mission ──► Session   (provider-issued Session ID)
Infrastructure  Station ──────────────────► Theater   (where the Session runs)
```

An **Operation** is a global project and its policy; Stations serve it, none owns it. A
**Mission** is one brief; a **Session** is one resumable provider run. A **Station** is a
node running `ai6 serve`; a **Theater** is the host a Session works on, reached through a
transport (`local`, `ssh`). The directorates (M, the Registry, Q Branch, 00 Section, Medical) decide; the
Station runs their decisions; Universal Exports is the only public door. See
[Architecture](https://jose-pr.github.io/ai6/architecture/) and
[Organization](https://jose-pr.github.io/ai6/organization/). Field mode on a remote Theater and
a SQL Registry are not implemented yet.

```toml
[operation.G]
name = "AI6"
path = "."
roster = ["007", "008", "Tanner"]

[agent.007]
handler = "claude"
model = "sonnet"
effort = "high"
roles = ["case-officer", "analyst"]
priority = 20

[role.case-officer]
purpose = "Own a bounded Mission and coordinate its delivery within the Operation."
policy = "Follow the Mission brief and Operation rules; request authorization for restricted actions."
```

## Development

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo clippy -p ai6 --no-default-features -- -D warnings
cargo test --workspace --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features
pwsh -File tools/public-api.ps1 -Mode check

cd clients/moneypenny
flutter analyze --fatal-infos
flutter test
```

The [test workflow](https://github.com/jose-pr/ai6/blob/main/.github/workflows/test.yml)
runs these on Linux, macOS and Windows with stable Rust and 1.89.

### Releasing

A `v*` tag runs the test matrix and a strict documentation build, then creates a source
release from the tagged revision and dispatches the documentation workflow. A `ci-*` tag
runs the tests without publishing.

## License

Mozilla Public License 2.0. See [LICENSE](https://github.com/jose-pr/ai6/blob/main/LICENSE).
