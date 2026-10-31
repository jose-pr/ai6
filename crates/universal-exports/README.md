# universal-exports

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/jose-pr/ai6/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/jose-pr/ai6/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/jose-pr/ai6/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://jose-pr.github.io/ai6/rust/universal_exports/)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/ai6/test.yml)](https://github.com/jose-pr/ai6/actions/workflows/test.yml)

Universal Exports is AI6's public front: the authenticated HTTP and WebSocket API that the
Outer Office app and `ai6ctl` reach instead of a Station. **M retains authority
over Operation access; the gateway enforces the scoped grants M issues.** See the
[crate API header](https://github.com/jose-pr/ai6/blob/main/crates/universal-exports/AGENTS.md).

## Features

| Feature | Adds | Default |
| --- | --- | --- |
| (none) | Authenticated Station channel, Operation/Mission/Session binding, bounded replay/fanout, write leases, HTTPS origin checks and loopback serving. | Yes |
| `cli` | The `universal-exports` service command. | No |

Terminal bytes remain binary and are redacted from debug output.

## Installation

This crate is part of the AI6 workspace and is not published separately. From the
repository root, add it as a path dependency:

```console
cargo add universal-exports --path crates/universal-exports
```

The library has no default features. Enable `cli` to build the `universal-exports`
service command.

## Quick start

```rust
use std::sync::Arc;
use std::time::Duration;
use universal_exports::{Gateway, GatewayConfig, GatewayLimits};
use ai6::FileRegistry;

let config = GatewayConfig::new(
    "https://localhost",
    "a-long-random-token-loaded-from-the-Q-credential-lease",
    GatewayLimits::default(),
    Duration::from_secs(60),
    Duration::from_secs(30),
)?;
let registry = FileRegistry::open("/var/lib/ai6/registry");
let _app = Gateway::with_registry(config, Arc::new(registry)).router();
# Ok::<(), Box<dyn std::error::Error>>(())
```

For a deployed listener, run `universal-exports` behind an HTTPS reverse proxy. It binds
to loopback, and the proxy must remove client-supplied `X-Forwarded-Proto` and set
it to `https`; browser endpoints require that marker and an exact `Origin` match.
The Station credential is accepted only through its environment variable, never as an
argument.

## Command line

`universal-exports` runs the gateway; the operator command-line client is the separate
`ai6ctl` crate.

```console
universal-exports --config /etc/ai6/unex.cfg
```

What it needs before it reaches the configuration provider is in its bootstrap file,
`unex.cfg` ([Configuration](https://jose-pr.github.io/ai6/configuration/)): its loopback listener, the public
origin, the Stations' token and the reverse proxy's secret as secret references, and the
provider, Registry and provider libraries. Everything else is a record of the provider the
Stations read: the `unex` record holds sign-in, login limits, the access cache and the
assistant, and the `access` record the role policy, which applies without a restart.

User access requires a Directory and an authorization policy. Login can use a Guatiao
Authenticator or an identity asserted by the trusted reverse proxy. Providers are the
loadable libraries of [`coqui-access`](https://github.com/Boriken-Dev/coqui-access)
(`--plugin-path` or `[plugins] paths` names the directory they are in), checked against the
provider's `plugin` records as a Station checks its own. Each provider's configuration is a
JSON object in the `unex` record, validated against its Guatiao schema; password accounts use
the same configuration for both provider kinds:

```toml
# unex.toml, the unex record of a configuration directory
authenticator = "coqui_access_password"
directory = "coqui_access_password"
authenticator_config = { source = { file = { path = "accounts.toml", format = "toml" } } }
directory_config = { source = { file = { path = "accounts.toml", format = "toml" } } }
```

A configuration is limited to 64 nested values. A credential written inline in one is
reported as a weakening: name a file instead.

For an existing authenticated reverse proxy, configure the same identity header it already
asserts upstream:

```toml
# unex.toml
directory = "coqui_access_password"
directory_config = { source = { file = { path = "C:/secure/accounts.toml", format = "toml" } } }
proxy_user_header = "X-Forwarded-User"
proxy_lookup_by = "name"
```

The reverse proxy must overwrite `X-Forwarded-User` after authenticating the browser and
remove any client-supplied value, and set `X-AI6-Proxy-Secret` to `unex.cfg`'s
`proxy_secret` on every request it forwards: an asserted identity without that secret is
refused, since anything reaching the loopback port directly (another local process) can set
the identity header too. The Directory maps that asserted name to the authoritative person
record; authorization still comes from the access policy.

## API overview

| Item | Purpose |
| --- | --- |
| `GatewayConfig`, `GatewayLimits` | HTTPS origin, Station credential, TTLs and bounded resource policy. |
| `Gateway` | M registration, typed lifecycle/terminal publication, board snapshots, shared access-token issue/verify/revoke, raw guatiao ingress, browser attach and input queue. |
| `AttachGrant`, `AttachMode` | Station/Mission/Session-scoped read or write authorization. |
| `Attachment`, `AgentInput` | Bounded viewer replay/live stream and queued terminal input or resize. |
| `GatewayError` | Refusals for auth, scope, bounds, malformed frames and backpressure. |
| `ControlStatus` | One operator control (cancel, retry, resume, interrupt) and what the Station did with it. |
| `TimelinePage`, `TimelineItem` | One page of an Operation's correlated, metadata-only activity timeline. |
| `Gateway::router` | Axum routes for the Station channel, attach token exchange and WebSocket sessions. |
| `Gateway::serve_loopback(port, shutdown)` | Loopback listener with caller-owned graceful shutdown. |

### HTTP routes

All `/v1/station/` paths require the Station bearer credential.

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/health` | Return `204 No Content` while the gateway is serving. |
| `GET` | `/v1/board` | Return Sessions for Operations permitted by the user's `mission.read` policy. |
| `GET` | `/v1/stations` | Return authorized Station health, hosted Operations, the Theaters each Station reaches (with the transport's capabilities and the Theater's declared facts), and the current Headquarters Station. |
| `GET` | `/v1/user/operations/{operation_id}/placement-preview` | Require `mission.read`; return where a Mission for the Operation would run (Station, Theater, Agent, best first) and a reason for every place ruled out. Optional `agent`, `role`, `stations` and `tags` narrow it. |
| `GET` | `/v1/missions` | Group registered provider Sessions by Mission and apply `mission.read` to each Operation. |
| `GET` | `/v1/operations/{operation_id}/missions/{mission_id}` | Return one authorized Mission and its AI6-published brief and sanitized launch manifest. |
| `GET` | `/v1/auth/session` | Validate the bearer access token against the shared Registry and return its person and expiry. |
| `POST` | `/v1/station/access-tokens` | Issue a person-bound access token after M authenticates the user; the raw token is returned once. Body: `{"person_id":"…","ttl_seconds":600}`. |
| `DELETE` | `/v1/station/access-tokens/{token_id}` | Revoke a token across gateway replicas sharing the Registry. |
| `POST` | `/v1/station/sessions` | Register an Operation, Mission ID and provider Session ID, optionally with the authoritative Mission brief and sanitized launch manifest; returns `{"next_sequence": N}`. |
| `POST` | `/v1/station/sessions/{session_id}/lifecycle/{sequence}` | Publish JSON lifecycle state; the gateway encodes the guatiao event. |
| `POST` | `/v1/station/sessions/{session_id}/terminal/{sequence}` | Publish raw terminal bytes; the gateway encodes the guatiao event. |
| `POST` | `/v1/station/sessions/{session_id}/events/{sequence}` | Ingest one guatiao frame. Schema/close frames use cursor `0`; values use a positive event sequence. |
| `POST` | `/v1/station/attach-grants` | Issue a read or write token within M's Station grant. |
| `GET` | `/v1/auth/methods` | Say what `/v1/auth/login` accepts here, with no token: `{"methods": [...]}`, `password` for a name and a secret, `proxy` for an identity the reverse proxy asserts. |
| `POST` | `/v1/auth/login` | Resolve the trusted proxy identity, or verify credentials with the configured Authenticator; returns one user access token. |
| `POST` | `/v1/auth/logout` | Revoke the presented user token across gateways sharing the Registry. |
| `GET` | `/v1/user/profile` | Return directory identity and effective `mission.read`, `mission.create`, `mission.control`, `approval.decide`, `approval.class`, `session.read`, `session.input`, `audit.read`, `agent.read`, `agent.configure`, `pool.read`, `pool.manage`, `cable.send` and `cable.read` actions by authorized Operation. |
| `GET` | `/v1/user/operations/{operation_id}/green-ink` | Require `approval.decide`; list the steps Agents wait to take, oldest first. |
| `POST` | `/v1/user/operations/{operation_id}/green-ink/{request_id}/approve` | Require `approval.decide`; sign the step as the caller for `expires_in_secs` (default one hour) with an optional `note`. |
| `POST` | `/v1/user/operations/{operation_id}/green-ink/{request_id}/reject` | Require `approval.decide`; refuse the step, with an optional `reason` the Agent is told. |
| `POST` | `/v1/user/operations/{operation_id}/green-ink/{request_id}/revoke` | Require `approval.decide`; withdraw an approval before it expires. |
| `GET`, `POST` | `/v1/user/operations/{operation_id}/class-authorizations` | Require `approval.class`; list, or grant as the caller, standing approvals of a kind of step on a target glob for named Agents or roles. |
| `DELETE` | `/v1/user/operations/{operation_id}/class-authorizations/{id}` | Require `approval.class`; revoke one. |
| `POST` | `/v1/user/operations/{operation_id}/missions/{mission_id}/actions` | Require `mission.control`; audit and queue a cancel, retry, resume or interrupt for the Station holding the Mission. Body: `{"action":"retry","agent":"008","idempotency_key":"…"}` (`agent` only with `retry`, `message` only with `resume`, optional `session_id`). Returns `202` and a `ControlStatus`, or `200` and the first one for a repeated key. |
| `GET` | `/v1/user/operations/{operation_id}/missions/{mission_id}/actions/{control_id}` | Require `mission.read`; return the control's status: `queued`, `delivered`, `applied`, `rejected` or `expired`, with the Mission state and reason the Station reported. |
| `GET` | `/v1/user/audit?operation_id=<id>&limit=<n>` | Return up to 500 newest metadata-only audit records authorized by `audit.read` for each Operation. |
| `GET` | `/v1/user/operations/{operation_id}/timeline?cursor=<c>&mission=<id>&limit=<n>` | Require `audit.read`; return one newest-first page (default 50, at most 200) of the Operation's lifecycle transitions, control outcomes, Cables sent, Sleeper authorizations and audit records, correlated by Station, Mission and Session, with `next_cursor` for the older page. |
| `GET` | `/v1/user/operations/{operation_id}/pools` | Require `pool.read`; return the pool accounts the Operation may draw from, with the Sessions using each, quota windows, limited-until and tokens today; never a credential. |
| `POST` | `/v1/user/operations/{operation_id}/pools/{pool}/{account}/clear-limit` | Require `pool.manage`; lift an account's provider limit, audited. |
| `GET` | `/v1/user/roster/{kind}` | Require `roster.read` on `config:roster`; list the roster's `operations`, `agents`, `roles`, `covers` or `assistant` (Moneypenny's settings, one record keyed `assistant`) with the revision and whether it takes edits. |
| `PUT`, `DELETE` | `/v1/user/roster/{kind}/{key}` | Require `roster.manage` on `config:roster`; store (a JSON object) or remove one record through the roster source. The whole roster is checked first: `422` with the reason when it would not be acceptable, `409` for a read-only source. An accepted `assistant` record answers with the settings it loosens (`weakened`). Accepted and refused edits are audited. |
| `GET` | `/v1/user/operations/{operation_id}/agent-roles` | Require `agent.read`; return the Headquarters Station role catalog merged with this Operation's validated overrides. Each role has `operation_override` to identify whether it replaces the Station definition. |
| `PUT` | `/v1/user/operations/{operation_id}/agent-roles/{role_id}` | Require `agent.configure`; validate and store a role purpose and policy in the shared Registry. |
| `DELETE` | `/v1/user/operations/{operation_id}/agent-roles/{role_id}` | Require `agent.configure`; remove the Operation override, fall back to the Station definition and record a metadata-only audit event. |
| `POST` | `/v1/user/attach-grants` | Issue a Session attach token only when `session.read` or `session.input` is allowed at that Station. |
| `POST` | `/v1/user/operations/{operation_id}/missions` | Require `mission.create` for the Operation and atomically enqueue the bounded title and explicit brief in the shared Registry. |
| `GET` | `/v1/user/operations/{operation_id}/mission-proposals` | Require `mission.create`; list bounded pending Agent proposals for review in that Operation, each with its `vetting` once M has vetted it. |
| `POST` | `/v1/user/operations/{operation_id}/mission-proposals/{proposal_id}/approve` | Require `mission.create`; record the approving operator and promote the proposal into scheduler-visible intake, cleared of every vetting finding but `malformed`. |
| `POST` | `/v1/user/operations/{operation_id}/mission-proposals/{proposal_id}/reject` | Require `mission.create`; record the rejecting operator and file the proposal outside runnable intake. |
| `GET` | `/v1/user/operations/{operation_id}/held-briefs` | Require `mission.create`; list the briefs vetting held, with what it found. |
| `POST` | `/v1/user/operations/{operation_id}/held-briefs/{key}/release` | Require `mission.create`; return the brief to the queue, cleared of the kinds it was held for. |
| `POST` | `/v1/user/operations/{operation_id}/held-briefs/{key}/refuse` | Require `mission.create`; file the brief as refused, with an optional `reason` for the record. |
| `GET` | `/v1/auth/session` | Validate a user bearer token and return its token ID, person ID and expiry. |
| `GET` | `/v1/station/inputs/next` | Wait for one bounded binary terminal input or resize frame for M. |
| `GET` | `/v1/station/controls/next?station=<id>&wait_ms=<n>` | Wait up to `wait_ms` (default 1000, at most 25000) for the next control queued for that Station; `200` with one DipTel control value frame (channel 4) or `204`. |
| `POST` | `/v1/station/controls/{control_id}/result` | Report a taken control's outcome: `{"station":"…","outcome":"applied"\|"rejected","state":"…","reason":"…"}`; `403` from another Station, `409` once finished. |
| `POST` | `/v1/attach` | Exchange a token in the request body for a one-connection cookie. |
| `GET` | `/v1/socket/{session_id}?after={sequence}` | Replay and stream events for one provider Session. |

`Gateway::with_access` injects identity and authorization providers. Configure
`--directory` and `--proxy-user-header` to accept identity from the existing
authenticated reverse proxy without a second password prompt. The proxy must
remove any client-supplied value for that header and set it only after successful
authentication. The gateway resolves its value through the configured Directory
using `--proxy-lookup-by` (default `name`), then applies the same authorization
policy as password login. A comma-separated identity, unknown Directory identity,
or absent proxy identity is rejected. For local desktop sign-in, an Authenticator
can be configured alongside the proxy identity or used on its own. Browser
`/v1/auth/login` requests must pass through the configured same-origin HTTPS proxy;
the native desktop client can reach the loopback listener without an Origin header.
Password and key tries are limited: before an Authenticator is asked, a try is counted against
the client address and against the account name, and one past either count is refused with
`429` and a `Retry-After`, whether or not its secret is right. The defaults are 5 a minute
per address and 10 in ten minutes per name; the `unex` record's `login_tries_per_address`,
`login_tries_per_name` and `client_address_header` change them. Behind the reverse proxy
every connection comes from the proxy, so name the header it fills with the client address
(`x-forwarded-for` or `x-real-ip`) or every client shares one count.
Successful login returns a short-lived user bearer token. Password secrets are
never logged or persisted. `/v1/auth/logout`
revokes that token in the shared Registry. The Registry stores only a domain-separated
SHA-256 digest, person ID, issue/expiry times, and revocation time. Token secrets are
returned only once. `/v1/board` and `/v1/missions` apply `mission.read` per Operation;
user attach grants apply `session.read` or `session.input` to the exact registered Operation and
Session tuple. A missing policy rule denies access. Provider calls run on Tokio's
blocking pool. `Gateway::with_registry` enables M-issued access tokens but leaves
user login and authorization endpoints unavailable. The `universal-exports` command accepts
provider IDs and an access-policy path to enable user access; it uses the Registry
`unex.cfg` names: a directory, `sqlite:PATH` or a `postgres://` URL
(`ai6::directorates::registry::backend::open_with`).

A control goes to the Station holding the Mission's latest Session record, else the one
holding its brief's lease, else the Headquarters Station, passing over Stations gone dark;
with none, the request is refused with `503` and `Retry-After`. The Station decides
whether the Mission can take it. Controls live in gateway memory: at most 1,024 are kept
(the oldest finished go first), 64 may wait per Station, and one no Station finishes
within two minutes is `expired`. A resume's message reaches only the Station; it is not in
the status or the audit record.

The timeline merges two durable Registry sources: activity events (the lifecycle
transitions and control outcomes this gateway records, Cables and Sleeper approvals the
Stations record) and audit records, including the records of each Station hosting the
Operation when the caller also has `audit.read` on that Station. Items hold identities,
names and states only: never prompts, resume messages, Cable bodies or targets, terminal
bytes or secrets. A Cable appears only in its sender's Operation. The cursor is opaque;
newer activity arriving between pages does not shift or repeat older items.

The gateway refuses request bodies and websocket frames above the configured
limit. Its configured history, fanout, input queue and per-viewer socket buffers
cannot exceed a combined budget (`[gateway] buffer_budget_bytes`, 512 MiB).

`GET /v1/board` returns sorted `BoardSession` records. `GET /v1/missions` returns
sorted `BoardMission` records with their registered Sessions and published Mission
briefs; `GET /v1/operations/{operation_id}/missions/{mission_id}` returns one authorized
Mission. All require a user bearer and filter by `mission.read` for each Operation.
`GET /v1/stations` derives Station check-ins and hosted Operations from the shared I
Registry; stale Stations remain visible as offline after 45 seconds, and the Headquarters
marker follows only an unexpired elected lease. Host and process metadata are omitted.
The brief includes the exact prompt and launch metadata, but only environment variable
names; secret values and initial context are never sent to the gateway. Newly registered Sessions
have a null state and sequence zero until M publishes their first lifecycle event.
Queued Missions without a registered Session are not included; creation requires
`mission.create` on the Operation and returns only the queued Mission ID. These endpoints do
not expose terminal data or credentials. Put the public gateway behind the deployment's HTTPS reverse proxy;
it must remove client-supplied `X-Forwarded-Proto` and set it to `https`.

## Bootstrap file

The command's bootstrap file is `unex.cfg`: `--config PATH`, else the file `UNEX_CONFIG`
names, else `unex.cfg` in the user's then the system's configuration directory (as `ai6.cfg`,
`ai6::bootstrap::UnexBootstrap`). Unknown keys are refused; relative paths are relative to
the file; a secret is `{ file = "PATH" }`, `{ env = "NAME" }` or the value itself, and the
last two are printed as a warning at start.

| Key | Effect | Default |
| --- | --- | --- |
| `listen` | Loopback address behind the HTTPS reverse proxy (`127.0.0.1` only); `--listen`. | `127.0.0.1:7082` |
| `public_origin` | Required browser HTTPS origin; `--public-origin`. | None |
| `station_token` | Required secret: the Stations' channel bearer credential. | None |
| `proxy_secret` | Secret the reverse proxy sends in `X-AI6-Proxy-Secret`; required with a proxy identity header, at least 32 characters. | None |
| `state_dir` | Where verified provider libraries, and a default Registry, are kept. | The platform's state directory |
| `[provider] location`, `password`, `poll` | The configuration provider Stations read; its roster is served under `/v1/user/roster` (a directory read-only, SQL editable); `--provider`. | The file's directory |
| `[registry] location`, `password`, `headquarters_lease` | Shared Registry for issued access-token records; `--registry`. | The provider's database when SQL, else `registry` in `state_dir` |
| `[plugins] paths`, `profile` | Provider libraries and how they are checked; `--plugin-path` adds, `--plugin-profile` overrides. | None; strict with a pin set, report without |

## Environment variables

| Variable | Purpose | Default |
| --- | --- | --- |
| `UNEX_CONFIG` | The bootstrap file; must exist when set. | `unex.cfg` in the user's, then the system's configuration directory |

Secret references in `unex.cfg` may name others (`{ env = "NAME" }`), which is reported as a
weakening. The `unex` record's keys are listed in [Configuration](https://jose-pr.github.io/ai6/configuration/).

## Exit status

`universal-exports` returns `0` after graceful shutdown, `1` when configuration or
listener startup fails, and `2` for invalid command-line arguments.

## Development

From the repository root, run:

```console
cargo fmt --all --check
cargo test -p universal-exports --all-features
cargo clippy -p universal-exports --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc -p universal-exports --no-deps --all-features
cargo run -p universal-exports --features cli -- --help
```

Gateway integration tests bind only to loopback.

### Releasing

`universal-exports` is an internal workspace crate (`publish = false`). Update
its version with the workspace package when an AI6 release requires it.

## License

MPL-2.0. See [LICENSE](https://github.com/jose-pr/ai6/blob/main/LICENSE).
