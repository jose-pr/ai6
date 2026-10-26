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
        station_token: impl Into<String>,
        limits: GatewayLimits,
        attach_token_ttl: Duration,
        write_lease_ttl: Duration,
    ) -> Result<Self, GatewayError>;
    pub fn public_origin(&self) -> &str;
    pub fn with_login_limits(self, login: LoginLimits) -> Self;
}

pub struct LoginLimits {
    pub per_address: std::num::NonZeroU32,       // tries a minute from one address; default 5
    pub per_name: std::num::NonZeroU32,          // tries in ten minutes for one name; default 10
    pub address_header: Option<axum::http::HeaderName>,  // default None
}
```

`GatewayLimits::default()` allows 16 Sessions, 12 clients per Session, 64
scrollback events, 256 KiB per frame, 40 input frames per second, 64 queued
inputs and 8 fanout events. Config validation requires an HTTPS origin, a
32-byte minimum Station credential, finite non-zero bounds, TTLs no longer than 300
seconds, at most 100,000 client slots, and a maximum aggregate 512 MiB budget
for history, fanout, input and socket write buffers. `GatewayConfig` formats its
Station credential as `[REDACTED]`.

`LoginLimits` bounds `POST /v1/auth/login` when a password or key is checked (an identity the
trusted proxy asserts is not counted). Before any Authenticator is asked, a try is counted
against the client address (an IPv6 address counts as its /64 network) and against the account
name (trimmed, lowercased, the first 64 bytes). A try past either count is refused with `429`
and a `Retry-After` of the seconds to wait, even if its secret is right; an address that is
held off does not use up a name's tries. The gateway listens on loopback behind a reverse
proxy, so every connection comes from the proxy: set `address_header` to the header the proxy
fills with the client address (`x-forwarded-for`, where the last entry counts, or
`x-real-ip`); without it, or when the header holds no address, the connection's address is
used. The address is also given to the Authenticator as `Attempt::peer`. The counts are
in memory and start empty at every restart.

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
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BoardSession {
    pub operation_id: String,
    pub mission_id: String,
    pub session_id: String,
    pub state: Option<String>,
    pub reason: Option<String>,  // why AI6 holds that state: "green-ink", "budget"
    pub sequence: u64,
    pub updated_at: Option<f64>, // Unix seconds of the latest lifecycle event
    pub exposure: Vec<String>,   // untrusted input the Session read, from its Registry record
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BoardMission {
    pub operation_id: String,
    pub mission_id: String,
    pub sessions: Vec<BoardSession>,
    pub brief: Option<LaunchManifest>,
    pub usage: Option<MissionUsage>,   // tokens and cost from the Registry ledger
    pub attempts: Vec<MissionAttempt>, // every attempt, first first, from the Session records
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MissionAttempt {
    pub attempt: u32,
    pub session_id: String,                  // AI6's Session ID
    pub provider_session_id: Option<String>,
    pub predecessor: Option<String>,         // AI6 ID of the attempt it replaced
    pub station: String, pub theater: Option<String>,
    pub status: String,                      // "active" | "parked" | "ended" | "lost"
    pub lost: Option<String>,                // why, when lost
    pub checkpoint_branch: Option<String>,   // where its work was last checkpointed
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
    pub theaters: Vec<BoardTheater>,
    pub resources: Option<ai6::Resources>, // the host's CPUs, load, memory and `sampled_at`;
                                           // omitted when the Station did not report them
    pub config: Option<ai6::ConfigStatus>, // roster revision it runs (decimal string), why it
                                           // refused the newest one, files that need a restart;
                                           // omitted when the Station did not report it
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoardTheater {
    pub id: String,
    pub access: String,              // "local", "ssh", "transport" or "ephemeral" (a template)
    pub host: Option<String>,        // ssh host only; user, port and key path are never published
    pub provider: Option<String>,    // transport provider of a "transport" Theater
    pub labels: BTreeMap<String, String>,
    pub capabilities: Option<ai6::Capabilities>, // terminal, run, files, forward, os, arch, notes
    pub agents: String,              // "auto" | "local" | "remote-hands"
    pub egress: String,              // "direct" | "proxy" | "station" | "none"; never a proxy URL
    pub harnesses: Vec<String>,
    pub workdir: Option<String>,     // where a remote-hands Mission works without an Operation path
    pub unavailable: Option<String>, // why the Station cannot reach it now
    pub weakened: Vec<String>,       // loose settings and ambient credentials found there
    pub requires: Option<ai6::Requirements>, // what Sessions need there
    pub report: Option<ai6::ProbeReport>,    // the Station's last probe: unmet, found, free space
    pub made_for: Option<String>,    // the Mission an ephemeral Theater was made for
    pub resources: Option<ai6::Resources>, // last sample; omitted until one succeeds
    pub sessions: usize,             // the Station's running Sessions there; omitted when 0
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlacementPreview {
    pub operation_id: String,
    pub policy_station: String,      // the Station whose copy of the Operation's policy was used
    pub choices: Vec<ai6::PlacementChoice>,
    pub rejections: Vec<ai6::PlacementRejection>,
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct LaunchManifest {
    pub prompt: String,
    pub agent_id: Option<String>,
    pub role_id: Option<String>,
    pub handler_provider: String,
    pub terminal_provider: String,
    pub working_directory: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub environment_keys: Vec<String>,
    pub station_id: Option<String>,  // Station that supervises the Session (absent in older registrations)
    pub theater_id: Option<String>,  // Theater the harness works on
    pub mode: Option<String>,        // "field" | "remote-hands"; absent in older registrations
    pub provenance: Option<ai6::Provenance>, // where the brief came from (origin, principal, received_by, ...); absent for a Mission started on a Station
}
```

`BoardSession` has no `state`, sequence `0` and no `updated_at` until the Session's first
lifecycle event. `state` is the Mission's state as that Session last reported it; an
earlier attempt keeps the state it last reported (often `queued` after a retry), so the
Mission's current state is the one of its Session with the latest `updated_at`. `reason` is the
lifecycle event's reason, if any, and clears with the next state. `exposure` lists where the Session read untrusted input (`brief:source`,
`cable:SENDER`, `tool:NAME`, `review`) and is empty for a clean Session. `BoardMission` groups registered Sessions by Station and
Mission ID; queued Missions without a registered provider Session are not present.
`brief` is present when AI6 published the exact Mission prompt and sanitized launch
manifest during Session registration. It contains environment variable names only;
values and initial context are never returned. The detail is scoped by the same
`mission.read` authorization as the Session list.
Both models are non-exhaustive; downstream Rust code should deserialize them or
read their public fields instead of constructing struct literals.

`GET /v1/stations` returns this user's visible Station check-ins and hosted Operations
from the shared Registry. A Station is healthy when its check-in is no older than 45
seconds; stale check-ins remain visible as offline. `headquarters_station` and each
Station's `headquarters` marker are populated only for an unexpired elected Headquarters
lease. The route filters Operations using `mission.read` and omits host and process
metadata. Each Station lists the Theaters it reaches (`theaters`; a Station that published
none reaches only its own `local` host) with what the transport can do there, and
placement choices carry their `mode` and load `score` (0 idle to 4; the Operation's
`placement` decides whether the lowest or highest comes first). Rejections include the
resource limits a Station or Theater is below or above.

`GET /v1/user/operations/{operation_id}/placement-preview` answers where a Mission for the
Operation would run, and why the other places would not (see `ai6::place`). It requires
`mission.read` on the Operation. The query may carry `agent`, `role`, `stations` and `tags`
(comma-separated), which only narrow the Operation's policy, as a Mission's brief does. The
policy comes from the Headquarters Station's check-in when it publishes one, otherwise from
the first Station that does (`policy_station`); `404` when none does. Rejections name every
Station in the Registry, not only those hosting the Operation.

## Operator controls

```rust
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ControlStatus {
    pub control_id: String,
    pub operation_id: String,
    pub mission_id: String,
    pub session_id: Option<String>,  // provider Session expected to be current
    pub action: String,              // "cancel" | "retry" | "resume" | "interrupt"
    pub agent: Option<String>,       // a retry's Agent
    pub station_id: String,          // the Station asked to act
    pub status: String,              // "queued" | "delivered" | "applied" | "rejected" | "expired"
    pub state: Option<String>,       // Mission state the Station reported
    pub reason: Option<String>,      // why it was rejected, AI6's wording
    pub requested_by: String,        // person ID
    pub requested_at: f64,
    pub finished_at: Option<f64>,
}
```

Returned by the control routes below. A resume's `message` is never in a status or an
audit record; it travels only in the control frame to the Station.

Routing: the Station holding the Mission's latest Session record (highest attempt), else
the Station holding a brief lease whose `mission_id` matches, else the Headquarters Station;
Stations whose check-in is older than 45 seconds are passed over, and with none the request
is refused `503` with `Retry-After: 15`. When the request names `session_id` and the
Registry knows another provider Session as current, it is refused `409`; otherwise the
current one is attached. A Mission the board knows only under another Operation is `404`.

Bounds (in memory, per gateway): 1,024 controls kept, oldest finished dropped first, `409`
when all are open; 64 queued per Station, `409` beyond; a control queued or delivered for
more than 120 seconds becomes `expired`. The idempotency key is scoped to the person,
Operation and Mission (1-128 bytes, no control characters); the same key with the same
action, Agent, message and Session returns the first status with `200`, with anything else
`409`. Every accepted request is audited as `mission.cancel`, `mission.retry`,
`mission.resume` or `session.interrupt` with the Mission ID as target.

## Activity timeline

```rust
pub const DEFAULT_TIMELINE_PAGE: usize = 50;
pub const MAX_TIMELINE_PAGE: usize = 200;

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TimelinePage {
    pub operation_id: String,
    pub items: Vec<TimelineItem>,      // newest first
    pub next_cursor: Option<String>,   // opaque; absent on the last page
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TimelineItem {
    pub id: String,                    // "event:<id>" | "audit:<id>"
    pub at: f64,                       // Unix seconds
    pub kind: String,                  // "lifecycle" | "control" | "cable" | "sleeper" | "audit"
    pub action: String,                // state, "<action>.<outcome>", "sent", "authorized", "revoked" or audit action
    pub operation_id: String,
    pub station_id: Option<String>,
    pub mission_id: Option<String>,
    pub session_id: Option<String>,
    pub actor: Option<String>,         // person ID or Agent codename
    pub target: Option<String>,        // an audit record's target
    pub detail: BTreeMap<String, String>,
}
```

`GET /v1/user/operations/{operation_id}/timeline` (query `cursor`, `mission`, `limit`;
unknown parameters refused) needs `audit.read` on the Operation; another Operation's is
`403`. It merges `ai6::TimelineStore::timeline_events(operation)` with the audit records
whose Operation it is and, for each Station that hosts the Operation and whose `audit.read`
the caller also holds, that Station's records (shown with `station_id`). Audit records of
`mission.create.requested`, `mission.cancel`, `mission.retry`, `mission.resume` and
`session.interrupt` carry their target as `mission_id`. `mission=` keeps items of that
Mission. Order is `at` descending, then `id` descending; `cursor` returns the items after
that position, so newer activity never shifts older pages. `limit` outside 1-200, an
unknown or malformed cursor and an invalid Mission ID are `400`. Every text field is cut
to 256 characters without control characters.

The gateway records a `lifecycle` event for each state change it ingests (a repeated state
is not one; the Station comes from the published launch manifest) and a `control` event
(`<action>.<status>`, actor the requester, details `control`, `state`, `agent`, `reason`)
when a Station reports a control. Neither write can fail the publication or report.

## Gateway state

```rust
#[derive(Clone)]
pub struct Gateway { /* shared, bounded state */ }

impl Gateway {
    pub fn new(config: GatewayConfig) -> Self;
    /// Any Registry store; the command passes `ai6::directorates::registry::backend::from_environment()?`.
    pub fn with_registry(config: GatewayConfig, registry: Arc<dyn ai6::RegistryBackend>) -> Self;
    pub fn with_access(
        config: GatewayConfig,
        registry: Arc<dyn ai6::RegistryBackend>,
        access: AccessProviders,
    ) -> Self;
    /// Serves `/v1/user/roster/...` from the roster the Stations read; without it those
    /// routes answer 503. The command opens `AI6_EXPORTS_ROSTER` with ai6's config::open_with.
    #[must_use]
    pub fn with_roster(self, source: Arc<dyn ai6::ConfigSource>) -> Self;
    pub fn authenticate_station(&self, presented: &str) -> Result<(), GatewayError>;
    pub async fn register_session(
        &self,
        station_token: &str,
        operation_id: OperationId,
        mission_id: MissionId,
        session_id: SessionId,
    ) -> Result<(), GatewayError>;
    pub async fn board_sessions(&self) -> Vec<BoardSession>;
    pub async fn board_missions(&self) -> Vec<BoardMission>;
    pub async fn publish_lifecycle(
        &self,
        station_token: &str,
        event: LifecycleEvent,
    ) -> Result<(), GatewayError>;
    pub async fn publish_terminal_output(
        &self,
        station_token: &str,
        session_id: SessionId,
        sequence: u64,
        bytes: Vec<u8>,
    ) -> Result<(), GatewayError>;
    pub async fn issue_attach_token(
        &self,
        station_token: &str,
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
        station_token: &str,
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

`new` creates bounded in-memory state and one bounded M input queue. `authenticate_station`
compares the presented bearer credential with the configured Station credential. Session
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
and fanout path. Lifecycle state accepts `queued` (another attempt is coming), `working`,
`blocked`, `stalled`, `parked` (a provider usage limit), `quarantined` (the Agent is
quarantined), `done`, `failed` and `abandoned`. The HTTP routes use JSON for lifecycle metadata and raw
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
| `GET /v1/user/operations/{operation_id}/placement-preview` | Requires `mission.read` at the Operation; returns the eligible (Station, Theater, Agent) choices, best first, and a reason for each rejection. Optional `agent`, `role`, `stations`, `tags` query parameters narrow it. |
| `GET /v1/missions` | Requires a user bearer; groups registered Sessions by Mission and filters each Operation through `mission.read`. |
| `GET /v1/operations/{operation_id}/missions/{mission_id}` | Requires a user bearer; returns one authorized Mission, its AI6-published brief when available, its `usage` (tokens and cost per Session and UTC day) once a provider has reported any, and its `attempts` from the shared Session records. `GET /v1/missions` carries `attempts` too. |
| `POST /v1/auth/login` | Resolves the sanitized identity header from the trusted reverse proxy, or uses the configured Authenticator and Directory; returns a one-time user bearer secret. |
| `POST /v1/auth/logout` | Revokes the presented user token across gateways sharing the Registry. |
| `GET /v1/user/profile` | Requires a user bearer; returns directory identity plus effective Operation and Station actions. Operation grants include mission/session/audit/Agent-role, `approval.decide`/`approval.class`, `pool.read` and `pool.manage` actions; Station grants include `station.read`, `station.configure` and `audit.read`; `roster` lists the person's `roster.read`, `roster.manage` and `audit.read` on `config:roster`. Every action is computed by the Authorizer, not inferred from directory groups. |
| `GET /v1/user/audit?operation_id=<id>&station_id=<id>&limit=<n>` | Requires a user bearer and `audit.read`; the optional Operation and Station filters are mutually exclusive. Returns at most 500 newest metadata-only records, filtered by the Authorizer for each resource. Records contain actor, action, resource scope, target and time; prompts, terminal output and secrets are excluded. |
| `GET /v1/user/operations/{operation_id}/timeline?cursor=&mission=&limit=` | Requires `audit.read` on the Operation; returns a `TimelinePage` (see Activity timeline). |
| `GET /v1/user/operations/{operation_id}/pools` | Requires `pool.read` on the Operation; returns a `PoolBoard { operation_id, accounts: Vec<BoardAccount> }`: each account a Station declared for the Operation with `pool`, `account`, `handler`, `kind`, `priority`, `max_concurrent`, `labels`, `active` (`BoardClaim { station_id, mission_id, since }`, this Operation's live claims only), `other_sessions` (the count of other Operations' live claims), `windows` (`BoardWindow { name, used_permille, resets_at, status }`), `limited_until` (only while in the future), `last_used` and `tokens_today` (UTC). Never a credential, its secret's name or a login directory. |
| `GET /v1/user/roster/{kind}` | `kind` is `operations`, `agents`, `roles` or `covers` (else `404`). Requires `roster.read` on `config:roster`; returns `RosterRecords { kind, revision, writable, records: Vec<RosterRecord { key, definition }> }`: each definition is the JSON object of its table in the configuration files, `revision` the roster revision in decimal (compare `BoardStation.config.revision`), `writable` whether edits are taken. `503` without a roster source. |
| `PUT /v1/user/roster/{kind}/{key}` | Body: the record's definition, a JSON object (else `400`). Requires `roster.manage` on `config:roster`. The source checks the whole roster first: `200` `RosterChange { revision }` and a `roster.update` audit event; `422` `{"refused": reason}` and a `roster.refused` audit event (target `<kind> <key>: <reason>`) when the roster it would leave is not acceptable, nothing changed; `409` for a read-only source (a configuration directory). Audit resource `config:roster`, target `<kind> <key>`. Every Station takes up an accepted edit at its next poll. |
| `DELETE /v1/user/roster/{kind}/{key}` | As `PUT`: `200` with the revision and a `roster.remove` audit event, `404` when there is no such record, `422` when the removal leaves the roster unacceptable (an Agent another extends). |
| `POST /v1/user/operations/{operation_id}/pools/{pool}/{account}/clear-limit` | Requires `pool.manage` on the Operation; lifts the account's limit (`404` unless a Station declared it for the Operation) and records a `pool.limit.clear` audit event with target `pool/account`; `204`. |
| `GET /v1/user/operations/{operation_id}/agent-roles` | Requires `agent.read`; returns the Headquarters Station catalog merged with validated Operation-scoped Registry overrides. Each role includes `operation_override` provenance. |
| `PUT /v1/user/operations/{operation_id}/agent-roles/{role_id}` | Requires `agent.configure`; validates and stores the role override and records only actor/action/Operation/role ID/time in audit. |
| `DELETE /v1/user/operations/{operation_id}/agent-roles/{role_id}` | Requires `agent.configure`; removes the override, falls back to the Station definition and records a metadata-only audit event. |
| `GET /v1/user/operations/{operation_id}/quarantine` | Requires `agent.read`; lists the Agent quarantines that apply to the Operation (its own and those covering every Operation) with reason, who raised them and when. |
| `POST /v1/user/operations/{operation_id}/quarantine/{agent_id}` | Requires `agent.configure`; body `{"reason": "..."}` (1-512 characters); quarantines the Agent in this Operation and records an `agent.quarantine` audit event. |
| `POST /v1/user/operations/{operation_id}/quarantine/{agent_id}/release` | Requires `agent.configure`; lifts this Operation's quarantine of the Agent (`404` when there is none; a quarantine of every Operation is released on a Station) and records `agent.quarantine.release`. |
| `GET /v1/user/operations/{operation_id}/green-ink` | Requires `approval.decide`; returns the Operation's `ai6::ApprovalRequest`s waiting for a decision, oldest first (expired ones are recorded as such and left out). |
| `POST /v1/user/operations/{operation_id}/green-ink/{request_id}/approve` | Requires `approval.decide`; body `{"expires_in_secs"?: 1..=31536000 (default 3600), "note"?: up to 1,024 characters}`, unknown fields refused. Signs as the caller and returns the `ai6::Approval`; the same decision again returns it, `409` when it was rejected or expired, `404` for an unknown request. Audited as `green_ink.approved` (target: `REQUEST_ID KIND TARGET`, never the reason). |
| `POST /v1/user/operations/{operation_id}/green-ink/{request_id}/reject` | Requires `approval.decide`; body `{"reason"?: ...}`; as approve, audited as `green_ink.rejected`. The reason reaches the Agent. |
| `POST /v1/user/operations/{operation_id}/green-ink/{request_id}/revoke` | Requires `approval.decide`; no body. Withdraws an approval before it expires (again is harmless and not audited again) and returns the `ai6::Approval` with `revoked`; `404` for an unknown or undecided request, `409` for a refusal. Audited as `green_ink.revoked` before it is applied, so a revocation that cannot be recorded (`500`) is not made; the Station revokes a credential leased against it. |
| `GET /v1/user/operations/{operation_id}/class-authorizations` | Requires `approval.class`; returns every `ai6::ClassAuthorization` of the Operation, oldest first, revoked and expired ones included. |
| `POST /v1/user/operations/{operation_id}/class-authorizations` | Requires `approval.class`; body `{"kinds", "target_pattern", "agents"?, "roles"?, "clearance_min"?, "max_uses"?, "expires_in_secs"}` (at least one Agent or role; `"*"` is every Agent). Signs as the caller, returns `201` and the record, audited as `class_authorization.created` (target: `ID KINDS PATTERN`). |
| `DELETE /v1/user/operations/{operation_id}/class-authorizations/{id}` | Requires `approval.class`; revokes it (again is harmless) and returns the record, audited as `class_authorization.revoked`. |
| `GET /v1/user/operations/{operation_id}/held-briefs` | Requires `mission.create`; returns at most 500 `HeldBriefEntry { key, operation, brief, vetting, held_at }` (the `ai6::HeldBrief` flattened beside its Registry key), oldest first. The brief carries its untrusted text; show it only on request. |
| `POST /v1/user/operations/{operation_id}/held-briefs/{key}/release` | Requires `mission.create`; no body. Returns the brief to the queue cleared of the kinds it was held for and returns `HeldBriefDecision { operation_id, key, state: "queued" }`; `404` when the Operation holds no such brief. Audited as `mission.brief.released` (target: the key) and recorded on the timeline as `intake` `released`. |
| `POST /v1/user/operations/{operation_id}/held-briefs/{key}/refuse` | Requires `mission.create`; body `{"reason"?: up to 1,024 characters}`. Files the brief as failed with outcome state `refused` (a Source gets its CX report; the reason stays in the Registry) and returns `state: "refused"`. Audited as `mission.brief.refused`, timeline `intake` `refused`. |
| `GET /v1/user/stations/{station_id}/settings` | Requires `station.read`; returns the live concurrent Mission limit, `min_free_memory_mb` and `max_cpu_percent` (defaults 512 and 95 when unset), whether it has an explicit Registry override, and whether the Station has the execution role. |
| `PUT /v1/user/stations/{station_id}/settings` | Requires `station.configure`; execution Stations only; body `{"max_concurrent_missions", "min_free_memory_mb"?, "max_cpu_percent"?}` (an absent limit keeps its current value); validates a 1–1,024 concurrent Mission limit and a `max_cpu_percent` of at most 100, persists them in the shared Registry, and records a Station-scoped metadata-only audit event whose target names the fields sent. |
| `POST /v1/user/attach-grants` | Requires a user bearer; checks `session.read` or `session.input` at the registered Operation before issuing an attach token. |
| `POST /v1/user/operations/{operation_id}/missions/{mission_id}/actions` | Requires `mission.control` (`session.input` is not enough). Body `{"action", "agent"?, "message"?, "session_id"?, "idempotency_key"}`, unknown fields refused; `agent` only with `retry`, `message` (1 byte to 16 KiB, not blank) only with `resume`. Audits, queues the control (see Operator controls) and returns `202` with a `ControlStatus`; a repeated key returns `200`. |
| `GET /v1/user/operations/{operation_id}/missions/{mission_id}/actions/{control_id}` | Requires `mission.read`; returns the control's `ControlStatus`, `404` for an unknown control or one of another Mission. |
| `POST /v1/user/operations/{operation_id}/missions` | Requires `mission.create` at the target Operation and atomically enqueues a bounded explicit brief in the shared Registry; returns the queued Mission ID. The body may carry `stations` to limit the Mission to those Stations (it can only narrow the Operation's list). |
| `GET /v1/auth/session` | Validates a user bearer and reports its token ID, person ID and expiry. |
| `POST /v1/station/sessions` | Bearer-authenticated registration request with `operation_id`, `mission_id`, `session_id`, and optional sanitized Mission brief metadata. |
| `POST /v1/station/sessions/{session_id}/lifecycle/{sequence}` | Bearer-authenticated JSON lifecycle update with Station, Mission ID and state. |
| `POST /v1/station/sessions/{session_id}/terminal/{sequence}` | Bearer-authenticated raw terminal output bytes. |
| `POST /v1/station/sessions/{session_id}/events/{sequence}` | Bearer-authenticated binary guatiao frame. |
| `POST /v1/station/attach-grants` | Bearer-authenticated grant request; returns token in JSON response body. |
| `GET /v1/station/inputs/next` | Bearer-authenticated long poll; returns one binary Guatiao terminal input or resize frame with `x-ai6-operation-id`, `x-ai6-mission-id` and `x-ai6-session-id` headers. AI6 validates the frame and dispatches it to that live Session's terminal. |
| `GET /v1/station/controls/next?station=<id>&wait_ms=<n>` | Bearer-authenticated long poll for that Station's next control (`wait_ms` default 1000, capped at 25000); `200` with one `application/vnd.ai6.guatiao` control value frame on channel 4 (the receiver builds the schema from any control) and the control becomes `delivered`, or `204`. Stations share one credential, so `station` is the caller's own word. |
| `POST /v1/station/controls/{control_id}/result` | Bearer-authenticated outcome `{"station","outcome":"applied"\|"rejected","state"?,"reason"?}`; `state` is a Mission state name, `reason` is cut to 512 characters without control characters. `204`; `404` unknown, `403` another Station's control, `409` already finished. |
| `POST /v1/attach` | Requires exact `Origin` and trusted HTTPS proxy marker; exchanges token from JSON body for a scoped cookie. |
| `GET /v1/socket/{session_id}?after={sequence}` | Requires the cookie, exact `Origin` and trusted HTTPS marker. Streams binary guatiao messages. |

Only the `/v1/station/` routes accept the Station bearer credential. The configured Station token
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
map these to 400, 401, 403, 404, 409 or 413 responses. The login route also answers
`429` with `Retry-After` past its limits.

## Command line

The optional `cli` feature builds the service command:

- `universal-exports --public-origin <HTTPS_ORIGIN> [--port <PORT>] [--plugin-path <PATH>]`
  starts the loopback gateway. `AI6_EXPORTS_STATION_TOKEN` is required from the environment;
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
  instead. Repeat `--plugin-path` for each provider directory or library. The libraries are
  checked as a Station checks its own: `--plugin-pins PATH` (`AI6_EXPORTS_PLUGIN_PINS`; must
  exist when named, default `plugins.toml` in the AI6 configuration directory) and
  `--plugin-profile strict|report|off` (`AI6_EXPORTS_PLUGIN_PROFILE`; default strict with a
  pin set, report without); refusals and weakenings are printed at start. Provider IDs
  are those of the `coqui-access` plugin libraries (`coqui_access_local`, `coqui_access_key`,
  `coqui_access_ldap`, `coqui_access_proxy`, `coqui_access_peer`, `coqui_roles`).
  `--login-tries-per-address`, `--login-tries-per-name` and `--client-address-header` set
  the login limits above.

`universal-exports` returns `0` after graceful shutdown, `1` when configuration or
listener startup fails, and `2` for invalid arguments. The operator command-line client is
the separate `ai6ctl` crate.

## Environment variables

The library reads no environment variables. The commands read:

| Variable | Effect | Default |
| --- | --- | --- |
| `AI6_EXPORTS_PUBLIC_ORIGIN` | Required browser HTTPS origin; overridden by `--public-origin`. | None |
| `AI6_EXPORTS_PORT` | Loopback listener port; overridden by `--port`. | `7082` |
| `AI6_EXPORTS_STATION_TOKEN` | Required Station channel bearer credential from Q; accepted only from the environment, never a command-line argument. | None |
| `AI6_EXPORTS_AUTHENTICATOR` | Guatiao Authenticator provider ID; paired with the Directory setting. | None |
| `AI6_EXPORTS_AUTHENTICATOR_CONFIG` | Path to a JSON object containing the Authenticator's Guatiao config. | None |
| `AI6_EXPORTS_PROXY_USER_HEADER` | Sanitized authenticated-user header supplied by the trusted reverse proxy. | None |
| `AI6_EXPORTS_PROXY_LOOKUP_BY` | Directory attribute used to resolve that identity. | `name` |
| `AI6_EXPORTS_DIRECTORY` | Guatiao Directory provider ID; required for Authenticator or proxy-based user access. | None |
| `AI6_EXPORTS_DIRECTORY_CONFIG` | Path to a JSON object containing the Directory's Guatiao config. | None |
| `AI6_EXPORTS_ACCESS_POLICY` | TOML configuration for the built-in role Authorizer. | Deny all |
| `AI6_EXPORTS_AUTHORIZER` | Optional custom Authorizer provider ID; conflicts with access-policy. | None |
| `AI6_EXPORTS_AUTHORIZER_CONFIG` | Path to a JSON object containing the custom Authorizer's Guatiao config. | None |
| `AI6_EXPORTS_ROSTER` | The roster the Stations read (`ai6 serve --config`): a configuration directory (listed, read-only), `sqlite:PATH` or a `postgres://` URL (editable); overridden by `--roster`. Its password may come from `AI6_CONFIG_PASSWORD`. | None (roster routes answer 503) |
| `AI6_EXPORTS_LOGIN_TRIES_PER_ADDRESS` | Password or key tries a minute from one client address at the login route. | `5` |
| `AI6_EXPORTS_LOGIN_TRIES_PER_NAME` | Tries in ten minutes for one account name, from anywhere. | `10` |
| `AI6_EXPORTS_CLIENT_ADDRESS_HEADER` | Header the trusted proxy sets to the client address; the last entry counts. | None (the connection's address) |
| `AI6_EXPORTS_URL` | API origin for `ai6ctl`; overridden by `--server`. | `http://127.0.0.1:7082` |
| `AI6_ACCESS_TOKEN` | User bearer required by every `ai6ctl` command except `health`. | None |

`--help` lists the flag-backed variables and names `AI6_EXPORTS_STATION_TOKEN` in its help text.
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
