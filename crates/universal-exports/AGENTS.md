# `universal-exports` — API header

Header-file-style reference for the `universal-exports` crate: every public
item with its signature, contract and gotchas, so the crate can be used
without reading its source. It ships inside the package and is self-contained.
Development documentation lives with the source at
<https://github.com/jose-pr/ai6>.

## Features, MSRV and dependencies

| Feature | Adds | Default |
| --- | --- | --- |
| (none) | Axum HTTP routes and bounded gateway state. | Yes |
| `cli` | The `universal-exports` service command and Guatiao provider loading. | No |

MSRV: Rust 1.89. The default build depends on Axum, Tokio, guatiao,
`ai6-diptel`, serde and UUID. The `cli` feature adds clap and Tokio signal support.

## Imports and modules

Every public item is imported from the crate root. The crate has no public
modules. There are no feature-gated items.

## Unsafe code

The crate forbids unsafe code with `#![forbid(unsafe_code)]`.

## Limits and construction

```rust
pub struct GatewayLimits {
    pub max_sessions: usize,
    pub max_clients_per_session: usize,
    pub scrollback_events: usize,
    pub max_frame_bytes: usize,
    pub max_input_frames_per_second: u32,
    pub input_queue_capacity: usize,
    pub fanout_capacity: usize,
}

pub struct GatewayConfig { /* fields are private */ }
impl GatewayConfig {
    pub fn new(
        public_origin: impl Into<String>,
        m_token: impl Into<String>,
        limits: GatewayLimits,
        attach_token_ttl: Duration,
        write_lease_ttl: Duration,
    ) -> Result<Self, GatewayError>;
    pub fn public_origin(&self) -> &str;
}
```

`GatewayLimits::default()` allows 16 Sessions, 12 clients per Session, 64
scrollback events, 256 KiB per frame, 40 input frames per second, 64 queued
inputs and 8 fanout events. Config validation requires an HTTPS origin, a
32-byte minimum M credential, finite non-zero bounds, TTLs no longer than 300
seconds, at most 100,000 client slots, and a maximum aggregate 512 MiB budget
for history, fanout, input and socket write buffers. `GatewayConfig` formats its
M credential as `[REDACTED]`.

## Authorization types

```rust
pub enum AttachMode { Read, Write }
pub struct AttachGrant {
    pub operation_id: OperationId,
    pub mission_id: MissionId,
    pub session_id: SessionId,
    pub mode: AttachMode,
}

pub struct AccessProviders { /* optional Authenticator, Directory, Authorizer and proxy identity */ }
impl AccessProviders {
    pub fn new(
        authenticator: std::sync::Arc<dyn ai6::Authenticator>,
        directory: std::sync::Arc<dyn ai6::Directory>,
        authorizer: std::sync::Arc<dyn ai6::Authorizer>,
    ) -> Self;
    pub fn from_reverse_proxy(
        directory: std::sync::Arc<dyn ai6::Directory>,
        authorizer: std::sync::Arc<dyn ai6::Authorizer>,
        header: axum::http::HeaderName,
        lookup_by: impl Into<String>,
    ) -> Self;
    pub fn with_proxy_identity(
        self,
        header: axum::http::HeaderName,
        lookup_by: impl Into<String>,
    ) -> Self;
}
```

All fields are public; `AttachMode` is non-exhaustive. M is the authority that
chooses the Operation, Mission ID, Session ID and mode. The gateway refuses a
grant if its Operation or Mission differs from the registered Session.

## Board models

```rust
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoardSession {
    pub operation_id: String,
    pub mission_id: String,
    pub session_id: String,
    pub state: Option<String>,
    pub sequence: u64,
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoardMission {
    pub operation_id: String,
    pub mission_id: String,
    pub sessions: Vec<BoardSession>,
    pub brief: Option<MissionBrief>,
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StationBoard {
    pub stations: Vec<BoardStation>,
    pub headquarters_station: Option<String>,
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BoardStation {
    pub station_id: String,
    pub healthy: bool,
    pub roles: Vec<String>,
    pub running: usize,
    pub capacity: usize,
    pub headquarters: bool,
    pub seen: f64,
    pub operations: Vec<String>,
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct MissionBrief {
    pub prompt: String,
    pub agent_id: Option<String>,
    pub role_id: Option<String>,
    pub handler_provider: String,
    pub terminal_provider: String,
    pub working_directory: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub environment_keys: Vec<String>,
}
```

`BoardSession` has no `state` and sequence `0` until the Session's first
lifecycle event. `BoardMission` groups registered Sessions by Station and
Mission ID; queued Missions without a registered provider Session are not present.
`brief` is present when AI6 published the exact Mission prompt and sanitized launch
manifest during Session registration. It contains environment variable names only;
values and initial context are never returned. The detail is scoped by the same
`mission.read` authorization as the Session list.
Both models are non-exhaustive; downstream Rust code should deserialize them or
read their public fields instead of constructing struct literals.

`GET /v1/stations` returns this user's visible Station check-ins and hosted Operations
from the shared I Registry. A Station is healthy when its check-in is no older than 45
seconds; stale check-ins remain visible as offline. `headquarters_station` and each
Station's `headquarters` marker are populated only for an unexpired elected Headquarters
lease. The route filters Operations using `mission.read` and omits host and process
metadata.

## Gateway state

```rust
#[derive(Clone)]
pub struct Gateway { /* shared, bounded state */ }

impl Gateway {
    pub fn new(config: GatewayConfig) -> Self;
    pub fn with_registry(config: GatewayConfig, registry: ai6::IntakeRegistry) -> Self;
    pub fn with_access(
        config: GatewayConfig,
        registry: ai6::IntakeRegistry,
        access: AccessProviders,
    ) -> Self;
    pub fn authenticate_m(&self, presented: &str) -> Result<(), GatewayError>;
    pub async fn register_session(
        &self,
        m_token: &str,
        operation_id: OperationId,
        mission_id: MissionId,
        session_id: SessionId,
    ) -> Result<(), GatewayError>;
    pub async fn board_sessions(&self) -> Vec<BoardSession>;
    pub async fn board_missions(&self) -> Vec<BoardMission>;
    pub async fn publish_lifecycle(
        &self,
        m_token: &str,
        event: LifecycleEvent,
    ) -> Result<(), GatewayError>;
    pub async fn publish_terminal_output(
        &self,
        m_token: &str,
        session_id: SessionId,
        sequence: u64,
        bytes: Vec<u8>,
    ) -> Result<(), GatewayError>;
    pub async fn issue_attach_token(
        &self,
        m_token: &str,
        grant: AttachGrant,
    ) -> Result<String, GatewayError>;
    pub async fn exchange_attach_token(&self, token: &str)
        -> Result<String, GatewayError>;
    pub async fn attach(
        &self,
        session_id: &SessionId,
        ticket: &str,
        after_sequence: u64,
    ) -> Result<Attachment, GatewayError>;
    pub async fn ingest(
        &self,
        m_token: &str,
        session_id: &SessionId,
        sequence: u64,
        frame: Vec<u8>,
    ) -> Result<(), GatewayError>;
    pub async fn submit_input(
        &self,
        session_id: &SessionId,
        connection_id: &str,
        frame: Vec<u8>,
    ) -> Result<(), GatewayError>;
    pub async fn disconnect(&self, attachment: Attachment);
    pub async fn disconnect_connection(
        &self,
        session_id: &SessionId,
        connection_id: &str,
    );
    pub async fn receive_agent_input(&self) -> Option<AgentInput>;
    pub fn public_origin(&self) -> &str;
    pub fn router(self) -> Router;
    pub async fn serve_loopback<F>(
        self,
        port: u16,
        shutdown: F,
    ) -> Result<(), std::io::Error>
    where
        F: Future<Output = ()> + Send + 'static;
}
```

`BoardMission` groups registered provider Sessions by Station and Mission ID.
It reflects Sessions known to this gateway; a queued Mission with no registered
provider Session is not included.

`new` creates bounded in-memory state and one bounded M input queue. `authenticate_m`
compares the presented bearer credential with the configured M credential. Session
registration is idempotent only for the same Station/Mission/Session tuple. Session
and credential tables have configured capacity limits.

`issue_attach_token` returns a random, short-lived, single-use token. It can only
be called with M's credential and a grant matching the registered tuple. User grants
are available with `with_access` and enforce the configured Authorizer first.
`exchange_attach_token` consumes it and returns a second one-connection ticket.
The HTTP route keeps that ticket in an HttpOnly, Secure, SameSite=Strict cookie;
neither credential is placed in a URL or server log.

`attach` consumes a ticket, reserves a viewer slot and, for `Write`, acquires the
Session's only expiring writer lease. Its replay contains the latest schema for
each channel followed by retained value frames newer than `after_sequence`. A
reader receives no write permission. Call `disconnect` or
`disconnect_connection` when a socket closes to release the slot and lease.

`ingest` accepts M's guatiao schema, value and close frames on protocol channels
1 and 2. Schema/close frames use sequence zero. Value frames must have a positive
monotonic sequence and match their Station/Mission/Session identity and terminal
direction. Frames are bounded before fanout. Socket write buffers are bounded
too, and their configured capacity counts toward the aggregate budget.

`publish_lifecycle` and `publish_terminal_output` are the higher-level M path:
they construct guatiao values with the shared protocol crate, announce each
channel schema on first use, then submit the value through the same validation
and fanout path. Lifecycle state accepts `working`, `blocked`, `stalled`,
`done` and `failed`. The HTTP routes use JSON for lifecycle metadata and raw
bytes for terminal output; both remain bearer-authenticated as M.

`submit_input` accepts a guatiao terminal input or resize value for the same provider
Session, with a newer sequence and an unexpired write lease. It enforces per-connection
rate and queue limits. `receive_agent_input` waits for the next bounded frame for M;
there is one receiver for the gateway instance.

## Attachments and input

```rust
pub struct Attachment {
    pub mode: AttachMode,
    pub replay: Vec<Vec<u8>>,
    pub events: tokio::sync::broadcast::Receiver<Vec<u8>>,
}

pub struct AgentInput {
    pub operation_id: OperationId,
    pub mission_id: MissionId,
    pub session_id: SessionId,
    pub frame: Vec<u8>,
}
```

`Attachment` also carries private internal connection data used by the
disconnect methods. Its `Debug` output redacts the connection handle.
`AgentInput::Debug` redacts the terminal frame and reports only its byte count.
If the broadcast receiver reports `Lagged`, close that viewer and reconnect it
with the last sequence it processed.

## HTTP routes and transport

`Gateway::router()` returns the Axum router. `Gateway::serve_loopback(port,
shutdown)` binds to `127.0.0.1` until the caller's graceful-shutdown future resolves.
The browser-facing HTTPS reverse proxy must connect to that listener, remove any
client-supplied `X-Forwarded-Proto`, and set the header to `https`.

| Method and path | Contract |
| --- | --- |
| `GET /health` | Returns `204 No Content` while the gateway is serving. |
| `GET /v1/board` | Requires a user bearer and returns Sessions for Operations allowed by `mission.read`. |
| `GET /v1/stations` | Requires a user bearer; returns visible Station health, hosted Operations and current Headquarters Station. |
| `GET /v1/missions` | Requires a user bearer; groups registered Sessions by Mission and filters each Operation through `mission.read`. |
| `GET /v1/operations/{operation_id}/missions/{mission_id}` | Requires a user bearer; returns one authorized Mission and its AI6-published brief when available. |
| `POST /v1/auth/login` | Resolves the sanitized identity header from the trusted reverse proxy, or uses the configured Authenticator and Directory; returns a one-time user bearer secret. |
| `POST /v1/auth/logout` | Revokes the presented user token across gateways sharing the Registry. |
| `GET /v1/user/profile` | Requires a user bearer; returns directory identity plus effective Operation and Station actions. Operation grants include mission/session/audit/Agent-role actions; Station grants include `station.read`, `station.configure` and `audit.read`. Every action is computed by the Authorizer, not inferred from directory groups. |
| `GET /v1/user/audit?operation_id=<id>&station_id=<id>&limit=<n>` | Requires a user bearer and `audit.read`; the optional Operation and Station filters are mutually exclusive. Returns at most 500 newest metadata-only records, filtered by the Authorizer for each resource. Records contain actor, action, resource scope, target and time; prompts, terminal output and secrets are excluded. |
| `GET /v1/user/operations/{operation_id}/agent-roles` | Requires `agent.read`; returns the Headquarters Station catalog merged with validated Operation-scoped I Registry overrides. Each role includes `operation_override` provenance. |
| `PUT /v1/user/operations/{operation_id}/agent-roles/{role_id}` | Requires `agent.configure`; validates and stores the role override and records only actor/action/Operation/role ID/time in audit. |
| `DELETE /v1/user/operations/{operation_id}/agent-roles/{role_id}` | Requires `agent.configure`; removes the override, falls back to the Station definition and records a metadata-only audit event. |
| `GET /v1/user/stations/{station_id}/settings` | Requires `station.read`; returns the live concurrent Mission limit, whether it has an explicit Registry override, and whether the Station has the execution role. |
| `PUT /v1/user/stations/{station_id}/settings` | Requires `station.configure`; execution Stations only; validates a 1–1,024 concurrent Mission limit, persists it in the shared I Registry, and records a Station-scoped metadata-only audit event. |
| `POST /v1/user/attach-grants` | Requires a user bearer; checks `session.read` or `session.input` at the registered Operation before issuing an attach token. |
| `POST /v1/user/operations/{operation_id}/missions` | Requires `mission.create` at the target Operation and atomically enqueues a bounded explicit brief in the shared I Registry; returns the queued Mission ID. |
| `GET /v1/auth/session` | Validates a user bearer and reports its token ID, person ID and expiry. |
| `POST /v1/m/sessions` | Bearer-authenticated registration request with `operation_id`, `mission_id`, `session_id`, and optional sanitized Mission brief metadata. |
| `POST /v1/m/sessions/{session_id}/lifecycle/{sequence}` | Bearer-authenticated JSON lifecycle update with Station, Mission ID and state. |
| `POST /v1/m/sessions/{session_id}/terminal/{sequence}` | Bearer-authenticated raw terminal output bytes. |
| `POST /v1/m/sessions/{session_id}/events/{sequence}` | Bearer-authenticated binary guatiao frame. |
| `POST /v1/m/attach-grants` | Bearer-authenticated grant request; returns token in JSON response body. |
| `GET /v1/m/inputs/next` | Bearer-authenticated long poll; returns one binary Guatiao terminal input or resize frame with Station and Mission identity headers. AI6 validates the frame and dispatches it to that live Session's terminal. |
| `POST /v1/attach` | Requires exact `Origin` and trusted HTTPS proxy marker; exchanges token from JSON body for a scoped cookie. |
| `GET /v1/socket/{session_id}?after={sequence}` | Requires the cookie, exact `Origin` and trusted HTTPS marker. Streams binary guatiao messages. |

Only the `/v1/m/` routes accept M's bearer credential. The configured M token
must be loaded from Q's scoped credential lease and must never be logged. Browser
users authenticate at `/v1/auth/login`; Authenticator and Directory calls execute
on the blocking pool. Reverse-proxy identity is accepted only from the configured
header; the proxy must strip that header from client requests and set it after
authentication. The value is resolved through the Directory before policy checks.
Board and attach access requires an active token in the shared Registry and an
Authorizer decision. Missing providers make user routes return `503`; an absent
policy rule denies access.

## Errors

`GatewayError` is non-exhaustive and implements `std::error::Error`. It reports
invalid configuration, authentication, scope, missing Session, capacity
conflict, oversized/invalid frames, queue backpressure, or an invalid/expired
credential. Error text is fixed and does not quote request data. HTTP routes
map these to 400, 401, 403, 404, 409 or 413 responses.

## Command line

The optional `cli` feature builds the service command:

- `universal-exports --public-origin <HTTPS_ORIGIN> [--port <PORT>] [--plugin-path <PATH>]`
  starts the loopback gateway. `AI6_EXPORTS_M_TOKEN` is required from the environment;
  it is never accepted as an argument. Set `AI6_EXPORTS_DIRECTORY` and either
  `AI6_EXPORTS_AUTHENTICATOR` or `AI6_EXPORTS_PROXY_USER_HEADER` to enable user access.
  `AI6_EXPORTS_PROXY_LOOKUP_BY` selects the Directory attribute for proxy identity
  resolution and defaults to `name`. `--authenticator-config`,
  `--directory-config` and `--authorizer-config` each load one provider's JSON
  configuration from a file; the corresponding `AI6_EXPORTS_*_CONFIG` variables are
  also supported. A config file must contain an object, is limited to 1 MiB and 64
  nested values, and is schema-validated by Guatiao. The same file can configure a
  provider used for both authentication and directory lookup. `AI6_EXPORTS_ACCESS_POLICY`
  loads the built-in role policy; `AI6_EXPORTS_AUTHORIZER` selects a custom provider
  instead. Repeat `--plugin-path` for each provider directory or library.

`universal-exports` returns `0` after graceful shutdown, `1` when configuration or
listener startup fails, and `2` for invalid arguments. The read-only operator client is
the separate `ai6ctl` crate.

## Environment variables

The library reads no environment variables. The commands read:

| Variable | Effect | Default |
| --- | --- | --- |
| `AI6_EXPORTS_PUBLIC_ORIGIN` | Required browser HTTPS origin; overridden by `--public-origin`. | None |
| `AI6_EXPORTS_PORT` | Loopback listener port; overridden by `--port`. | `7082` |
| `AI6_EXPORTS_M_TOKEN` | Required M bearer credential from Q; accepted only from the environment, never a command-line argument. | None |
| `AI6_EXPORTS_AUTHENTICATOR` | Guatiao Authenticator provider ID; paired with the Directory setting. | None |
| `AI6_EXPORTS_AUTHENTICATOR_CONFIG` | Path to a JSON object containing the Authenticator's Guatiao config. | None |
| `AI6_EXPORTS_PROXY_USER_HEADER` | Sanitized authenticated-user header supplied by the trusted reverse proxy. | None |
| `AI6_EXPORTS_PROXY_LOOKUP_BY` | Directory attribute used to resolve that identity. | `name` |
| `AI6_EXPORTS_DIRECTORY` | Guatiao Directory provider ID; required for Authenticator or proxy-based user access. | None |
| `AI6_EXPORTS_DIRECTORY_CONFIG` | Path to a JSON object containing the Directory's Guatiao config. | None |
| `AI6_EXPORTS_ACCESS_POLICY` | TOML configuration for the built-in role Authorizer. | Deny all |
| `AI6_EXPORTS_AUTHORIZER` | Optional custom Authorizer provider ID; conflicts with access-policy. | None |
| `AI6_EXPORTS_AUTHORIZER_CONFIG` | Path to a JSON object containing the custom Authorizer's Guatiao config. | None |
| `AI6_EXPORTS_URL` | API origin for `ai6ctl`; overridden by `--server`. | `http://127.0.0.1:7082` |
| `AI6_ACCESS_TOKEN` | User bearer required by `ai6ctl missions`, `mission`, `sessions` and `session`. | None |

`--help` lists the flag-backed variables and names `AI6_EXPORTS_M_TOKEN` in its help text.
The command does not log the credential.

## Gotchas

- Bind via `serve_loopback`; the trusted reverse proxy must sanitize the
  `X-Forwarded-Proto` header before forwarding. It must also overwrite the
  configured authenticated-user header and never forward a client-supplied value.
- A schema frame is not a cursor event; send it with sequence zero. Value events
  carry their own positive per-Session sequence.
- Reconnect after a slow viewer is closed for lag. Replay is bounded and may not
  retain every old event.
- A write lease expires independently of the agent Session. A later writer may
  acquire it while an old socket is still closing; stale connections cannot send.
