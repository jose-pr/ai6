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
`ai6-diptel`, serde and UUID, and, for Liaison, `coqui-a2a`, `coqui-agentic-peer` and
`coqui-web-channel` (pinned by git revision), `url`, `time` and `webpki-root-certs`. The `cli`
feature adds clap and Tokio signal support.

## Imports and modules

Every public item is imported from the crate root, except Liaison's, in the public module
`liaison`. There are no feature-gated items.

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
    pub buffer_budget_bytes: ai6::Limit,    // 512 MiB; none takes the budget check away
    pub client_slots: ai6::Limit,           // 100,000; none takes the slot check away
    pub station_token_min_bytes: usize,     // 32
    pub origin_bytes: ai6::Limit,           // 2,048
}
impl From<&ai6::UnexGatewayLimits> for GatewayLimits {}   // the `unex` record's [gateway]

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
    // [controls], [timeline] and [text] of the `unex` record; the defaults without it.
    pub fn with_unex_limits(self, limits: ai6::UnexLimits) -> Self;
}

pub struct LoginLimits {
    pub per_address: std::num::NonZeroU32,       // tries a minute from one address; default 5
    pub per_name: std::num::NonZeroU32,          // tries in ten minutes for one name; default 10
    pub address_header: Option<axum::http::HeaderName>,  // default None
    pub name_bytes: usize,                       // 64: a longer name is counted by its first bytes
    pub forget_every: u32,                       // 256 tries between sweeps, at least 1
    pub address_window: std::time::Duration,     // 60 s
    pub name_window: std::time::Duration,        // 600 s
}
```

`GatewayLimits::default()` allows 16 Sessions, 12 clients per Session, 64
scrollback events, 256 KiB per frame, 40 input frames per second, 64 queued
inputs and 8 fanout events. Config validation requires an HTTPS origin of at most
`origin_bytes`, a Station credential of at least `station_token_min_bytes` (32), finite non-zero
bounds, TTLs of more than zero, at most `client_slots` (100,000) client slots, and, within
`buffer_budget_bytes` (512 MiB), the buffers for history, fanout, input and socket writes;
there is no ceiling on a frame or a TTL, and a `Limit` of none takes its check away. `GatewayConfig` formats its
Station credential as `[REDACTED]`.

`LoginLimits` bounds `POST /v1/auth/login` when a password or key is checked (an identity the
trusted proxy asserts is not counted). Before any Authenticator is asked, a try is counted
against the client address (an IPv6 address counts as its /64 network) and against the account
name (trimmed, lowercased, the first `[login] name_bytes`, 64, bytes). A try past either count is refused with `429`
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
    // Err below `min_bytes` (`[unex] proxy_secret_min_bytes`, 32) or without a proxy identity. Without a secret no
    // asserted identity is accepted: login with the identity header also needs
    // PROXY_SECRET_HEADER ("x-ai6-proxy-secret") equal to it, else 401.
    pub fn with_proxy_secret(self, secret: impl Into<String>, min_bytes: usize) -> Result<Self, String>;
}
pub const PROXY_SECRET_HEADER: &str = "x-ai6-proxy-secret";
```

`universal-exports` reads the secret from `[unex] proxy_secret` and refuses to start
with the `unex` record's `proxy_user_header` but no secret.

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
    pub station_id: Option<String>, // the Station holding it, from its Registry record
    pub agent: Option<String>,   // the Agent its Mission's published brief names
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BoardMission {
    pub operation_id: String,
    pub mission_id: String,            // the Mission's UID when it came from the Registry
    pub handle: String,                // what it is called now; its ID when the Registry names it no other way
    pub sessions: Vec<BoardSession>,
    pub brief: Option<LaunchManifest>,
    pub usage: Option<MissionUsage>,   // tokens and cost from the Registry ledger
    pub attempts: Vec<MissionAttempt>, // every attempt, first first, from the Session records
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MissionAttempt {
    pub attempt: u32,
    pub session_id: String,                  // AI6's Session ID, the run's UID
    pub handle: Option<String>,              // the run's `<mission handle>.<attempt>` when it opened
    pub provider_session_id: Option<String>,
    pub predecessor: Option<String>,         // AI6 ID of the attempt it replaced
    pub station: String, pub theater: Option<String>,
    pub status: String,                      // "active" | "parked" | "ended" | "lost"
    pub lost: Option<String>,                // why, when lost
    pub checkpoint_branch: Option<String>,   // where its work was last checkpointed
    pub route: Option<ai6::RouteRef>,        // the handler, model and pool it ran on, once launched
    pub account: Option<ai6::AccountRef>,    // the account it drew from, a route's own included
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StationBoard {
    pub stations: Vec<BoardStation>,
    pub headquarters_station: Option<String>,
    pub operations: Vec<BoardOperation>,   // every Operation the person may read, in ID order
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoardOperation {
    pub operation_id: String,
    pub parent: Option<String>,   // nearest readable ancestor; omitted for a root or when all are hidden
}

/// `GET /v1/user/roster/operation/{id}/effective`
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EffectiveOperation {
    pub operation_id: String,
    pub lineage: Vec<String>,                                   // ancestors, nearest first
    pub inherit: bool,
    pub definition: serde_json::Map<String, serde_json::Value>, // layered, before presets and defaults
    pub origins: BTreeMap<String, String>,                      // dotted key -> Operation that wrote it
    pub revision: String,
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BoardStation {
    pub station_id: String,
    pub handlers: Vec<String>,       // the Agent handlers it reported at check-in
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
    pub head_of_station: Option<ai6::StationReport>, // its Head of Station's latest report
                                           // (summary, typed findings); omitted until one is filed
                                           // and unless the viewer has `station.read` on
                                           // `station:ID` (it covers every Operation there)
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
    pub name: Option<String>,        // what to show beside the Mission ID: a person's title or an issue's, as data
    pub prompt: String,
    pub agent_id: Option<String>,
    pub role_id: Option<String>,
    pub handler_provider: String,
    pub terminal_provider: String,
    pub working_directory: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub environment_keys: Vec<String>,
    pub station_id: String,          // Station that supervises the Session
    pub theater_id: Option<String>,  // Theater the harness works on, when the Station knows it
    pub mode: String,                // "field" | "remote-hands"
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

Operations form a tree. Every request about an Operation names its ancestors
(`operation:<id>`, nearest first) from the most recent Station check-in that carries
`operation_lineage`, so a rule on a parent Operation reaches its subordinates unless it sets
`propagate = false`; an Operation no check-in names has no ancestors (only its own and `*`
rules apply). Every authorization of an Operation goes through one helper, so the tree reaches
each route alike. A person sees an ancestor only with `mission.read` on it: `parent` in the
profile and on the board is the nearest readable ancestor, and a hidden one is never named.

`GET /v1/stations` returns this user's visible Station check-ins and hosted Operations, each
Station with `issues_weakened`, the issue settings looser than the default of the Operations the person may read,
from the shared Registry, and `operations`: every Operation of the Stations and of the tree
the person may read, each with its `parent`. A Station is healthy when its check-in is no older than 45
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
current one is attached. The Mission must be known under the Operation of the route: a
Session record's `operation`, its brief lease's `operation` metadata, its queued brief's
`operation` or a board entry names it, and none of them names another. Otherwise it is `404`,
refused before anything is routed, audited or queued, with no Station or Session in the body. A
`cancel` of a Mission whose brief is filed `done` or `failed`, with no Station to ask, is settled here:
`202`, `applied`, the brief's state, `station_id` empty, audited.

Bounds (in memory, per gateway): 1,024 controls kept, oldest finished dropped first, `409`
when all are open; 64 queued per Station, `409` beyond; a control queued or delivered for
more than 120 seconds becomes `expired`. The idempotency key is scoped to the person,
Operation and Mission (1-128 bytes, no control characters); the same key with the same
action, Agent, message and Session returns the first status with `200`, with anything else
`409`. Every accepted request is audited as `mission.cancel`, `mission.retry`,
`mission.resume` or `session.interrupt` with the Mission ID as target.

## Activity timeline

```rust
// A page holds `[timeline] page` (50) items unless it asks for more, up to `[timeline] max_page`
// (200); an item's text is cut at `[timeline] text_chars` (256).

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
    pub mission_id: Option<String>,    // the Mission's UID
    pub handle: Option<String>,        // what that Mission is called now
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

## Moneypenny, the assistant

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct AssistantConfig { /* private */ }
impl Default for AssistantConfig { /* agent "moneypenny", the defaults below */ }
impl AssistantConfig {
    pub fn new(agent_id: impl Into<String>) -> Result<Self, GatewayError>; // InvalidConfiguration
    pub fn agent_id(&self) -> &str;
    /// [assistant.limits]: jobs, reports and changes are held to them as the Stations do;
    /// a person's message is refused past the smaller of these message_bytes and the
    /// gateway table's. Err(InvalidConfiguration) for what AssistantLimits::validate refuses
    pub fn with_limits(self, limits: ai6::AssistantLimits) -> Result<Self, GatewayError>;
    /// [assistant.gateway]: conversations kept, live ones, tokens, answers, proposals,
    /// tools, the socket. Err(InvalidConfiguration) for
    /// what AssistantGatewayLimits::validate refuses
    pub fn with_gateway_limits(self, limits: ai6::AssistantGatewayLimits)
        -> Result<Self, GatewayError>;
    /// The cables record's body_bytes and default_ttl (CableConfig::message_limits): a
    /// proposed Cable with a longer body is refused; one naming no lifetime is shown with it
    pub fn with_cable_limits(self, body_bytes: ai6::Limit, default_ttl: ai6::Seconds) -> Self;
    /// The longest `text` filter of an app route (default 128 bytes, at least 1), in her
    /// links and cards and in a message's `route`; past it the view is refused, or the page
    /// line dropped
    pub fn with_route_text_bytes(self, bytes: usize) -> Self;
    /// The bounds of her views, each raised to at least 1; sent to the app as `view_limits`.
    /// The command reads them from `[assistant.views]` at start
    pub fn with_view_limits(self, limits: ViewLimits) -> Self;
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ViewLimits {   // Default: the value in each comment
    pub blocks: usize /* 24 */, pub view_bytes: usize /* 64 KiB */,
    pub text_bytes: usize /* 8 KiB */, pub short_bytes: usize /* 512 */,
    pub columns: usize /* 12 */, pub rows: usize /* 200 */, pub items: usize /* 100 */,
    pub series: usize /* 8 */, pub points: usize /* 200 */, pub pie_slices: usize /* 12 */,
    pub diff_bytes: usize /* 16 KiB */, pub fields: usize /* 20 */,
    pub schedule_rows: usize /* 50 */, pub graph_nodes: usize /* 60 */,
    pub graph_edges: usize /* 120 */, pub svg_bytes: usize /* 32 KiB */,
    pub svg_elements: usize /* 2000 */, pub svg_depth: usize /* 16 */,
    pub navigate_label_bytes: usize /* 120 */,
    pub svg_id_bytes: usize /* 64 */,
}
impl Gateway { pub fn with_assistant(self, config: AssistantConfig) -> Self; }
```

With a Registry, every entry of every conversation also goes to her conversation log
(`ai6::AssistantLogStore`), written in order by a thread of its own that the router starts:
what the app shows, each tool call with its arguments (`tool_call`), the size, status and time of
its answer (`tool_result`) and each change of a proposal (`proposal_status`); an entry from her
harness or tools carries the `AssistantTurn` that ran it, the report's `AssistantRun` giving the
pool account, model and tokens. The person's access token and her tool token are replaced with
`[redacted]` wherever they appear. The log is trimmed every `log_prune_every` (1 hour) to
`log_age` (90 days) and `log_bytes` (1 GiB), and flushed when `serve_loopback` stops. A conversation asked for in a hello that the
gateway does not hold is rebuilt from the log when it is that person's, with the Station,
provider Session and route of its last turn; a proposal that was waiting shows as expired.

The command reads `[assistant.limits]`, `[assistant.gateway]`, `[assistant.views]` and the
`cables` record at start. Defaults: 20 conversations a person and 2,000 in all (oldest idle one
dropped first), each kept until untouched for 7 days with its newest 500 entries and at most
1 MiB of them (4 MiB a person: the person's oldest views are emptied first, then their oldest
entries dropped); tool tokens accepted for one hour (`[assistant.gateway] token_ttl`), and past it for a conversation's
current token while her answer runs, its Station heard from within `answer_deadline` and no Stop asked;
a Station has 60 seconds to take up a message, and an accepted message is given up (an
`error` entry, `busy` false) after 10 minutes without a report from its Station; 120 tool
calls a conversation per minute; 2 conversations a person with a harness running or an answer
pending. Past that last bound, a new message closes the person's least recently used idle
harness (a `close` job to its Station, and the conversation's tokens withdrawn); when every
other one is answering, the message is refused with `busy_elsewhere`. Conversations live in the
gateway's memory only and are lost when it restarts (the provider Session on the Station
survives, but the app shows a new conversation).

The model runs on a Station, as the roster Agent named here (see `ai6::AssistantJob`), not in
the gateway. Each message from the app is queued as an `AssistantJob` for the conversation's
Station, else the Headquarters Station, else any healthy Station (checked in within 45 s) by
ID; a Station that refuses it (`rejected`) or does not take it up within the answer timeout is
passed over, and with none left the app gets an error entry naming the last reason. A job
carries the message (prefixed with `[App page: ROUTE]`, the app route the message was sent
from in its link form, `app:/<page>?k=v&…`, when the message's `route` is one; nothing when it
is not), Moneypenny's instructions and the field dictionary
(`docs/organization.md`) as context, a recap of the conversation's last 8 KiB apart as
`recap` (a Station gives it only to a harness that starts afresh, never on resume), the provider
Session to resume, and an MCP token bound to the person, the conversation and the Station the
job is placed on. The token is set when the job is placed: the conversation's token for that
Station is reused while more than 10 minutes of it remain (so a running harness keeps the
token it started with), otherwise a new random one is made; placing the job on another
Station withdraws every token the conversation holds for any other. A Station is served its
`close` and `interrupt` jobs before messages; the `accepted` answer to either is taken as is,
and a `rejected` interrupt becomes an `error` entry with the Station's reason.

The app's `{"type":"interrupt"}` stops the answer of the socket's own conversation (a socket
only ever holds a conversation of its signed-in person). With nothing being answered it is
answered `{"type":"interrupt","stopping":false}` and changes nothing. Otherwise it is answered
`stopping: true`, Moneypenny is told `[App: the person stopped your last answer.]` with the
next message, and either a message no Station has accepted is withdrawn at once (a late
`accepted` for it is refused `409`, so its Station closes what it started; when a Station had
taken it, the conversation's tool tokens are withdrawn and that Station is cut off
(`Conversation.cut_off`) until it accepts a later message: its Cable standing answers `send`
and `read` false, and of its reports only `cable_sent` events are taken, for the audit log), or one
`interrupt` job is queued for the Station running the harness. The answer ends, as a `notice`
entry `You stopped Moneypenny's answer.` and `busy` false, when the message is withdrawn or
that Station reports `interrupted`.

Her tools are served on `POST /v1/moneypenny/mcp` (MCP Streamable HTTP, protocol
`2025-06-18`, `2025-03-26` or `2024-11-05`; one JSON response per request, `202` for
notifications, no server stream). Only a token a job carried is accepted (`401` otherwise,
including a person's access token or the Station credential). Each tool is a request to the
gateway's own routes with the person's current access token, so it is authenticated and
authorized exactly as the app's request (`403` becomes "Refused: …", `401` "sign in again").
Inputs are checked against the tool's schema (unknown properties refused) before anything
runs, and identifiers are percent-encoded into paths. Results are JSON inside
`<ai6-data tool="NAME">…</ai6-data>` with `<` escaped, cut at 200,000 characters. Tools:
`agent_roles`, `audit`, `board` (`/v1/stations` and `/v1/board`), `class_authorizations`,
`compartments`, `field_guide` (the `docs/` pages, compiled in), `green_ink`, `held_briefs`, `mission`,
`mission_proposals`, `missions`, `placement_preview`, `pools`, `profile`, `quarantines`,
`roster`, `source_item` and `source_items` (the `source-item` and `source-items` routes: an
Operation's tracker items read live), `station_settings`, `timeline`; all read only. `propose_shared_change` is listed
with the write tools.

Write tools only propose (an input whose text holds a character AI6 vetting counts as invisible is refused, `INVALID_INPUT`): `propose_issue_comment` (issue.comment), `propose_issue_create` (issue.create), `propose_issue_link` (issue.link), `propose_issue_proposal_decision` (issue.create, `mark_ready` also mission.create), `propose_issue_update` (issue.update), `propose_class_authorization` (approval.class), `propose_held_brief_decision`
(release or refuse; mission.create), `propose_mission` (mission.create),
`propose_mission_control` (cancel, retry, resume, interrupt; mission.control; the proposal ID
is the idempotency key), `propose_mission_proposal_decision` (mission.create),
`propose_roster_edit` and `propose_roster_removal` (roster.manage on `config:roster`; kind `assistant`, key `assistant`, is her own settings: the edit is checked as `AssistantSettings` before it is proposed, and its summary ends `Weakens: …` with `AssistantSettings::changes_from` the settings in force, read from the roster as she proposes; none is proposed when they cannot be read),
`propose_station_settings` (station.configure on `station:<id>`). Each checks its input and
the person's permission through the Authorizer first: without it the tool answers "Refused"
and records nothing. Otherwise it records a proposal (ID `proposal-<hex>`, the exact method,
path and body, a summary, the permission it needs) and shows it as a `proposal` entry; the
model is told nothing changed. There is no tool to confirm and none to sign green ink. The
app sends `{"type":"confirm","proposal":ID}` or `{"type":"cancel","proposal":ID}`; a confirm
is taken only from a socket signed in with the access token the proposal was made under
(`forbidden` otherwise), for a `pending` proposal (`conflict` otherwise) less than an hour
old (then `expired`). It records the audit event `moneypenny.confirmed` (actor the person;
the Operation, or resource `config:roster` / `station:<id>`; target `CONVERSATION PROPOSAL
TOOL PATH`) and only then sends the request through the gateway's own route as the person,
which validates and audits as for the app; a confirmation that cannot be audited does not
run. The proposal becomes `done` or `failed` with AI6's answer, sent as
`{"type":"proposal","proposal":{id,tool,summary,request,needs,status,result}}`, and
Moneypenny is told (`[App: the person confirmed …]`) at once when she is idle, else with the
next message; a cancellation is told with the next message. 32 proposals are kept per
conversation.

Workspaces come from the `[assistant]` table of the roster `with_roster` serves (a
configuration directory's `agents.toml`, or a SQL roster's `assistant` record). With
`personal_repo` set, each message job carries `person_id` and `ai6::AssistantWorkspace`: the template filled
in for the person signed in to the conversation (a person whose identifier cannot fill
`{person}` gets an `unavailable` error instead of a job), the repositories of the person's
Directory groups and `org`, the author (`display_name` else `name`, without control
characters, `<` or `>`; `email` else `<16 hex>@people.ai6.invalid`). Every message job
carries, read from the roster for each message, `reach`: `AssistantSettings::reach_for(person,
groups)`, the deployment's scripts and network narrowed by the tables of the person's Directory
groups and their own; `skills`: `AssistantSettings::skills_for(person, groups)`, the catalog
(`[assistant.skills]`) less what the same tables leave out; and `groups` (the person's
Directory groups), so the Station re-caps both by its own roster's tables rather than trust the
gateway's narrowing. A conversation carries no mark of what she read; tool results stay framed in
`<ai6-data>`. A proposal carries `read`, the read tools called since
the person's last message, at most 16, each `{tool, ids}` with only the identifier arguments
`operation_id`, `mission_id`, `station_id`, `agent`, `kind` and `key`, and `opens`, the link
of the page it changes (`app:/missions?operation_id=G&mission_id=m-1`, none for a shared
change or Cable).
A confirmed shared change is audited
`moneypenny.confirmed` (resource `config:assistant`) and sent as a commit job to the
conversation's Station (else the first healthy one), staying `confirming` until the Station
answers; cancelled, nothing is sent. A
commit job's answer becomes a `notice` (or `error`) entry, settles its proposal `done` or
`failed`, and is told to her with the next message. A `notice` event becomes a `notice`
entry and a note for her.

`propose_shared_change {group, path, content, message}` (group a `file_safe` name or `org`,
path a repository path, content ≤ 64 KiB, message one line ≤ 200 characters, no hidden
characters) is refused ("Refused: …") unless the person is in the group and it shares a
repository (`org` is everyone's). It then waits up to `shared_base_wait` (5 s) for the
conversation's Station to
report the call's `ai6::AssistantSharedBase` (on a `tool` event's `shared`, matched by group,
path and the SHA-256 of `content`, used once; a conversation keeps at most `shared_bases`)
and is refused ("Nothing was proposed: …") without one. The
change carries it as `AssistantChange::base`, so the commit is refused when the file changed
on `main` since. With that repository's `write = true` it is audited
`moneypenny.shared.write` (resource `config:assistant`, target `CONVERSATION GROUP PATH`) and
sent at once as a `shared` commit job; otherwise it is a proposal (needs `membership` on
`group:<group>`) handled as above.

Cables. Each message's job carries `person_id` and `cables`: the Operations the Stations check
in with (`operations` of their check-in) where the Authorizer gives the person `cable.send` or
`cable.read`, at most 64 in Operation ID order, read again for every message. With any, the
Station gives her its Cable endpoint (see the `ai6` header, assistant desk), and asks `GET
/v1/station/assistant/cables` before every Cable call, which answers whether the person holds
`cable.send` and `cable.read` there now. Every Cable she sends or replies is held by the Station
for the person to confirm. A `cable_proposal` event becomes a proposal: tool
`send_cable` or `reply_to_cable`, request `CABLE <target>` with body `{operation, recipient,
ttl_seconds, body}`, summary naming the recipient and lifetime,
needs `cable.send` on `operation:<id>`, the session of the conversation's current access
token; a Cable whose ID, Operation, target or recipient is not a valid identity, whose digest
is not `AssistantCable::request_digest` of what is shown, whose body exceeds 16 KiB or holds a
hidden character, is an `error` entry instead. With 32 proposals pending or confirming, none
of which is ever dropped, a new one (write tool or Cable) is refused: a write tool answers so,
a Cable is an `error` entry and a `cable_discard` job for its Station. Confirming one checks
`cable.send` again (a refusal fails it, nothing audited or sent), records
`moneypenny.confirmed`, then queues a control job with `cable_confirm {id, digest}` and
`token_expires_at` one answer timeout away for the Station holding it; that Station's
`cable_sent` event makes it `done` (`Sent as Cable <id>.`), its `rejected` answer `failed` with
the reason, no answer within one and a half answer timeouts `failed` (the job withdrawn), and
Moneypenny is told each way. Cancelling one queues a `cable_discard` job. A control job's
answer is taken only from the Station it was given to (`403` otherwise, and it waits on). Every
`cable_sent` event is audited `moneypenny.cable.sent` (actor the person, the Cable's Operation,
target `CONVERSATION CABLE`).

Memory. `/v1/user/memories` (route table) serves every scope through
`ai6::directorates::m::memories`; `/v1/user/assistant/memory` is a person's own, and refuses an ID
of another scope as not there. What a person holds (`memories::PersonCaller`): their own memory; an
Operation's with `mission.read` there by the policy alone, or a compartment granting `memory.read`
(`CompartmentAccess::through`, audited `compartment.read`); the global one always; `memory.write`
on `operation:<id>` or `config:memory` to change and to decide proposals; a chief
(`LiveAccess::effective().is_chief`) sees every entry whatever its compartments. Anything that
does not answer is not held. `?person=<id>` on the person routes reads another person's memory only
with `assistant.memory.admin` on `config:assistant` (`403` otherwise), audited `memory.read` before
it is read; `PUT` and `DELETE` with another person are `403`. Every change is audited before it is
made: `memory.write`, `memory.forget`, `memory.compartments`, `memory.mark_cleared`,
`memory.proposed`, `memory.proposal.approved`, `memory.proposal.rejected` (an Operation's under its
`operation`, the global under `config:memory`, a person's under `config:assistant` with target
`PERSON ID`), and one that cannot be audited is not made (`503`). Her tools `memory_search`,
`memory_read`, `memory_write` and `memory_forget` run as the person (`moneypenny:PERSON`, the
conversation as session): her memory of the person at once, shown as a `notice` with `data.diff`;
an Operation's or the global one as a proposal, told as a `notice`. An entry an exposed Session
wrote that she is shown adds `memory:<id>` to the conversation (`Conversation::memory_marks`, at
most `[registry.limits.sessions] exposures`), which every later write of hers carries. A message's
job holds the newest summaries of her memory of the person and of the global memory in `context`,
fenced in `<ai6-data tool="memory_search">`, within `[assistant.gateway] memory_context_bytes` and
what `context_bytes` leaves after the rest of the job's context and recap. An input with a hidden
character is refused.

The `show` tool takes `{"view": VIEW}` in the strict view schema version 1 (serde, unknown
fields refused): `{"version": 1, "title"?, "blocks": [1..=24]}`, each block tagged by `kind`:
`text` (`markdown` ≤ 8 KiB; every `](` link must be an app route, `app:/<page>?k=v&…`), `table` (`columns` 1..=12,
`rows` ≤ 200 of one scalar per column), `key_values` (`items` 1..=100 `{key, value}`), `chart`
(`chart` `bar`|`line`, `title`/`x_label`/`y_label`?, `series` 1..=8 of `{name, points` 1..=200
`{x: scalar, y: finite number}}`), `card` (`card` `mission`|`station`|`theater`, `id`,
`operation_id` (required for a Mission), `state`?, `fields` ≤ 20), `timeline` (`items` 1..=100
`{at, label, detail?}`), `diff` (`title`?, `before`, `after` ≤ 16 KiB each), `form` (at most one:
`tool` one of the `propose_*` tools, `title`, `fixed` (object), `fields` 1..=20 `{name, label,
type: text|number|boolean|choice|json, value?, choices?}`; `choices` exactly for `choice`, no
name both fixed and a field). Scalars are text (≤ 512 bytes, no control characters), finite
numbers, booleans or null; the whole view ≤ 64 KiB. A view outside it answers `isError`
"View refused: REASON" and nothing reaches the app; a valid one becomes a `view` entry with
`data: {"id": "view-<hex>", "view"}`. The app's `{"type":"submit","view":ID,"values":{…}}`
fills the form's tool from `fixed` plus the values of its own fields (a `json` field's text is
parsed; a choice must be listed; another name is `bad_request`) and records a proposal as the
tool would, in the submitting socket's session (`refused` with the tool's answer otherwise).

The `navigate` tool (`readOnlyHint`) takes exactly `{"to": ROUTE, "label": TEXT}`: `to` an app
route in its JSON form, checked by `routes.rs` (its schema is flat, `page` plus every field of
any page; which fields a page takes is the parser's rule), `label` 1 to `navigate_label_bytes` bytes
without control characters but newlines. Anything else answers `isError` "Navigation refused:
REASON" and adds nothing; a valid call adds a `navigation` entry, `data: {"id": "nav-<hex>",
"route", "label"}`, and answers "Offered to the person as nav-…; the page
opens when they choose it." It is not audited. The `audit` tool's `operation_id` and `station_id`
are an Audit route's (not both).

### Pages she publishes

`Gateway::with_pages(PagesConfig { dir, origin, listen })` keeps pages (`{dir}/{id}/meta.json`
and `files/`) and serves them; `Gateway::pages_router()` is the router of the pages' own
listener (page routes only, `None` without an `origin`) and `serve_pages(address, shutdown)`
runs it. Without an origin the routes are `/pages/p/{id}/…` on the API router. Quotas and
options are the roster's `[assistant.pages]`, read at each use. Tools (listed by `tools/list`):
`publish_page {kind: dashboard|report|file, title, path | content+format, entry?, replace?,
feeds?}`, `unpublish_page {id}`, `pages`. A `path` is uploaded by the Station
(`PUT /v1/station/assistant/pages/{digest}/{path}?station=&conversation=`, then `POST …/{digest}/done`
with `{files: [AssistantPageFile]}` or `{refused}`; `digest` is `page_call_digest(path)`); the
call waits `upload_wait` for it and checks the manifest. Audit (`config:assistant` scope) comes
before the page exists. The person's routes: `GET /v1/user/pages`, `POST /v1/user/pages/{id}/open
{network?, handoff?}` → `{url: "{pages base}/p/{id}/?[network=&]handoff=", expires_at}`, a hand-off
(audited `moneypenny.page.handoff`; single-use, in memory only, `handoff_ttl` long and at most
`handoffs` per person, bound to the page, the person and the bearer's sign-in) or, with `handoff:
false`, the plain address; `POST …/remember {remember}`, `DELETE …`; any other person gets 404.
The first navigation to `/p/{id}/?handoff=` spends it (audited `moneypenny.page.handoff_used`),
sets the page cookie, on the pages' own origin too, and answers 303 to the clean address; a second
use, an ended token, another page's, or one for a sign-in that has ended answers as an unknown
page, and a subresource request, a prefetch or a `HEAD` neither spends nor honours it. Page routes: `GET /p/{id}/[?network=]` (the frame: the bar
and an `<iframe>` of `{token}/f/{entry}`; audited `moneypenny.page.opened`), `/p/{id}/{token}/f/{path}`
(a file; a top-level request redirects to the frame), `/p/{id}/{token}/data/{feed}` and
`…/f/data/{feed}` (`{data, at}`; 403 when feeds or the network are off or the viewer may not read
it, 429 past `feed_reads_per_minute`). Every one names its viewer (`page_access::viewer`): the
`AccessProviders::with_page_authenticator` provider, given the headers but `Cookie`, the TCP peer
and the certificates the client proved on `Gateway::serve_tls` (`Valid` names the person, `Unknown`
falls through, anything else refuses), else the page cookie `ai6_pages={sign-in id}.{HMAC}`
(`HttpOnly; Secure; SameSite=None; Path={pages base path}/`, set by `/v1/auth/login`, `resume` and
`renew` for a request with `Origin` when pages share the API's origin, dropped by `logout`; the
sign-in is checked with `login_by_id` each request), then checks `may_view`. `{token}` is
`{expires}.{network}.{HMAC of page, viewer, network, expires}` under `{dir}/.key`, `open_ttl`
long: it is no credential, and ties a file or feed request to the viewer's frame. An ended sign-in
answers 403 ("You are signed out…"); nobody, someone the page is hidden from, a token not theirs or
expired answer as an unknown page (404), the reason logged once per page and viewer (stderr). A
request with `Origin: null` (the page's own credentialed fetch) gets `Access-Control-Allow-Origin:
null` and `-Credentials: true`. Every page response carries the sandbox CSP (no
`allow-same-origin`), `nosniff`, `no-referrer`, `no-store`, CORP `cross-origin`; no `/v1` route
answers CORS or reads the page cookie, a client certificate or the page authenticator.
`tls_server_config(chain_pem, key_pem) -> Result<rustls::ServerConfig, String>` asks for a client
certificate it does not require and takes any whose key is proven (trust is the page
authenticator's); `Gateway::serve_tls(listener, config, shutdown)` serves HTTP/1.1 on it.

## Gateway state

```rust
#[derive(Clone)]
pub struct Gateway { /* shared, bounded state */ }

impl Gateway {
    pub fn new(config: GatewayConfig) -> Self;
    /// Any Registry store; the command opens the one `ai6.cfg` names.
    pub fn with_registry(config: GatewayConfig, registry: Arc<dyn ai6::RegistryBackend>) -> Self;
    pub fn with_access(
        config: GatewayConfig,
        registry: Arc<dyn ai6::RegistryBackend>,
        access: AccessProviders,
    ) -> Self;
    /// Serves `/v1/user/roster/...` from the roster the Stations read; without it those
    /// routes answer 503. The command opens `ai6.cfg`'s provider with ai6's config::open_with.
    #[must_use]
    pub fn with_roster(self, source: Arc<dyn ai6::ConfigProvider>) -> Self;
    /// How long a Station's assistant job poll is held while Moneypenny is not served
    /// (`[assistant.gateway] max_wait`); once she is, her config decides.
    #[must_use]
    pub fn with_job_poll_max_wait(self, max_wait: Duration) -> Self;
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
    pub fn terminal_bytes(&self, session_id: &SessionId) -> usize;
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
    pub async fn receive_agent_input_within(&self, wait: Duration) -> Option<AgentInput>;
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

`attach`'s replay holds a `TerminalGap` (`ai6-diptel`) ahead of any frame that follows a position or
a frame with a sequence skipped between them, and a frame published past a skipped sequence is
preceded by one on the live stream; the gateway keeps no record of gaps but its history.
`terminal_bytes` is the most terminal output one event of the Session may carry, at any sequence;
`POST /v1/station/sessions` answers it beside `next_sequence`, and a Station splits longer output.
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
shutdown)` binds to `127.0.0.1` until the caller's graceful-shutdown future resolves, then waits
up to `[assistant.gateway] log_flush_timeout` (10 s; `none` for all of it) for Moneypenny's
conversation log to keep what is queued, logging how many entries were left when it gives up.
The browser-facing HTTPS reverse proxy must connect to that listener, remove any
client-supplied `X-Forwarded-Proto`, and set the header to `https`.

| Method and path | Contract |
| --- | --- |
| `GET /health` | Returns `204 No Content` while the gateway is serving. |
| `GET /v1/board` | Requires a user bearer and returns Sessions for Operations allowed by `mission.read`, each with the `station_id` of its Registry record and the `agent` of its Mission's brief (`null` when unknown). |
| `GET /v1/stations` | Requires a user bearer; returns visible Station health, hosted Operations and current Headquarters Station. |
| `GET /v1/user/operations/{operation_id}/placement-preview` | Requires `mission.read` at the Operation; returns the eligible (Station, Theater, Agent) choices, best first, and a reason for each rejection. Optional `agent`, `role`, `stations`, `tags` query parameters narrow it. |
| `GET /v1/missions` | Requires a user bearer; groups registered Sessions by Mission and filters each Operation through `mission.read`. `?operation=P` keeps P's Missions; with `&subtree=true`, those of P and of every Operation under it. `?mission=ID` keeps the one Mission `ID` names. |
| Any route with `{mission_id}` (and `held-briefs/{key}`, `missions/{id}/products`, the timeline's `?mission=`) | Takes the Mission's UID, the key of a brief dropped by hand, or any handle it was ever given, compared without case (`ai6::BriefStore::resolve_mission`); an ID the Registry does not know is taken as it is. Answers carry `mission_id` (the UID) and `handle` (the current one). |
| `POST /v1/user/operations/{operation_id}/missions/{mission_id}/verdict` | Requires `mission.verify`; body `{"verdict": "confirmed" \| "disputed" \| "failed-verification", "note"?}`, unknown fields refused. Puts it on the latest attempt, replacing any it has, and returns `{operation_id, mission_id, attempt, session_id, verdict, note?, by, at}`; a refusal is `{"error": TEXT}` with `400` (malformed), `409` (the attempt has not ended done or held) or `404`. Audited `mission.verdict` first (resource `mission:UID`, target `UID VERDICT`, never the note), timeline kind `verdict` action the verdict. A Station settles a held or verifying Mission by it; `redispatch` is refused here (`400`): it goes by `verify`. |
| `POST /v1/user/operations/{operation_id}/missions/{mission_id}/verify` | Requires `mission.verify`; body `{"decision": "done" \| "redispatch" \| "failed", "reason", "amendment"?}` (`reason` required, `amendment` only with `redispatch`), unknown fields refused. Decides a Mission whose latest attempt is `verifying`: it becomes the attempt's verdict (`confirmed`, `failed-verification` or `redispatch`, the reason as its note), and its Station then settles the Mission done or failed, or queues attempt N+1 told the reason and amendment. Returns `{operation_id, mission_id, attempt, session_id, decision, verdict, note, amendment?, by, at}`; `400` (malformed), `409` (the latest attempt is not verifying) or `404`. Audited `mission.verify` first (resource `mission:UID`, target `UID DECISION`, never the reason), timeline kind `verify` action the decision. |
| `POST /v1/user/operations/{operation_id}/missions/{mission_id}/probe` | Requires `mission.verify`, no body. Asks the Station holding a verifying Mission to read back what it was to deliver (`branch`, `commit_on`, `comment`, `artifact`, `approval`, `file`) on its next pass; returns `{operation_id, mission_id, attempt, session_id, requested_at}`, `409` unless the latest attempt is verifying. Audited `mission.probe`. The findings are the latest attempt's `evidence` (`{deliverable, result: found \| missing \| unprobed \| failed, detail, at}`) in the Mission's detail beside its `deliverables` (`kind` or `kind:target`), and `probe_requested` clears when the Station has answered. |
| `POST /v1/user/operations/{operation_id}/missions/{mission_id}/rename` | Requires `mission.control`; body `{"handle"}`. Returns `{operation_id, mission_id, handle, previous}`; a refusal is `{"error": TEXT}` with `400` (malformed), `409` (a handle another Mission had) or `404` (no such Mission in the Operation). Audited `mission.rename` (resource `mission:UID`, target the UID), timeline kind `control` action `rename.done` with details `handle` and `previous`. |
| `GET /v1/operations/{operation_id}/missions/{mission_id}` | Requires a user bearer; returns one authorized Mission, its AI6-published brief when available, its `usage` (tokens and cost per Session and UTC day) once a provider has reported any, and its `attempts` from the shared Session records. `GET /v1/missions` carries `attempts` too. |
| `GET /v1/auth/methods` | No token. `{"methods": [...]}`: `password` when the Authenticator takes a name and a secret (it says so in `describe`, or says nothing), `proxy` with a proxy identity header. Names no account. |
| `POST /v1/auth/login` | Resolves the sanitized identity header from the trusted reverse proxy, or uses the configured Authenticator and Directory; returns a one-time user bearer secret, `expires_at`, `expires_in` (seconds left, for a client that does not trust its clock) and `ends_at`. `ttl_seconds` is optional, default `[gateway] access_token_ttl`, at most `access_token_max_ttl`. A request with `Origin` (checked) also gets `Set-Cookie: ai6_session=…; Path=/v1/auth; HttpOnly; Secure; SameSite=Strict; Max-Age=…`, ending `session_max` after signing in, unless `[gateway] session_cookie` is `false`, `session_max` is `0` or the body says `"session_cookie": false`; and, when pages share the API's origin, the page cookie `ai6_pages` (see Pages she publishes), which `resume` and a renewal with `Origin` set again and `logout` with `Origin` drops. |
| `POST /v1/auth/resume` | Takes the `ai6_session` cookie, not a bearer. `Origin` must be the public origin (`403`). Answers `201` as login does, with a new token of the same sign-in: its time still runs from signing in, and its renewals stop at the same `ends_at`. `401` and the cookie cleared when it is refused: not active, past the end, the person gone from the directory, a proven proxy identity naming someone else, or the setting off; `401` with nothing cleared when no cookie came. Refused cookies are counted by address alone (`login_tries_per_address`), apart from login tries, and past that answer `429`. |
| `POST /v1/auth/logout` | Revokes the presented user token across gateways sharing the Registry. With an `ai6_session` cookie from the public origin it also takes that, and the token may then be absent or over; `204`, `401` when it ended nothing, and a cookie sent is cleared either way. A token or cookie of a sign-in takes the whole sign-in: the cookie and every token issued from it. |
| `POST /v1/auth/renew` | Moves the presented token's expiry on by `[gateway] access_token_ttl`, up to `session_max` after it was issued, never earlier; the value does not change. `Origin` is checked when present; it is not counted as a login try. `200 {token_id, expires_at, expires_in, ends_at}`, no secret (`ends_at` is `null` with no bound); `401` when not active or the person has left the directory. |
| `GET /v1/user/profile` | Requires a user bearer; returns directory identity plus effective Operation and Station actions. Operation grants are those of `ai6::OPERATION_ACTIONS` the person holds, in its order, and Station grants those of `ai6::STATION_ACTIONS`; `roster` lists the person's `roster.read`, `roster.manage` and `audit.read` on `config:roster`. Every action is computed by the Authorizer, not inferred from directory groups. Operations come from the Stations' `operations` and their tree (`operation_lineage`); each `operations` entry carries `parent`, the nearest Operation above it the person may read (omitted for a root or when every ancestor is hidden). `moneypenny` says whether this gateway serves her (`assistant = false` does not). |
| `GET /v1/user/roster/operation/{id}/effective` | Requires `roster.read` on `config:roster` and a roster source (else `503`); `404` for an Operation the roster does not hold. Returns `EffectiveOperation`: its `lineage`, `inherit`, the `definition` its Stations resolve (its ancestors' keys layered in, before presets and defaults) and `origins`, the Operation that wrote each dotted key. |
| `GET /v1/user/audit?operation_id=<id>&station_id=<id>&limit=<n>` | Requires a user bearer and `audit.read`; the optional Operation and Station filters are mutually exclusive. Returns at most 500 newest metadata-only records, filtered by the Authorizer for each resource. Records contain actor, action, resource scope, target and time; prompts, terminal output and secrets are excluded. |
| `GET /v1/user/operations/{operation_id}/timeline?cursor=&mission=&limit=` | Requires `audit.read` on the Operation; returns a `TimelinePage` (see Activity timeline). |
| `GET /v1/user/operations/{operation_id}/source-item?tracker=&item=` | Requires `mission.read`; reads the item live through the Headquarters Station (the gateway holds no tracker token): `{item: SourceReadItem, comments: [SourceReadComment] /* the latest `[assistant.gateway] source_comments` */, missions: [UID] /* active links to it */, error}`. `504` when no Station answers within `source_wait`, `502 {error}` when it could not read. |
| `GET /v1/user/operations/{operation_id}/source-items?tracker=&query=` | Requires `mission.read`; `{items: [SourceReadItem], error}` of what the Operation's sources find now (at most `source_items`, titles holding `query`), as `source-item` relays it. |
| `GET /v1/station/source-reads/next?station=&wait_ms=` | Station token. The oldest waiting `ai6::SourceRead`, or `204` after `wait_ms` (at most `[assistant.gateway] max_wait`). The Headquarters Station asks while it holds the lease. |
| `POST /v1/station/source-reads/{id}/result?station=` | Station token; body `ai6::SourceReadAnswer`, at most `source_answer_bytes`. `204`, `404` when nobody waits for it any more. |
| `GET /v1/user/operations/{operation_id}/pools` | Requires `pool.read` on the Operation; returns a `PoolBoard { operation_id, accounts: Vec<BoardAccount> }`: each account a Station declared for the Operation with `pool`, `account`, `handler`, `kind`, `priority`, `max_concurrent`, `labels`, `active` (`BoardClaim { station_id, mission_id, since }`, this Operation's live claims only), `other_sessions` (the count of other Operations' live claims), `windows` (`BoardWindow { name, used_permille, resets_at, status }`), `limited_until` (only while in the future), `last_used`, `tokens_today` (UTC) and `login` (`BoardLogin { stations, expires_at, renewed_at, renewal_held_by, failures, needs_person }`, for a login sealed in the Registry). Never a credential, its secret's name, a login directory or a sealed value. |
| `GET /v1/user/roster/{kind}` | `kind` is `operations`, `agents`, `roles`, `covers`, `assistant` (Moneypenny's `[assistant]` settings: at most one record, keyed `assistant`) or `roster` (the roster-wide `[roster]` settings, such as `max_operation_depth`: at most one record, keyed `settings`); else `404`. Requires `roster.read` on `config:roster`; returns `RosterRecords { kind, revision, writable, records: Vec<RosterRecord { key, definition, revision }> }`: each definition is the JSON object of its table in the configuration files, its `revision` a digest of it for `If-Match`, the list's `revision` the roster revision in decimal (compare `BoardStation.config.revision`), `writable` whether edits are taken. `503` without a roster source. |
| `GET /v1/user/roster/{kind}/schema` | Requires `roster.read` on `config:roster`; `404` for any other kind. Returns `ai6::RecordForm { schema, form, key { title, description, fixed? } }`: the record's JSON Schema and Guatiao form, as its type declares them. A `format` of `record:<kind>` names another kind's record, `reported:handlers`, `reported:theaters` or `reported:stations` what Stations report, `used:<field>` what other records of the kind hold there, `settings-of:<field>` settings for the provider that field names, and `seconds` and `bytes` what a number counts. A record keyed `schema` is still written with `PUT` and `DELETE` on this path. |
| `PUT /v1/user/roster/{kind}/{key}` | Body: the record's definition, a JSON object (else `400`). Requires `roster.manage` on `config:roster`. The source checks the whole roster first: `200` `RosterChange { revision, weakened }` (`weakened`, omitted when empty: for a stored `assistant` record, `AssistantSettings::changes_from` the settings the source held just before it) and a `roster.update` audit event; `422` `{"refused": reason}` and a `roster.refused` audit event (target `<kind> <key>: <reason>`) when the roster it would leave is not acceptable, nothing changed; `409` for a read-only source (a configuration directory). With `If-Match: "<revision>"` (the record's, or `0` for a record that must be new) a record that no longer has it answers `409` `{"changed": revision}` and nothing changes; without it what is there is replaced. Audit resource `config:roster`, target `<kind> <key>`. Every Station takes up an accepted edit at its next poll. |
| `DELETE /v1/user/roster/{kind}/{key}` | As `PUT`, `If-Match` too: `200` with the revision and a `roster.remove` audit event, `404` when there is no such record, `422` when the removal leaves the roster unacceptable (an Agent another extends, an Operation with subordinate Operations). |
| `POST /v1/user/operations/{operation_id}/pools/{pool}/{account}/clear-limit` | Requires `pool.manage` on the Operation; lifts the account's limit (`404` unless a Station declared it for the Operation) and records a `pool.limit.clear` audit event with target `pool/account`; `204`. |
| `GET /v1/user/operations/{operation_id}/agent-roles` | Requires `agent.read`; returns the Headquarters Station catalog merged with validated Operation-scoped Registry overrides. Each role includes `operation_override` provenance. |
| `PUT /v1/user/operations/{operation_id}/agent-roles/{role_id}` | Requires `agent.configure`; validates and stores the role override and records only actor/action/Operation/role ID/time in audit. |
| `DELETE /v1/user/operations/{operation_id}/agent-roles/{role_id}` | Requires `agent.configure`; removes the override, falls back to the Station definition and records a metadata-only audit event. |
| `GET /v1/user/operations/{operation_id}/agents` | Requires `agent.read`; returns `Vec<AgentView>` by codename: every Agent the Headquarters Station's check-in posts to the Operation (`operation_policies`) and every Agent with a record there. `AgentView { agent, posted, operation: AgentTally /* this Operation */, every_operation: AgentTally /* outside any Operation, and quarantines in every one */, by_clearance: BTreeMap<String, AgentTally> /* this Operation */, last_mission_at /* this Operation */, model_history: Vec<ModelTenure> /* this Operation, each with its Sessions' tally; a model with no finished Mission here is not in it */, routes: Vec<AgentRoute { handler, model /* None: the record sets none, the handler's default */ }> /* what the roster record says, from the HQS check-in's `operation_policies`; empty without `roster.read` and for an Agent not posted */, retirement: Option<RetirementNotice { retire_on, on_retire, status /* retiring, desk_duty, retired */ }> /* from the HQS check-in's `retirements` */, clearance, configured_clearance /* from the HQS check-in's `agent_clearances` */, grant: Option<ClearanceGrant>, promotion: Option<ApprovalRequest> /* M's pending promotion: kind `clearance`, Session `m`, Mission `promotion-AGENT`; a clearance request an Agent filed itself is not shown */, grant_unreadable: bool /* then clearance is "desk" */ }`. Nothing of other Operations is shown. |
| `PUT /v1/user/operations/{operation_id}/agents/{agent}/clearance` | Requires `agent.configure`; `{agent}` is a codename or unique alias the Headquarters Station's check-in (`agent_clearances`) lists for the Operation, stored under the codename; `404` for any other name; body `{"clearance": "desk"\|"field"\|"00", "reason": 1-512 characters, "expires_in_secs"?: 1..=31536000}`, unknown fields refused. Stores a lowering `ClearanceGrant` (signer = caller, no approval: it never raises the Agent above its configured clearance), replacing any grant there and revoking the approval of a replaced raise; returns the grant. Audited first as `agent.clearance.lowered` (target `AGENT CLEARANCE`). |
| `GET /v1/user/operations/{operation_id}/agents/scorecard[?window_days=N]` | Requires `agent.read` on the Operation **and** `roster.read` (`403` otherwise: it shows which handler and model each Agent ran on); returns an `ai6::Scorecard` computed from the Registry's Session records and usage ledger: per Agent, per route within it and per model, each `Standing` listing the attempts behind every judgment. The window and small-sample line come from the Operation's `scorecard` settings in the HQS check-in (`operation_policies`), defaults before it checks in; `window_days` overrides the window, `0` covering every attempt on record. |
| `GET /v1/user/models` | Requires `roster.read`; returns what every Station last reported at check-in under `models`, newest look first: `[{station, source /* pool/account, or agent/handler/model for a route no pool serves */, pool?, account?, agent?, route?, handler, listing: {known, unknown_because?, models: [{id, name?, description?}]}, at}]`. A handler that cannot list has `known: false` with why, and no models; nothing is guessed, and no credential is in a report. It makes no route. |
| `DELETE /v1/user/operations/{operation_id}/agents/{agent}/hold[?handler=H[&model=M][&pool=P]]` | Requires `agent.configure`; lifts the hold on the Agent, or with `handler` on that one route, and returns it; `400` for a `model` or `pool` without `handler`, `404` when there is no such hold. Audited first as `agent.hold.lifted`. `AgentView.holds` lists an Agent's holds; a route hold is left out without `roster.read`. Signing M's proposal of kind `hold` also needs `agent.configure`, and revoking its approval lifts the hold it put on. |
| `DELETE /v1/user/operations/{operation_id}/agents/{agent}/clearance` | Requires `agent.configure`; `{agent}` resolves as for `PUT`, else is taken as a codename; removes the grant (the Agent works at its configured clearance) and revokes the approval of a removed raise; returns it, `404` when there is none. Audited first as `agent.clearance.revoked`. Raising a clearance is only ever M's promotion request, approved through green ink. |
| `GET /v1/user/operations/{operation_id}/quarantine` | Requires `agent.read`; lists the Agent quarantines that apply to the Operation (its own and those covering every Operation) with reason, who raised them and when. |
| `POST /v1/user/operations/{operation_id}/quarantine/{agent_id}` | Requires `agent.configure`; body `{"reason": "..."}` (1-512 characters); quarantines the Agent in this Operation and records an `agent.quarantine` audit event. |
| `POST /v1/user/operations/{operation_id}/quarantine/{agent_id}/release` | Requires `agent.configure`; lifts this Operation's quarantine of the Agent (`404` when there is none; a quarantine of every Operation is released on a Station) and records `agent.quarantine.release`. |
| `GET /v1/user/operations/{operation_id}/green-ink` | Requires `approval.decide`; returns the Operation's `ai6::ApprovalRequest`s waiting for a decision, oldest first (expired ones are recorded as such and left out). |
| `POST /v1/user/operations/{operation_id}/green-ink/{request_id}/approve` | Requires `approval.decide`, and also `agent.configure` for a promotion request (kind `clearance`); body `{"expires_in_secs"?: 1..=31536000 (default 3600), "note"?: up to 1,024 characters}`, unknown fields refused. Signs as the caller and returns the `ai6::Approval`; the same decision again returns it, `409` when it was rejected or expired, `404` for an unknown request. Audited as `green_ink.approved` (target: `REQUEST_ID KIND TARGET`, never the reason). |
| `POST /v1/user/operations/{operation_id}/green-ink/{request_id}/reject` | Requires `approval.decide`; body `{"reason"?: ...}`; as approve, audited as `green_ink.rejected`. The reason reaches the Agent. |
| `GET /v1/user/operations/{operation_id}/green-ink/expired?since=UNIX` | Requires `approval.decide`; returns the `ai6::Approval`s that expired undecided at `since` or later, the newest first, at most `[registry.limits.approvals] expired_listed`; `since` defaults to `[text] green_ink_expired_window` ago. |
| `GET /v1/user/operations/{operation_id}/green-ink/sources` | Requires `approval.decide`; returns the Operation's `ai6::SourcePage`s, `{provider, web, project?}`, for linking what a reason names; empty on a gateway with no roster. |
| `POST /v1/user/operations/{operation_id}/green-ink/{request_id}/rearm` | Requires `approval.decide`; body `{"expires_in_secs"?, "note"?}`, unknown fields refused. Re-opens a request that expired undecided and returns the `ai6::ApprovalRequest`, waiting until `expires_in_secs` from now (default as long as it first waited); `409` for one a person decided or still waiting, `404` for an unknown request. Audited as `green_ink.rearmed` before it is applied (target as approve, never the note), timeline kind `green_ink` action `rearmed`. The Mission's Station takes it up. |
| `POST /v1/user/operations/{operation_id}/green-ink/{request_id}/revoke` | Requires `approval.decide`; optional body `{"reason"}`, kept as `revoked.note`. Withdraws an approval before it expires (again is harmless and not audited again) and returns the `ai6::Approval` with `revoked`; `404` for an unknown or undecided request, `409` for a refusal. Audited as `green_ink.revoked` before it is applied, so a revocation that cannot be recorded (`500`) is not made; the Station revokes a credential leased against it.  Revoking a promotion (kind `clearance`) also removes the clearance grant it raised, while it is still the Agent's grant, audited as `agent.clearance.revoked`. |
| `GET /v1/user/operations/{operation_id}/class-authorizations` | Requires `approval.class`; returns every `ai6::ClassAuthorization` of the Operation, oldest first, revoked and expired ones included. |
| `POST /v1/user/operations/{operation_id}/class-authorizations` | Requires `approval.class`; body `{"kinds", "target_pattern", "agents"?, "roles"?, "proposed_roles"?, "clearance_min"?, "max_uses"?, "expires_in_secs"}` (at least one Agent or role; `"*"` is every Agent; for kind `proposal`, `roles` match the proposing Session's own Mission role and `proposed_roles` lists the other roles a proposal may ask for). Signs as the caller, returns `201` and the record, audited as `class_authorization.created` (target: `ID KINDS PATTERN`). |
| `DELETE /v1/user/operations/{operation_id}/class-authorizations/{id}` | Requires `approval.class`; revokes it (again is harmless) and returns the record, audited as `class_authorization.revoked`. |
| `GET /v1/user/operations/{operation_id}/mission-proposals` | Requires `mission.create`; returns at most 100 pending `ai6::MissionProposal`s, oldest first, each with `vetting` once Headquarters has vetted it (absent until then). |
| `POST /v1/user/operations/{operation_id}/mission-proposals/{proposal_id}/approve` | Requires `mission.create`; promotes the proposal to the queue (`202`, `{operation_id, proposal_id, mission_id, state: "queued"}`) and clears the promoted brief of every vetting finding kind but `malformed` (the operator saw them) in the same Registry step (`promote_cleared`), and audits `mission.proposal.approved`. `409` for a rejected proposal. |
| `POST /v1/user/operations/{operation_id}/mission-proposals/{proposal_id}/reject` | Requires `mission.create`; files the proposal as rejected and audits `mission.proposal.rejected`; `409` for an approved one. |
| `GET /v1/user/operations/{operation_id}/missions/{mission_id}/issues` | Requires `mission.read`; `MissionIssues { issues: [IssueView { issue, tracker, item, url, relation, state, linked_by, title, issue_state, labels, phase, open_question }], entries: [EntryView { id, kind, issue, actor, state, url, reason, dropped, at }], replies: ReplyCounts { delivered, pending, ignored } }`; a Mission nothing in this Operation names is `404`. Titles and labels are untrusted text; entries carry no body. |
| `POST .../missions/{mission_id}/issues` | Requires `issue.link`; body `{"issue":"TRACKER:ITEM","relation"?:"related"\|"child"\|"parent"\|"mirror"}`. Writes the link active and a `link` entry the Headquarters Station resolves: a foreign or missing issue comes back unlinked. Audited `issue.link`. |
| `DELETE .../missions/{mission_id}/issues/{reference}` | Requires `issue.link`; unlinks, refusing what still waits to write to it. `404` for a reference not linked to this Mission, before audit, as on every route below. Audited `issue.unlink`. |
| `POST .../issues/{reference}/accept` | Requires `issue.link`; accepts a link an Agent proposed, `409` when it is not proposed. Audited `issue.link.accepted`. |
| `POST .../issues/{reference}/comments` | Requires `issue.comment`; body `{"body"}`. Refused `400` for a token shape, invisible characters, or past the Operation's `max_body_bytes`; links and mentions are the person's. Takes the Mission's and the Operation's post buckets (`429`). Footed with the person's name. On an issue with an open question it is the answer: a reply with `operator: true`, which resumes the Mission unexposed, and the answer says `answered: true`. Audited `issue.comment.queued`, never the text. |
| `POST .../issues/{reference}/update` | Requires `issue.update`; body `{"add_labels"?,"remove_labels"?,"transition"?,"close"?,"reopen"?,"add_assignees"?,"remove_assignees"?}`. No green ink. A label that starts work also needs `mission.create`. Audited `issue.update.queued`. |
| `POST /v1/user/operations/{operation_id}/issues` | Requires `issue.create` (a label that starts work also `mission.create`); body `{"tracker","title","body"?,"labels"?,"issue_type"?,"priority"?,"parent"?,"mission"?}`. `202`: an approved proposal the Headquarters Station sends. `parent` needs `mission`. Audited `issue.create.queued`. |
| `GET /v1/user/operations/{operation_id}/issue-proposals` | Requires `mission.read`; every `IssueProposal` of the Operation, oldest first, with the exact text, findings, dropped fields and how it came out. |
| `POST .../issue-proposals/{id}/approve` | Requires `issue.create`; body `{"mark_ready"?}`, which also needs `mission.create`. Returns the proposal; `404` for another Operation's, `409` once decided. Audited `issue.create.approved`. |
| `POST .../issue-proposals/{id}/reject` | Requires `issue.create`; body `{"reason"?}`. Audited `issue.create.rejected`. |
| `GET /v1/user/operations/{operation_id}/held-briefs` | Requires `mission.create`; returns at most 500 `HeldBriefEntry { key, operation, brief, vetting, held_at }` (the `ai6::HeldBrief` flattened beside its Registry key), oldest first. The brief carries its untrusted text; show it only on request. |
| `POST /v1/user/operations/{operation_id}/held-briefs/{key}/release` | Requires `mission.create`; no body. Returns the brief to the queue cleared of the kinds it was held for, for its content as it is (an edited brief is vetted again), and returns `HeldBriefDecision { operation_id, key, state: "queued" }`; releasing again returns the same; `404` when the Operation neither holds nor released such a brief, `409` once it was placed or finished. Audited as `mission.brief.released` (target: the key) and recorded on the timeline as `intake` `released`. |
| `POST /v1/user/operations/{operation_id}/held-briefs/{key}/refuse` | Requires `mission.create`; body `{"reason"?: up to 1,024 characters}`. Files the brief as failed with outcome state `refused` (a Source gets its CX report; the reason stays in the Registry) and returns `state: "refused"`; refusing again returns the same, `409` once it was placed or finished. Audited as `mission.brief.refused`, timeline `intake` `refused`. |
| `GET /v1/user/operations/{operation_id}/missions/{mission_id}/reviews` | Requires `mission.read`; returns `Vec<ReviewView { gate, policy, pass_at, distinct_models, rounds, redo_limit, status, why?, iteration, round, sent_back, panel, votes: [VoteView { iteration, round, agent, model, weight, vote, reasons, at }], turns, tokens, overrule? }>` of the Mission in that Operation; reasons are reviewers' words, data. |
| `POST /v1/user/operations/{operation_id}/missions/{mission_id}/reviews/{gate}/overrule` | Requires `mission.review`; `gate` is `plan` or `output`; body `{"decision": "pass"|"redo"|"refuse", "reason": 1..2,048 bytes, no control characters}`. Only while the review is `reviewing`, `waiting` or `person` (`409` otherwise, and when another write came first); a redo counts as a send-back and its reason goes to the author. Audited `mission.review.overrule` (target `MISSION:GATE:DECISION`), timeline kind `review` action `overruled`; the Station holding the Mission acts on it at its next round. |
| `GET /v1/user/stations/{station_id}/settings` | Requires `station.read`; returns the live concurrent Mission limit, `min_free_memory_mb` and `max_cpu_percent` (defaults 512 and 95 when unset), whether it has an explicit Registry override, and whether the Station has the execution role. |
| `PUT /v1/user/stations/{station_id}/settings` | Requires `station.configure`; execution Stations only; body `{"max_concurrent_missions", "min_free_memory_mb"?, "max_cpu_percent"?}` (an absent limit keeps its current value); validates a 1–1,024 concurrent Mission limit and a `max_cpu_percent` of at most 100, persists them in the shared Registry, and records a Station-scoped metadata-only audit event whose target names the fields sent. |
| `POST /v1/user/attach-grants` | Requires a user bearer; checks `session.read` or `session.input` at the registered Operation before issuing an attach token. |
| `POST /v1/user/operations/{operation_id}/missions/{mission_id}/actions` | Requires `mission.control` (`session.input` is not enough). Body `{"action", "agent"?, "message"?, "session_id"?, "idempotency_key"}`, unknown fields refused; `agent` only with `retry`, `message` (1 byte to 16 KiB, not blank) only with `resume`. Audits, queues the control (see Operator controls) and returns `202` with a `ControlStatus`; a repeated key returns `200`. A `cancel` of a Mission whose brief is still queued or held withdraws it (as `/withdraw`, no note) and returns `202` with the control already `applied`, state `failed`, `station_id` empty; a `cancel` of a claimed brief also records when it was first asked (`ai6::BriefStore::request_cancel`), from which the Headquarters Station settles it after `[registry.limits.briefs] cancel_settle_after`. |
| `POST /v1/user/operations/{operation_id}/missions/{mission_id}/withdraw` | Requires `mission.control`; body `{"note"?}` (at most `[text] held_reason_chars`, no control characters but newlines). Files the queued or held brief as failed (`ai6::VettingStore::withdraw_brief`) and returns `WithdrawnBrief { operation_id, mission_id, state: "withdrawn", reason }`, the reason "withdrawn by PERSON[: NOTE]"; again returns the same. `404` when the Operation has no such brief queued or held, `409` (naming `mission cancel`) once a Station took it or it finished. Audited `mission.withdraw` (target: the key), timeline `intake` `withdrawn`. |
| `GET /v1/user/operations/{operation_id}/missions/{mission_id}/actions/{control_id}` | Requires `mission.read`; returns the control's `ControlStatus`, `404` for an unknown control or one of another Mission. |
| `POST /v1/user/operations/{operation_id}/missions` | Body may carry `review: {plan?, output?}` (policy names, `none` included), which needs `mission.review` too; requires `mission.create` at the target Operation and atomically enqueues a bounded explicit brief in the shared Registry; returns the queued Mission ID. The body may carry `stations` to limit the Mission to those Stations (it can only narrow the Operation's list); one that names no Station that ever checked in, or one the Operation's published policy leaves out, is refused `400` with a plain-text reason naming it, an unknown pin with the Station IDs it equals ignoring case and those of Stations whose `host` it names. A `role` that `ai6::is_head_of_station_role` accepts is `400`: only an authorized Sleeper queues a Head of Station round. `issue: {tracker, item}` follows up an item that has a Mission: the handle is that Mission's current handle and the item's next follow-up number from 2 (`G-gh-31-2`), the brief's `data.follow_up` names the item, and the Mission is linked to it (`related`, by `person:<subject>`); an item with no Mission is `400` with the reason. `create_issue: {tracker, title?, body?}` (with `issue` it is `400`) also needs `issue.create`: it stores an approved issue proposal whose one issue carries the brief (`ProposedIssue::mission`), audits `issue.create.queued` and `mission.create.requested` (target `issue-proposal ID`), and answers `{operation_id, proposal, state: "awaiting_issue"}` with no `mission_id` or `handle`; the Headquarters Station enters the Mission once the issue exists. A tracker the Operation's roster does not name is `400`. |
| `GET /v1/auth/session` | Validates a user bearer and reports its token ID, person ID and expiry. |
| `POST /v1/station/sessions` | Bearer-authenticated registration request with `operation_id`, `mission_id`, `session_id`, and optional sanitized Mission brief metadata. |
| `POST /v1/station/sessions/{session_id}/lifecycle/{sequence}` | Bearer-authenticated JSON lifecycle update with Station, Mission ID and state. |
| `POST /v1/station/sessions/{session_id}/terminal/{sequence}` | Bearer-authenticated raw terminal output bytes. |
| `POST /v1/station/sessions/{session_id}/events/{sequence}` | Bearer-authenticated binary guatiao frame. |
| `POST /v1/station/attach-grants` | Bearer-authenticated grant request; returns token in JSON response body. |
| `GET /v1/station/inputs/next?wait_ms=<n>` | Bearer-authenticated long poll, held as a control poll is (`wait_ms` default 1000, capped at 25000; `204` when nothing came); returns one binary Guatiao terminal input or resize frame with `x-ai6-operation-id`, `x-ai6-mission-id` and `x-ai6-session-id` headers. AI6 validates the frame and dispatches it to that live Session's terminal. |
| `GET /v1/station/controls/next?station=<id>&wait_ms=<n>` | Bearer-authenticated long poll for that Station's next control (`wait_ms` default 1000, capped at 25000); `200` with one `application/vnd.ai6.guatiao` control value frame on channel 4 (the receiver builds the schema from any control) and the control becomes `delivered`, or `204`. Stations share one credential, so `station` is the caller's own word. |
| `POST /v1/station/controls/{control_id}/result` | Bearer-authenticated outcome `{"station","outcome":"applied"\|"rejected","state"?,"reason"?}`; `state` is a Mission state name, `reason` is cut to 512 characters without control characters. `204`; `404` unknown, `403` another Station's control, `409` already finished. |
| `GET /v1/user/moneypenny` | WebSocket, text frames only. First frame within 10 s: `{"type":"hello","token":ACCESS_TOKEN,"conversation"?:ID}` (the token in the socket, since a browser cannot set a header); a bad token gets `{"type":"error","code":"unauthorized","message"}` and close 1008. Answers `{"type":"ready","conversation","entries":[{role,text,at}],"earlier","busy","view_limits":{…}}` (the newest 100 entries within 512 KiB; `earlier` counts those left out); a conversation of another person is never opened (a new one is). Then `{"type":"message","text":1..=16 KiB,"route"?:ROUTE,"images"?:[{"media_type","data"}]}` (an app route in its JSON form, `{"page":"missions","operation_id":"G",…}`; images base64, PNG, JPEG, WebP or GIF by their bytes, at most `[assistant.limits] images` of `image_bytes` each, kept per person by SHA-256 in `{pages dir}/.images`; the person's entry carries `data.images: [{sha256, media_type, bytes}]` and the job `images`; the socket takes a frame of `socket_message_bytes` plus that many images) (`error` `busy` while one is answered, `unauthorized` once the token ended) `{"type":"new"}`, `{"type":"confirm","proposal":ID}`, `{"type":"cancel","proposal":ID}`, `{"type":"submit","view":ID,"values":{…}}` and `{"type":"interrupt"}` (answered `{"type":"interrupt","stopping":bool}`; see Moneypenny, the assistant); the gateway sends `{"type":"entry","entry":{"role":"person"\|"moneypenny"\|"tools"\|"error"\|"proposal"\|"view"\|"navigation"\|"notice","text","at","data"?}}`, `{"type":"proposal","proposal":{…}}` and `{"type":"busy","busy"}`. `503` without `with_assistant`. |
| `GET /v1/user/moneypenny/images/{sha256}` | User bearer; an image the person sent her, in its own type with `nosniff`, `no-store` and a sandbox CSP; `404` for anyone else's, an administrator included. |
| `GET /v1/station/assistant/images/{sha256}?station=&conversation=` | Station token; an image the person of a conversation the Station runs sent; `403` for a conversation another Station runs, `404` for an image not kept for that person. |
| `POST /v1/moneypenny/mcp` | Moneypenny's tools (see Moneypenny, the assistant); bearer: a job's MCP token only. |
| `GET /v1/user/assistant/memory[?person=ID][&q=TEXT]` | User bearer. `{"person","entries","total"}`, with no content: newest change first, or with `q` best first and at most `search_results`. Another person's needs `assistant.memory.admin` (`403`), audited `memory.read`. |
| `GET /v1/user/assistant/conversations[?person=ID\|?everyone=true]` | User bearer. `{"conversations":[{conversation,person,first_at,last_at,entries,bytes}]}` from her conversation log, newest first: the person's own, or another's or everyone's with `audit.read` and `session.context` on `config:assistant` (`403`), audited `assistant.log.list`. |
| `GET /v1/user/assistant/conversations/{id}/log` | User bearer. `{"conversation","person","entries"}`, each an `ai6::AssistantLogEntry` (with its `turn`). Another person's needs both actions above, audited `assistant.log.read` (resource `config:assistant`, target `<person> <id>`) before it is read; without them `404`, as for one not logged. |
| `GET /v1/user/assistant/conversations/{id}/context` | User bearer. `{"conversation","person","contexts"}`, each as the Session context route gives it; another person's as for `/log`, audited `assistant.context.read`. |
| `GET /v1/user/operations/{operation_id}/sessions/{session_id}/context` | User bearer and `session.context` on the Operation (`403`). `{session,owner,station,handler,format,source,filed_at,cut_from?,bytes,content}`; audited `session.context.read` (resource `mission:<uid>`, target the Session); `404` for a Session of another Operation or with no context. |
| `POST /v1/user/assistant/memory` | User bearer. Body `{"name"?, "summary", "content"}`; `201` with the entry's header. `400` out of bounds, `409` when the memory is full. |
| `GET /v1/user/assistant/memory/{id}[?person=ID]` | User bearer. The entry with `content`; `404` for an ID that is not this person's, the same whoever's it is. |
| `PUT /v1/user/assistant/memory/{id}` | User bearer, own memory only. Body `{"name"?, "summary", "content", "revision"}`; the header. `409` with `revision` when it has moved on. |
| `DELETE /v1/user/assistant/memory/{id}[?revision=N]` | User bearer, own memory only. Forgets it and its prior versions; `{"id"}`. |
| `GET /v1/user/assistant/memory/{id}/versions[?person=ID]` | User bearer. `{"versions"}`, what it held before, newest first. |
| `GET /v1/user/memories[?scope=SCOPE][&q=TEXT]` | User bearer. One scope newest first, or a search best first across one scope or every one the person may read: `{"entries","total"}`, no content; what they may not see is neither listed nor counted. |
| `POST /v1/user/memories` | User bearer. `{"scope","name"?,"summary","content","compartments"?}`; `201` with the header, `202` with the proposal when the caller may only propose, `403` without the scope's write or a read-in of a compartment it adds. |
| `GET`, `PUT`, `DELETE /v1/user/memories/{id}` | User bearer. As the person routes, on any scope; `PUT` takes `compartments`, left out to keep them. `404` for what the person may not see, the same as a missing ID. |
| `GET /v1/user/memories/{id}/versions` | User bearer. `{"versions"}`, newest first. |
| `GET /v1/user/memories/proposals[?scope=SCOPE]` | User bearer. `{"proposals"}` the person may decide, oldest first. |
| `POST /v1/user/memories/proposals/{id}` | User bearer. `{"approve"}`; `{"approved","entry"?}`. `404` for one they may not see, `409` when its entry moved on. |
| `GET /v1/station/assistant/next?station=<id>&wait_ms=<n>` | Station bearer; `200` with the Station's next `ai6::AssistantJob` or `204` after `wait_ms` (default 1,000, at most `[assistant.gateway] max_wait`), held the same with Moneypenny off. |
| `POST /v1/station/assistant/reports` | Station bearer; body `ai6::AssistantReport` (at most 64 events). `204`; `404` unknown conversation, `409` a job no longer waited for, `403` a Station that does not run the conversation. `text` events become `moneypenny` entries, `tool` events `tools` entries, `completed` ends the answer (its text shown only when no text came before), `interrupted` ends it with a `notice` entry, `failed`, `limit` and a `closed` while answering become `error` entries; `cable_proposal` and `cable_sent` as under Cables in Moneypenny, the assistant. |
| `GET /v1/station/assistant/cables?station=ID&conversation=ID&operation=ID` | Station bearer. `{"send", "read"}` for a conversation the Station runs: whether its person holds `cable.send` and `cable.read` there now (both false once the person's sign-in ended); `404` unknown conversation, `403` a Station that does not run it. |
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

- `universal-exports [--config <PATH>] [--provider <LOC>] [--registry <LOC>] [--listen <ADDR>]
  [--public-origin <HTTPS_ORIGIN>] [--plugin-path <PATH>]... [--plugin-profile P]` starts the
  loopback gateway from `ai6.cfg` (below) with these flags over it. Its `[unex] station_token` is
  required and is never accepted as an argument. It opens the configuration provider and
  reads its `unex` record (`ai6::UnexRecord`: `authenticator`, `directory`, `authorizer`, their
  `*_config` JSON objects, `proxy_user_header`, `proxy_lookup_by` (default `name`),
  `login_tries_per_address`, `login_tries_per_name`, `client_address_header`,
  `access_cache_ttl`, `access_cache_negative_ttl`, `access_cache_entries`, `assistant`
  (default true), `assistant_agent` (default `moneypenny`) and `route_text_bytes` (default
128, `AssistantConfig::with_route_text_bytes`)) and its `access` record, the
  role policy (`Settings::access_policy`; none denies every action until it is seeded). User access needs
  `directory` and either `authenticator` or `proxy_user_header`; `access` and `authorizer`
  are exclusive. Provider configurations are limited to 64 nested values and validated by
  Guatiao; an inline credential in one is printed as a weakening. Provider libraries are
  checked against the provider's `plugin` records under `[unex.plugins] profile`. The Directory and
  a plugin Authorizer are loaded behind coqui-access-kit's cache; a lifetime past the default
  is printed as a warning. The gateway polls the provider every `[provider] poll` seconds and
  swaps in a changed `access` or `operation_access` record (a refused one keeps the last good policy); a changed
  `unex` or `plugin` record is logged as needing a restart.

### Access administration (`access_admin.rs`, `accounts.rs`)

`LiveAccess` (`new(provider, host_chiefs, managed_by)`, `apply(&snapshot)`, `reload()`,
`seed(&snapshot, &registry)`, `observe_lineage`, `effective()`, `invalid()`) holds the compiled
policy: the `access` and `operation_access` records over the tree the Stations last published
(`Gateway::operation_tree` hands each tree to it, so Operation policies are recompiled when the
tree moves). It is the `Authorizer` of `AccessProviders`; with a plugin `Authorizer`
(`EitherAuthorizer`) the chief tier still applies and every write route answers `409 access is
managed by authorizer ID`. `Gateway::with_access_admin(live)` and `with_accounts(Arc<dyn
Accounts>)` serve the routes. Seeding: at start with user access and no `access` record, the
built-in roles are written create-only and audited `access.seeded`; a read-only provider
(`FileProvider`) keeps them in memory, a record that exists is never seeded again.
`[unex.access] chief` subjects are always chief and cannot be removed through the API;
with none anywhere the start logs that nobody can change access.

Every write takes the revision read, `If-Match: "<revision>"` or `?revision=` (`428` without,
`409` when stale or when the provider is read-only), checks the guard (`400 {"error": reason}`),
stores with `ConfigProvider::put_if` (compare-and-swap in SQL) and audits before answering.
A revision is `0` for no stored record, else a digest of the stored record.

| Route | Needs | Answers |
| --- | --- | --- |
| `GET /v1/user/access` | `access.read` at `config:access` | `revision`, `writable`, `managed_by?`, `roles` (`chief` first, `built_in`, `editable`), `rules` (`editor`, `changed_unix`), `delegable { actions, default, weakenings }`, `chief { stored, host }`, `invalid?` |
| `PUT`, `DELETE /v1/user/access/roles/{role}` | `access.manage` at `config:access` | `{revision}`; a role a rule names cannot be deleted (`409` naming the rules); body `{actions, description?}` |
| `PUT /v1/user/access/rules` | same | whole ordered list `{rules: [{subject, scope, role, propagate?}]}` |
| `PUT /v1/user/access/delegable` | same | `{actions: [..] or null}` |
| `PUT /v1/user/access/chief` | same, and a chief | `{chief: [..]}` |
| `GET /v1/user/access/check?subject=&action=&resource=` | `access.read` at the resource (`config:access` if not an Operation) | `{allowed, tier, rule_index, role}` |
| `GET /v1/user/operations/{op}/access` | `access.read` at the Operation | its record, `inherited` roles from readable ancestors, `orphaned`, `bindings` that reach it (a hidden ancestor is not named), `delegable` |
| `PUT`, `DELETE .../operations/{op}/access/roles/{role}`, `PUT .../access/rules` | `access.manage` at the Operation | `{revision}` |
| `GET /v1/user/accounts` | `account.manage` at `config:accounts` | accounts, never a secret |
| `PUT /v1/user/accounts/{name}` | same | `{groups?, disabled?, expires_unix?}`; makes or changes; a new account's password is in `secret` once; a `groups` change also needs `access.manage` (`400`); an account that is, or would become, chief needs a chief (`400`) |
| `POST /v1/user/accounts/{name}/secret` | same | a new password in `secret`, once, in no audit record or log |

Audit: `access.update`, `access.remove`, `access.refused`, `access.seeded`, `account.update`,
`account.secret`, `account.refused` (`operation` set for an Operation record; never a secret).
The profile gains `global_actions` (`access.read`, `access.manage`, `account.manage`) and each
Operation's actions gain `access.read` and `access.manage`. The accounts provider is the `unex`
record's `accounts` (`accounts_config`), else the authenticator when it offers accounts;
without one the routes answer `409 no account provider`. Moneypenny reads with the `access`
tool and proposes with `propose_access_change` and `propose_account_change`; a reset password
goes to the app as a `reveal` event on the conversation, never into her transcript.

## Compartments for people

A person holds an action where the access policy allows it or a compartment read into them grants
it; only `mission.status`, `mission.read`, `product.read`, `cable.send` and `cable.read` can come
from a compartment, on `operation:` resources and a board Mission. Each request reads what is
granted from the configuration provider once; a record that cannot be read grants nothing, and a
provider that cannot be read grants nothing. A read allowed only by a compartment is written as
`compartment.read` (`<compartment> <action>`, once per person, compartment, resource and action
every `[gateway] compartment_audit` seconds, default 60) before it returns, and a failed write
refuses it. The profile asks and writes nothing. A compartment grant does not count towards what an
editor may hand out: the guards ask `EffectiveAccess::holds`, which is the access tiers alone.

A refusal is `400`, `409` or `503` with `{"error": reason}`, the provider's own words.

| Route | Needs | Answers |
| --- | --- | --- |
| `GET /v1/user/compartments`, `GET .../{id}` | `compartment.read` at `compartment:<id>` | `id`, `description`, `actions`, `min_clearance?`, `limits`, `weakenings`, `covers` and `read_in` (`{entry, propagate}`), `revision`, `writable`; what the person cannot see with `mission.status` or `mission.read` is neither named nor counted |
| `GET /v1/user/compartment-form` | signed in | the settings form (`ai6::RecordForm`) |
| `PUT /v1/user/compartments/{id}` | `compartment.manage` there | settings only; covers and read-ins stay, so what the editor cannot see survives; audited `compartment.put` |
| `DELETE /v1/user/compartments/{id}` | same | `compartment.delete` |
| `PUT .../{id}/covers/{resource}` | `compartment.manage` there and `compartment.cover` on the Operation | `operation:<id>` or `mission:<id>`, body `{propagate?}`; `compartment.cover` |
| `DELETE .../{id}/covers/{resource}` | either | `compartment.uncover`: an Operation may always withdraw |
| `PUT .../{id}/read-in/{subject}` | `compartment.manage` there; a person, group or account also `access.manage` there | body `{propagate?}`; `compartment.read_in` |
| `DELETE .../{id}/read-in/{subject}` | `compartment.manage` there | `compartment.read_out` |
| `GET /v1/missions/{id}/products` | `product.read` or `mission.read` by the policy, or `product.read` through a compartment | status, `outcome_untrusted`, `cx_report_untrusted`, `refs`; anything else is `404` |
| `GET /v1/missions?compartment=K` | reads or is read into K | only Missions the person may read that K covers |

The profile's Operations gain `compartments`, the IDs that give the person something there, and
the actions `mission.status`, `product.read` and `compartment.cover`; `global_actions` gains
`compartment.manage` for a rule at `*`. Moneypenny's `compartments` tool reads the list, and her
tools and Cables follow the person's widened grants with no change of their own: no `compartment:`
audience, and her Cables stay per Operation.

`universal-exports` stops gracefully at Ctrl-C and, on unix, at SIGTERM, and returns `0` after
graceful shutdown, `1` when configuration or listener startup fails, and `2` for invalid
arguments. The operator command-line client is
the separate `ai6ctl` crate.

## Liaison (`universal_exports::liaison`)

A2A 1.0 at AI6's boundary, as the wire format of Cables: a peer's message is a Cable through
`ai6`'s `CableRelay` (sender `a2a:<peer>`, acting for its principal, `with_peer` stamped), the
addressee's reply Cable is the answer, and a Cable to `a2a:<peer>` leaves as A2A. Nothing is kept
but Cables, and nothing becomes a Mission or brief. A Task ID is the peer's Cable ID, a context ID
the Cable conversation; a context's replies come back to the Session `liaison-<context>`.

```rust
pub const ADDRESSEE_MEMBER: &str = "addressee";            // message or request metadata naming the addressee
pub use ai6::directorates::registry::cables::{LIAISON_PREFIX, LIAISON_STATION};  // "a2a:", "liaison"
pub enum Addressee { Agent { operation, agent }, Role { operation, role } }  // FromStr/Display: agent:<op>/<agent>, role:<op>/<role>
pub fn Addressee::operation(&self) -> &str
pub struct LiaisonPeer { pub name, pub id: Option<String>, pub principal, pub addressees: Vec<Addressee>, pub a2a: bool, pub url: Option<url::Url> }
pub fn LiaisonPeer::sender(&self) -> String                 // a2a:<name>
pub struct LiaisonConfig { pub peers: BTreeMap<String, LiaisonPeer>, pub poll: Duration, pub max_links: ai6::Limit,
    pub plain_beyond_loopback: bool, pub recheck: Duration }   // Default: no peers, 1 s, 64, false, 1 s
pub fn LiaisonConfig::with_peer(self, peer: LiaisonPeer) -> Self
pub fn LiaisonConfig::from_settings(&ai6::bootstrap::LiaisonSettings) -> Result<Self, LiaisonError>
pub struct Liaison;                                          // Clone; impl coqui_a2a::Agent
pub fn Liaison::new(gateway: Gateway, config: LiaisonConfig) -> Liaison
pub fn Gateway::with_liaison(self, config: LiaisonConfig) -> Gateway   // also serves POST /v1/a2a
pub fn Gateway::liaison(&self) -> Option<Liaison>
pub struct LinkListener;                                     // Drop stops it and its outbound thread
pub fn LinkListener::start(liaison: &Liaison, address: SocketAddr, tls: Option<Arc<rustls::ServerConfig>>,
    identity: &Path) -> Result<LinkListener, LiaisonError>   // key made the first time (Noise)
pub fn LinkListener::local_addr(&self) -> SocketAddr
pub fn LinkListener::peer_id(&self) -> &str                 // sha256:<hex>, what a peer pins
pub struct LiaisonError;                                     // Display, Error
```

- A link peer is admitted only by proving a configured `id`; `a2a = false` refuses its `a2a/1` streams. A `ws://`
  listener binds beyond loopback only with `plain_beyond_loopback`.
- `POST /v1/a2a` (and `/v1/a2a/`) is A2A's JSON-RPC binding, Server-Sent Events for the streaming methods;
  `GET /v1/a2a/.well-known/agent-card.json` its card, interface `{public_origin}/v1/a2a`, `JSONRPC`, `1.0`. Both need
  the bearer of a person who is a peer's principal: 401 without one, 403 for anyone else, 404 without Liaison.
- Each message needs the principal's `cable.send` on the addressee's Operation, each read its `cable.read`; the
  relay's grants name `a2a:<peer>`. Refusals are `PermissionDeniedError`, another peer's Task `TaskNotFoundError`.
- A follow-up in a context carries `with_affinity` of the Session that took the context's first Cable.
- `SendMessage` blocks until a reply (answered with the reply `Message`) or the Task ends, unless
  `returnImmediately`; `CancelTask` withdraws a Cable nobody has had.
- Outbound: Cables to `a2a:<peer>[/<addressee>]` are read from each Operation where the principal holds `cable.read`,
  sent over the peer's own link or by dialing `url` (`wss://` with the public roots), and answered by a reply Cable.
- Audit: `liaison.send_message`, `get_task`, `list_tasks`, `cancel_task`, `subscribe_to_task`, `outbound`, actor the
  principal, metadata only; the relay's `cable.send` record for each Cable.

## Bootstrap file

The command reads the one bootstrap file every program reads, `ai6.cfg`: `--config PATH`, else
the file `AI6_CONFIG` names, else `ai6.cfg` in the user's then the system's configuration directory
(`ai6::bootstrap::Bootstrap`). Unknown keys are refused; relative paths are relative to the file; a
secret is `{ provider = "file", key = "PATH" }`, `{ provider = "env", key = "NAME" }` or
`{ provider = "inline", value = "…" }` (the built-in secret providers, the only ones this file can
name), and the last two are printed as a warning at start.

| Key | Effect | Default |
| --- | --- | --- |
| `[unex] listen` | Loopback address behind the HTTPS reverse proxy (`127.0.0.1` only); `--listen`. | `127.0.0.1:7082` |
| `[unex] public_origin` | Required browser HTTPS origin; `--public-origin`. | None |
| `[unex] station_token` | Required secret: the Stations' channel bearer credential. | None |
| `[unex] proxy_secret` | Secret the reverse proxy sends in `X-AI6-Proxy-Secret`; required with a proxy identity header, at least 32 characters. | None |
| `[unex] state_dir` | Where verified provider libraries, and a default Registry, are kept. | The platform's state directory |
| `[provider] location`, `password`, `poll` | The configuration provider Stations read; its roster is served under `/v1/user/roster` (a directory read-only, SQL editable); `--provider`. | The file's directory |
| `[registry] location`, `password`, `headquarters_lease` | Shared Registry for issued access-token records; `--registry`. | The provider's database when SQL, else `registry` in `[unex] state_dir` |
| `[unex.plugins] paths`, `profile`, `keep_verified` | Provider libraries, how they are checked, and which old verified copies stay; `--plugin-path` adds, `--plugin-profile` overrides. | None; strict with a pin set, report without; the newest 8 |
| `[unex.pages] dir`, `origin`, `listen` | Where pages are kept, their own origin (a second listener with page routes only), and its loopback address. | `pages` beside the file; the API's origin; `127.0.0.1:7083` |
| `[unex.liaison] listen`, `identity`, `poll`, `max_links`, `plain_beyond_loopback` | Liaison's link listener (TLS with `[unex.tls]`), its key, how often it looks for replies and outgoing Cables, links at once, and a plain listener beyond loopback (a weakening). | Off; `liaison/identity` in `[unex] state_dir`; 1 s; 64; false |
| `[unex.liaison.peer.<name>] id`, `principal`, `addressees`, `a2a`, `url` | One peer: its link key, the subject it acts as, what it may write to, whether its link may open A2A, where to dial it. Any peer turns on `/v1/a2a`. | None; `a2a` true |

## Environment variables

The library reads no environment variables. The command reads `AI6_CONFIG` (the bootstrap
file; must exist when set) and the variables its secret references name. `ai6ctl` reads
`UNEX_URL` and `UNEX_ACCESS_TOKEN` (its own header).

`--help` lists the flag-backed variables and names `ai6.cfg` and its `station_token`.
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
