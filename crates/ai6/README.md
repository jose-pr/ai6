# ai6

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/jose-pr/ai6/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/jose-pr/ai6/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/jose-pr/ai6/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://jose-pr.github.io/ai6/rust/ai6/)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/ai6/test.yml)](https://github.com/jose-pr/ai6/actions/workflows/test.yml)

The AI6 Station daemon and its public contracts, as a Rust library and the `ai6`
command. **Directorates decide (`ai6::directorates`); the Station runs their decisions
(`ai6::station`)**: it schedules Missions, starts Agent Sessions through Guatiao
providers, keeps their state across restarts and publishes them to Universal Exports.
See the [API header](https://github.com/jose-pr/ai6/blob/main/crates/ai6/AGENTS.md)
and the [guides](https://jose-pr.github.io/ai6/).

## Features

- AgentHandler, terminal, Source and access contracts as Guatiao kinds, usable without the daemon.
- A Station daemon: private loopback API, Registry-leased intake, Mission scheduling,
  durable Mission records and automatic provider-Session reattachment after a restart.
- Agent selection by role and priority; when an Agent's handler is unavailable, the next
  qualified Agent is posted instead.
- Isolated Mission workspaces (safe houses, made by providers such as `git`), commit cover audits and Session watchdogs.
- Scoped agent-to-agent Cables with per-Session MCP tools.
- Station identities with pinned-peer or CA mutual TLS.

## Installation

This crate is part of the AI6 workspace and is not published separately:

```console
cargo add ai6 --git https://github.com/jose-pr/ai6
```

| Feature | Adds | Default |
| --- | --- | --- |
| (none) | Contracts: AgentHandler, terminal, access, roster, Mission, Registry, Cables, Q loadouts. | Yes |
| `daemon` | The Station's private HTTP service, scheduler, Guatiao provider host and watchdogs. | No |
| `publisher` | Bounded publication of lifecycle events and raw terminal bytes to Universal Exports. | No |
| `cables-mcp` | Per-Session authenticated MCP tools for Agent-to-Agent Cables. | No |
| `station-tls` | Persistent Station certificate identities and pinned-peer or CA mutual TLS, on `coqui-peer-tls`. | No |
| `cli` | The `ai6` command with all of the above and Ctrl-C handling. | No |

## Quick start

```console
cargo build --workspace --all-features
cargo run -p ai6 --features cli -- serve --config-dir ./examples/config --plugin-path ./target/debug
```

The example configuration registers Operation `G` and Agents `007`, `008` (its backup)
and `Tanner`.

## Command line

```console
ai6 serve [--listen 127.0.0.1:7081] [--config-dir PATH] [--plugin-path PATH]...
          [--station ID] [--role control|execution]... [--label KEY=VALUE]...
          [--terminal ID] [--max-concurrent-missions N] [watchdog options]
ai6 identity
ai6 peer waiting
ai6 peer accept FINGERPRINT
ai6 sleeper [--config-dir PATH] list
ai6 sleeper [--config-dir PATH] authorize OPERATION NAME
ai6 sleeper [--config-dir PATH] revoke OPERATION NAME
```

- `ai6 serve` runs the Station. Harnesses run in an available terminal provider,
  preferring `pty`; `--terminal` pins one. Agents and handlers do not choose it. `--role` and `--label` configure placement;
  both roles are on by default. Watchdog defaults are set with `--stall-timeout`,
  `--loop-timeout`, `--loop-min-lines`, `--blocked-after`, `--max-runtime`, `--history`
  and `--normalize-digits` (seconds may be fractional).
- `ai6 identity` creates or prints this Station's certificate fingerprint.
- `ai6 peer waiting` lists certificates a peer-trust listener rejected; `ai6 peer accept`
  pins one of them.
- `ai6 sleeper` lists an Operation's standing orders and authorizes or revokes them; a
  sleeper queues briefs on its schedule only while its current definition is authorized.

Exit status is `0` on success and `1` on a configuration, listener or identity failure.

## API overview

| Path | Purpose |
| --- | --- |
| `ai6::directorates::m::roster` | Operations, Agents (inheritance, aliases, clearance, priority) and Agent roles |
| `ai6::directorates::m::theaters` | Theaters (`theaters.toml`): the hosts a Station reaches, local or over SSH |
| `ai6::directorates::m::placement` | Placement: eligible (Station, Theater, Agent) choices with a reason for every rejection |
| `ai6::directorates::m::mission` | Durable Mission lifecycle, provider Session association and resume settings |
| `ai6::directorates::m::limits` | `OnLimit` and the parking rule for provider usage limits |
| `ai6::directorates::m::identity` | Validated `MissionId`, provider-issued `SessionId` and access-token records |
| `ai6::directorates::m::cover` | Built-in and TOML cover policies, Agent briefings and commit audits |
| `ai6::directorates::m::access` | The shared `coqui-access` kinds and `coqui-roles` authorizer, AI6's scope kinds and actions, and the `access.toml` reader |
| `ai6::directorates::registry::store` | The Registry: briefs, leases, Station check-ins, Headquarters election, audit and role overrides |
| `ai6::directorates::registry::sessions` | Shared, fenced Session records: lease, fencing token, attempt and predecessor |
| `ai6::directorates::registry::sleepers` | Sleepers: cron standing orders, their authorization and per-window briefs |
| `ai6::directorates::registry::sources` | The Source kind (trackers and CI), brief ingestion, cursors and CX reports |
| `ai6::directorates::registry::cables` | Permission-scoped Agent-to-Agent Cable relay |
| `ai6::directorates::q` | Loadouts, Mission-scoped secret leases, MCP config and sandbox arguments |
| `ai6::directorates::q::terminal` | Terminal and terminal-session kinds and the Session bridge |
| `ai6::directorates::section_00::agent` | AgentHandler, AgentSession and provider-neutral events |
| `ai6::directorates::medical::watchdog` | Stall, loop and blocked-prompt detection for Sessions |
| `ai6::station::server` | `ServeConfig`, `StationConfig` and the `serve*` entry points |
| `ai6::station::plugins` | Guatiao provider discovery and typed construction |
| `ai6::station::safehouse` | Opens a Mission's safe house through a provider; state directory |
| `ai6::directorates::q::safehouse` | The safe-house provider contract and its models |
| `ai6::station::publisher` | The Universal Exports publisher (`publisher` feature) |
| `ai6::station::identity` | Station certificates, fingerprints and peer trust (`station-tls` feature) |

## Station service

The loopback service lists providers (`GET /internal/agent-handlers`, `/internal/terminals`,
`/internal/plugins`), creates and inspects Missions (`/internal/missions`), and starts,
messages, interrupts and resumes them. A start resolves the Operation and Agent, then runs
the Agent's handler in the Station's terminal provider. Mission records persist atomically
in `missions.json` under the state directory; on startup, Missions that were working
become blocked and the scheduler reattaches them through their provider Session ID.
Graceful shutdown interrupts active Sessions and keeps their safe houses for recovery.
Each Session also has a record in the shared Registry, held by a lease whose fencing token
refuses a Station that lost it. When a provider usage limit stops a Session the Mission is
parked until the quota resets and resumes the same provider Session; an Operation may instead
set `on_limit = "backup"` to post the next qualified Agent as a new attempt.

The scheduler leases briefs from the file-backed Registry, uses each brief's
`source--id` key as the Mission ID, selects an Agent by role, tags and priority among
those whose handler is available, and files outcomes back. Station and Agent
concurrency limits count leases across the shared Registry. The Station checks in its
roles, labels, capacity, active work, local Operation checkouts, handlers, reachable Theaters
and each Operation's placement policy every 15 seconds. Placement chooses the Station,
Theater and Agent and can say why it ruled the others out.

Each Mission snapshots its Operation's cover policy and starting commit; at completion AI6
audits commit authors and messages and attaches a `cover` report. A violation marks the
Mission `blown` when the Operation sets `on_blown = "fail"`. With `cables-mcp`, Claude and
Codex Sessions receive a per-Session MCP endpoint with a random bearer token that is never
persisted or passed as an argument. With `publisher` and both `AI6_EXPORTS_URL` and
`AI6_EXPORTS_STATION_TOKEN` set, the Station registers Sessions with Universal Exports, publishes
lifecycle and terminal output through a bounded queue, and dispatches authorized terminal
input back to the live Session; its state is at `GET /internal/publisher`.

## Access policy

`RoleAuthorizer` evaluates first-match rules from `access.toml`. A rule binds
`account:<name>`, `person:<id>` or `group:<name>` to a role at `station:<id>`,
`operation:<id>`, `theater:<id>` or `*`; an absent match or action denies.

```toml
[role.viewer]
actions = ["mission.read"]

[role.operator]
actions = ["mission.read", "mission.create", "session.input"]

[[rule]]
subject = "person:person-007"
scope = "operation:G"
role = "operator"

[[rule]]
subject = "group:field"
scope = "operation:G"
role = "viewer"
```

`ai6::role_authorizer_from_toml` reads this format into a `coqui-roles` `RoleAuthorizer` that
accepts only AI6's scope kinds. The same authorizer is the `coqui_roles` Guatiao provider
(configured with `scope_kinds`, `roles` and `rules`). Authentication and directories come from
the [`coqui-access`](https://github.com/Boriken-Dev/coqui-access) provider libraries
(`coqui_access_local`, `_key`, `_ldap`, `_proxy` and `_peer`), which a Station or Universal
Exports loads from `--plugin-path`.

## Development

From the repository root:

```console
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo clippy -p ai6 --no-default-features -- -D warnings
cargo test --workspace --all-features
```

### Releasing

`ai6` is versioned with the workspace (`publish = false`); releases are cut from the
repository with a `v*` tag.

## License

MPL-2.0. See [LICENSE](https://github.com/jose-pr/ai6/blob/main/LICENSE).
