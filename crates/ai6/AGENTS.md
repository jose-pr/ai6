# `ai6` — API header

Header-file-style reference for the `ai6` crate: the public library and daemon command,
with signatures, contracts and gotchas. It ships inside the package and is self-contained.
Development documentation lives with the source at <https://github.com/jose-pr/ai6>.

| Feature | Adds | Default |
| --- | --- | --- |
| (none) | Public AI6 contracts without daemon dependencies. | Yes |
| `daemon` | The Station's HTTP service, scheduler and Guatiao provider host, and the field runner. | No |
| `publisher` | Bounded lifecycle and raw terminal-byte publication to Universal Exports. | No |
| `cables-mcp` | Per-Session authenticated MCP tools for Agent-to-Agent Cables. | No |
| `gate` | The green ink gate's client of a Station (`section_00::gate`), for agent providers; part of `daemon`. | No |
| `station-tls` | Persistent Station certificate identities and peer/CA mutual TLS (`coqui-peer-tls`). | No |
| `sqlite` | `SqlRegistry` and `SqlConfigSource` in a SQLite file. | No |
| `postgres` | `SqlRegistry` and `SqlConfigSource` in PostgreSQL, shared by the Stations of an HA deployment. | No |
| `cli` | The `ai6` command with all of the above and Ctrl-C handling. | No |

MSRV: Rust 1.89. The crate depends on Guatiao, `coqui-access`, `coqui-roles`, `coqui-peer` (digest text for pins), serde, toml, fs2, sha2, lru and zeroize.
`daemon` adds Axum, Tokio, Guatiao provider loading, `ureq`, `subtle`, `futures-util` and `ring` (Ed25519 publisher signatures),
`publisher` adds `url`, and `station-tls` adds `coqui-peer-tls` and rustls.
`sqlite` adds `rusqlite` with SQLite compiled in (a C compiler at build time); `postgres`
adds `tokio-postgres`, Tokio, `tokio-rustls`, rustls (ring) and `rustls-native-certs`. `cli` adds clap and Tokio signal support. The crate builds as a
Rust library and contains no `unsafe` code. `cables-mcp` adds RMCP's Streamable HTTP
server, JSON Schema support, constant-time bearer checks and per-Session tokens; the
default feature set remains dependency-light.

The library root re-exports common contracts. Two public modules group the rest.
`directorates` holds the decisions: `directorates::m` owns access, identity, Missions, the
roster, where it comes from (`config`) and cover; `directorates::registry` owns the Registry
(intake), the traits a Registry store implements (`backend`) and Cables; `directorates::q`
owns loadouts, secret leases, terminal contracts and the safe-house contract; `directorates::section_00` owns the
AgentHandler contract; `directorates::medical` owns Session monitoring. `station` holds the
node that runs them: `station::server` (daemon service and `serve*`), `station::plugins`
(provider host), `station::safehouse` (opens a Mission's safe house through a provider),
`station::runner` (the field runner), `station::publisher` (`publisher` feature) and
`station::identity` (`station-tls` feature). The command exposes `ai6 serve`,
`ai6 runner serve|call`, `ai6 identity` and `ai6 peer waiting|accept`. `ai6 serve
--registry LOCATION` and `--config LOCATION` take a directory, `sqlite:PATH` or a
`postgres://` URL for the Registry and the roster (defaults: `AI6_REGISTRY`, else the state
directory; the configuration directory); `--headquarters-lease SECS` (default 60) sets
`StationConfig::with_headquarters_lease`; `--config-poll SECS` (default 5, 0 off) sets
`StationConfig::with_config_poll`.

## Access contracts (`ai6::directorates::m::access`)

The access kinds, their models and the first-match role authorizer belong to the shared
[`coqui-access`](https://github.com/Boriken-Dev/coqui-access) and `coqui-roles` crates
(their API headers describe every field and method) and are re-exported here and at the
`ai6` root: `Authenticator`, `Directory`, `Authorizer`, `Accounts`, `Attempt`, `Lookup`,
`Verdict`, `Person`, `AuthorizationRequest`, `AuthorizationDecision`, `AccountInfo`,
`AccountPut`, `PutOutcome`, `Waiting`, `Description`, `KIND_PREFIX`, `Role`, `Rule`,
`RolePolicy`, `RolePolicyError`, `RoleAuthorizer` and `RoleAuthorizerBuilder`. The Guatiao
kinds are `coqui_authenticator`, `coqui_directory`, `coqui_authorizer` and `coqui_accounts`,
so a provider built for them for any host loads here.
`Attempt::headers` is an ordered `Vec<(String, String)>`.

What AI6 adds:

```rust
pub const SCOPE_KINDS: [&str; 4] = ["station", "operation", "theater", "config"];
                              // "config:roster" is the roster every Station reads
pub const ACTIONS: &[&str];   // agent.configure, agent.read, approval.class,
                              // approval.decide, audit.read, cable.read, cable.send,
                              // mission.control, mission.create, mission.read,
                              // pool.manage, pool.read, roster.manage, roster.read,
                              // session.input, session.read, station.configure,
                              // station.read; cable.* on operation:<id> bound
                              // Moneypenny's Cables for the person
pub fn role_authorizer() -> RoleAuthorizerBuilder;   // builder limited to SCOPE_KINDS
pub fn role_authorizer_from_toml(source: &str) -> Result<RoleAuthorizer, AccessPolicyError>;
pub fn load_role_authorizer(path: impl AsRef<Path>) -> Result<RoleAuthorizer, AccessPolicyError>;
#[non_exhaustive] pub struct AccessPolicyError;   // Display, Error
```

`role_authorizer_from_toml` reads the `access.toml` format: `[role.<name>]` tables with
`actions`, then `[[rule]]` entries (`subject`, `scope`, `role`) tried in order. A subject is
`account:<name>`, `person:<id>` or `group:<name>`; a scope is `*` or one of `SCOPE_KINDS`
followed by `:<id>`. No matching rule is deny, and the first matching rule is decisive even
when its role lacks the action. The file is read up to 1 MiB. The Universal Exports gateway
loads and enforces this policy for user requests.

`station::plugins::Plugins` scans the plugin paths for libraries that declare an AI6 kind or
any kind beginning with `KIND_PREFIX`, and lists and constructs `Authenticator`,
`Directory`, `Authorizer` and `Accounts` providers by id (`authenticators()`,
`account_managers()`, `authenticator(id, config)`, `accounts(id, config)` and the
rest); the caller composes authentication, directory lookup and authorization. AI6
registers no access provider of its own. The providers are the loadable libraries of
`coqui-access`, built from <https://github.com/Boriken-Dev/coqui-access>:

| Id | Kinds | Config (a Guatiao value; every key optional) |
| --- | --- | --- |
| `coqui_access_local` | Authenticator, Directory, Accounts | `source`, `max_accounts`, `argon2` |
| `coqui_access_key` | Authenticator, Directory, Accounts | `source`, `prefix`, `key_hash`, `argon2`, `max_keys` |
| `coqui_access_peer` | Authenticator, Directory, Accounts | `source`, `waiting`, `max_peers`, `waiting_places`, `seen_every_secs` |
| `coqui_access_ldap` | Authenticator, Directory | `url`, `start_tls`, `ca_file`, `base`, `bind`, `bind_password`, `user_filter`, `name_attribute`, `group_attribute`, `group_filter`, `group_base`, `nested_groups`, `timeout_seconds` |
| `coqui_access_proxy` | Authenticator | `trusted`, `user_header`, `subject_header`, `subject_field` |
| `coqui_roles` | Authorizer | `scope_kinds`, `roles`, `rules` |

`source` is `{ "file": { "path": "<file>", "format": "toml" } }`; with none, records live in
memory. The host's own tests build the plugin libraries as dev-dependencies and load them from
beside the test binary; after removing or renaming a plugin, clean `target/`, because the host
loads every library it finds.

## Watchdog (`ai6::directorates::medical::watchdog`)

```rust
pub struct WatchdogConfig;
impl WatchdogConfig {
    pub fn with_stall_timeout(self, timeout: Duration) -> Self;
    pub fn with_loop_timeout(self, timeout: Duration) -> Self;
    pub fn with_loop_min_lines(self, count: usize) -> Self;
    pub fn with_blocked_after(self, timeout: Duration) -> Self;
    pub fn with_max_runtime(self, timeout: Option<Duration>) -> Self;
    pub fn with_history(self, count: usize) -> Self;
    pub fn with_normalize_digits(self, normalize: bool) -> Self;
}
pub struct WatchdogOverrides {
    pub stall_timeout: Option<Duration>,
    pub loop_timeout: Option<Duration>,
    pub loop_min_lines: Option<usize>,
    pub blocked_after: Option<Duration>,
    pub max_runtime: Option<Duration>,
    pub history: Option<usize>,
    pub normalize_digits: Option<bool>,
}
impl WatchdogOverrides {
    pub fn apply_to(&self, base: &WatchdogConfig) -> WatchdogConfig;
}
impl Watchdog {
    pub fn with_config(config: WatchdogConfig) -> Self;
    pub fn max_runtime(&self) -> Option<Duration>;
    pub fn is_max_runtime_exceeded(&self) -> bool;
    pub fn pause(&self);       // waiting on a person: never stalled, looping, blocked or past max runtime
    pub fn resume(&self);      // fresh deadlines; paused time does not count toward max runtime
    pub fn is_paused(&self) -> bool;
}
impl ServeConfig {
    pub fn with_watchdog(self, watchdog: WatchdogConfig) -> Self;
}
```

`WatchdogConfig::default()` uses a 300-second stall timeout and loop timeout, 50 repeated
lines, a 10-second prompt timeout, no maximum runtime, 4,096 least-recently-used line
digests and no digit normalization. Its `with_stall_timeout`, `with_loop_timeout`, `with_loop_min_lines`,
`with_blocked_after`, `with_max_runtime`, `with_history` and `with_normalize_digits` builders
set those limits. `Watchdog::with_config` starts a monitor from the configuration;
`ServeConfig::with_watchdog` applies it to daemon Sessions. Provider events and terminal
output reset the inactivity deadline; accepted operator messages also reset prompt and
repeated-line tracking. Terminal output is split on carriage returns and newlines, ANSI
sequences are removed before comparison, and digit runs can be normalized to `#`. Repeated
lines with no novel output for the configured loop timeout interrupt and close the handler
operation as stalled. A quiet prompt-shaped tail marks the Mission blocked after the
configured prompt timeout; later terminal output or an accepted operator message returns it
to Working. The optional maximum runtime is measured from Session start and is not reset by
activity; `is_max_runtime_exceeded()` and `max_runtime()` expose that deadline. The daemon
publishes lifecycle updates through Universal Exports when configured. Each `[agent.<id>.limits]`
table can override selected daemon defaults; numeric duration values are finite, non-negative
seconds, may be fractional and merge field by field through `extends` inheritance.

## Usage, budgets, exposure and quarantine (`ai6::directorates::medical`)

Medical beside the watchdog; the operator-facing description is the Medical guide in the
docs. Everything here is plain data and decisions; the Station applies them.

```rust
// usage — token and cost accounting, and budgets
pub struct UsageTotals { pub input: u64, pub output: u64, pub cache_read: u64,
                         pub cache_write: u64, pub cost_micro_usd: u64 }   // serde, default 0
impl UsageTotals { pub fn add(&mut self, other: &Self); pub fn tokens(&self) -> u64 /* input+output+cache_write */ }
pub struct UsageTracker;   // default(); .observe(&TokenUsage) -> UsageTotals (the addition)
pub struct MissionUsage { pub total: UsageTotals, pub sessions: BTreeMap<String, UsageTotals>,
                          pub daily_cost_micro_usd: BTreeMap<String /* UTC YYYY-MM-DD */, u64> }
pub struct Budget { pub session_tokens: Option<u64>, pub mission_tokens: Option<u64>,
                    pub mission_cost_micro_usd: Option<u64>,
                    pub operation_cost_micro_usd_per_day: Option<u64>,
                    pub warn_at_permille: Option<u16> /* default 800 */ }
impl Budget {
    pub fn preset(p: BudgetPreset) -> Self;           // Observe (no limits) | Strict
    pub fn is_limited(&self) -> bool;
    pub fn merge(&self, child: &Self) -> Self;        // child's set fields win
    pub fn check(&self, session: &UsageTotals, mission: &UsageTotals,
                 operation_day_cost: Option<u64>) -> Vec<BudgetBreach>;   // Warn or Limit per scope
}
pub struct BudgetDefinition;  // the TOML table; .resolve() -> Result<Budget, String> naming the key
pub fn micro_usd(dollars: f64) -> u64;  pub fn utc_day(unix_seconds: i64) -> String;

// exposure — which Sessions read untrusted input
pub struct Exposure { pub source: String /* brief:source | brief:registry_drop | cable:X | tool:X | review */, pub at: f64 }
pub enum Restriction { Proposals, Approvals, Cables, Secrets }
pub struct ExposurePolicy;   // default() = strict; ::preset(Strict | Observe); .restricts(r); .exposed_by_tool(name)
pub struct ExposureDefinition;   // [operation.ID.exposure]: preset, restrict = [..], tools = [..]
pub struct ExposureGate;     // clone-shared per Session: new(policy, inherited), mark(source, at) -> bool,
                             // exposures(), is_exposed(), restricts(r), take_changes()

// quarantine — when an Agent is stopped everywhere
pub enum IncidentKind { Stall, Loop, Blown, Budget }
pub struct Incident { pub kind: IncidentKind, pub at: f64, pub operation: Option<String>, pub mission: String }
pub struct QuarantinePolicy { pub window_secs: u64 /* 86400 */, pub stalls: u32 /* 3 */,
                              pub budget_stops: u32 /* 2 */, pub blown: bool /* true */ }
impl QuarantinePolicy { pub fn reason(&self, incidents: &[Incident], now: f64) -> Option<String> }
pub struct QuarantineDefinition;   // [operation.ID.quarantine]; 0 turns a count trigger off
```

`Operation::{budget, exposure, quarantine}` and `Agent::budget` carry the resolved values
(`[operation.ID.budget]`, `[agent.ID.budget]`, `[operation.ID.exposure]`,
`[operation.ID.quarantine]`). Unset budget keys are not enforced; `preset = "strict"` sets
2,000,000 session tokens, 5,000,000 Mission tokens, $10 per Mission and $50 per Operation
per UTC day. A budget value that is negative, non-numeric (or fractional for token counts)
fails the roster load with an error naming the key.

## Operations and Agent roster (`ai6::directorates::m::roster`)

```rust
#[non_exhaustive]
pub struct Agent {
    pub id: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub handler: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub roles: Vec<String>,
    pub tags: Vec<String>,
    pub clearance: String,
    pub priority: i64,
    pub context: String,
    pub env: BTreeMap<String, String>,
    pub limits: WatchdogOverrides,
    pub budget: Budget,                           // layered over the Operation's
    pub max_concurrent: Option<usize>,
    pub pool: Option<String>,                     // pins one account pool; inherited through `extends`
    pub retire_on: Option<String>,                // UTC date YYYY-MM-DD the model reaches end of life
    pub on_retire: OnRetire,                      // Desk (default) | Retired
    pub retire_warning_days: u32,                 // DEFAULT_RETIRE_WARNING_DAYS = 14; 0 = no warning
}
impl Agent {
    /// The defaults a file gives `[agent.ID]`: clearance "field", priority 0, nothing else.
    pub fn new(id: impl Into<String>) -> Self;
    /// Serving | Retiring (within the warning window) | DeskDuty | Retired, judged by UTC day.
    pub fn retirement(&self, now: i64) -> Retirement;
    pub fn is_retired(&self, now: i64) -> bool;   // Retired only; DeskDuty still works
    /// Configured clearance, or a live grant's: a raise (grant.approval set) replaces it, any
    /// other grant only lowers it; capped by the Operation's; "desk" once its date has come.
    pub fn effective_clearance(&self, operation: &Operation, grant: Option<&ClearanceGrant>,
                               now: i64) -> String;
}
#[non_exhaustive] pub enum OnRetire { Desk /* default */, Retired }      // snake_case; as_str()
#[non_exhaustive] pub enum Retirement { Serving, Retiring, DeskDuty, Retired } // as_str(): serving | retiring | desk_duty | retired
/// What select_agent looks for; Default is any Agent of the global roster.
#[non_exhaustive] #[derive(Clone, Copy, Default)] #[must_use]
pub struct AgentQuery<'a> { pub operation: Option<&'a str>, pub role: Option<&'a str>,
    pub tags: &'a [String], pub available_handlers: Option<&'a [String]>,
    pub excluded: &'a [String], pub now: Option<i64> /* None: retirement not judged */ }
impl<'a> AgentQuery<'a> { pub fn new() -> Self; pub fn operation(self, Option<&'a str>) -> Self;
    pub fn role(self, Option<&'a str>) -> Self; pub fn tags(self, &'a [String]) -> Self;
    pub fn handlers(self, Option<&'a [String]>) -> Self; pub fn excluding(self, &'a [String]) -> Self;
    pub fn at(self, now: i64) -> Self }
#[non_exhaustive]
pub struct AgentRole {
    pub id: String,
    pub purpose: String,
    pub policy: String,
}
impl AgentRole {
    pub fn new(id: impl Into<String>, purpose: impl Into<String>, policy: impl Into<String>) -> Self;
    pub fn validate(&self) -> Result<(), RosterError>;
}
#[non_exhaustive]
pub struct Operation {
    pub id: String,
    pub name: Option<String>,
    pub description: String,
    pub path: Option<PathBuf>,
    pub theater: Option<String>,
    pub config: Option<PathBuf>,
    pub roster: Vec<String>,
    pub clearance: String,
    pub max_concurrent: Option<usize>,
    pub workspace: Workspace,
    pub station_labels: BTreeMap<String, String>,
    pub stations: Vec<String>,                    // Stations allowed to run it; empty allows all
    pub cover: String,
    pub cover_identity: Option<CoverIdentity>,
    pub on_blown: String,
    pub on_limit: OnLimit,                        // Rotate (default), Wait or Backup; see Provider limits
    pub limit_fallback_secs: u64,                 // park time when a limit has no reset time; default 1800
    pub on_lost: OnLost,                          // Retry (default) or Fail; see Recovery
    pub placement: PlacementStrategy,             // Spread (default) or Pack; see Placement
    pub lost_after_secs: u64,                     // runner/wrapped harness out of reach before it is probed and lost; default 45; above 0
    pub budget: Budget,                           // no limit unless configured
    pub exposure: ExposurePolicy,                 // strict unless configured
    pub quarantine: QuarantinePolicy,             // 3 stalls/loops, 2 budget stops, 1 blown cover per 24 h
    pub green_ink: GreenInkPolicy,                // strict unless configured; see Green ink
    pub vetting: VettingPolicy,                   // every check on unless configured; see Vetting
    pub sources: toml::Table,                     // provider ID -> that provider's settings (or a target string)
    pub sleepers: BTreeMap<String, Sleeper>,      // standing orders; see Sleepers
    pub pools: Option<Vec<String>>,               // account pools it may use; None (default): every pool
    pub harness: HarnessPolicy,                   // [operation.ID.harness]; ignore unless configured
    pub promotion: PromotionPolicy,               // [operation.ID.promotion]; see Promotion below
}
impl Operation {
    /// The defaults a file gives `[operation.ID]`: clearance "field", cover "covert",
    /// on_blown "flag", strict exposure and green ink, `Workspace::default()`.
    pub fn new(id: impl Into<String>) -> Self;
    /// The checks the file loader runs after resolving an Operation.
    pub fn validate(&self) -> Result<(), RosterError>;
    /// `workspace.provider`, else `"git"` when the source or `path` holds a `.git`, else `None`.
    pub fn safe_house_provider(&self) -> Option<String>;
}
#[non_exhaustive]
pub struct Workspace {   // Default: the defaults below
    pub provider: Option<String>,
    pub source: Option<PathBuf>,   // defaults to the Operation's `path`
    pub base: String,              // default "HEAD"
    pub branch: String,            // template, default "{id}-{slug}"
    pub isolate: bool,             // default true
    pub checkpoint_remote: Option<String>, // Git remote checkpoints are pushed to; None (default) pushes nothing
    pub checkpoint_secs: u64,      // between checkpoints; default DEFAULT_CHECKPOINT_SECS (300); above 0
}
#[non_exhaustive] pub enum OnLost { Retry, Fail }   // serde "retry" | "fail"
pub const DEFAULT_CHECKPOINT_SECS: u64 = 300;
pub struct CoverIdentity { pub name: String, pub email: String }
#[non_exhaustive]
pub struct RosterError;   // message + kind(); Display, Error
impl RosterError { pub fn kind(&self) -> RosterErrorKind; }
#[non_exhaustive] #[derive(Clone, Copy, PartialEq, Eq)]
pub enum RosterErrorKind {
    Invalid,       // the configuration is not acceptable; a refused change changed nothing
    Unavailable,   // could not be read or written (I/O, database); retrying may succeed
    ReadOnly,      // the ConfigSource takes no put/remove
}
impl Roster {
    pub fn load(root: impl Into<PathBuf>) -> Result<Self, RosterError>;
    pub fn from_parts(
        operations: impl IntoIterator<Item = Operation>,
        agents: impl IntoIterator<Item = Agent>,
        roles: impl IntoIterator<Item = AgentRole>,
        covers: Covers,
    ) -> Result<Self, RosterError>;
    pub fn builder() -> RosterBuilder;
    pub fn operation(&self, key: &str) -> Result<&Operation, RosterError>;
    pub fn operations(&self) -> impl Iterator<Item = &Operation>;
    pub fn role(&self, key: &str) -> Result<&AgentRole, RosterError>;
    pub fn roles(&self) -> impl Iterator<Item = &AgentRole>;
    pub fn agent(&self, operation_key: Option<&str>, agent_key: &str)
        -> Result<Agent, RosterError>;
    pub fn agent_ids(&self) -> impl Iterator<Item = &str>;   // global codenames, in order
    pub fn select_agent(&self, query: &AgentQuery<'_>) -> Result<Agent, RosterError>;
    pub fn agents_for_operation(&self, operation_key: &str) -> Result<Vec<Agent>, RosterError>;
    /// Resolves every global Agent (extends, clearance, budget) now instead of on first use;
    /// an Operation's `.ai6/agents.toml` is not read.
    pub fn check_agents(&self) -> Result<(), RosterError>;
    pub fn assistant(&self) -> &AssistantSettings;  // `[assistant]`; the default without one
}
#[must_use] #[derive(Default)]
pub struct RosterBuilder;
impl RosterBuilder {
    pub fn operation(self, operation: Operation) -> Self;
    pub fn agent(self, agent: Agent) -> Self;
    pub fn role(self, role: AgentRole) -> Self;
    pub fn cover(self, cover: Cover) -> Self;      // replaces a built-in of the same name
    pub fn assistant(self, settings: AssistantSettings) -> Self;
    pub fn build(self) -> Result<Roster, RosterError>;   // Roster::from_parts; settings validated
}
pub fn config_dir() -> Result<PathBuf, RosterError>;

// ai6::directorates::m::assistant, re-exported at the crate root
#[derive(Default, Serialize, Deserialize)] // serde: unknown keys refused
pub struct AssistantSettings;   // keys: personal_repo, shared.<group> {repo, write}, org {repo, write}, allow_elevated
impl AssistantSettings {
    pub fn new() -> Self;                                  // keeps nothing, refuses elevated Stations
    pub fn with_personal_repo(self, template: impl Into<String>) -> Self; // {person} | {person_digest}
    pub fn with_shared(self, group: impl Into<String>, repo: SharedRepo) -> Self;
    pub fn with_org(self, repo: SharedRepo) -> Self;       // checked out as "org"
    pub fn with_allow_elevated(self, allow: bool) -> Self; // a weakening
    pub fn personal_repo(&self) -> Option<&str>;
    pub fn shared(&self) -> &BTreeMap<String, SharedRepo>;
    pub fn org(&self) -> Option<&SharedRepo>;
    pub fn allow_elevated(&self) -> bool;
    pub fn validate(&self) -> Result<(), String>;          // "assistant.<key>: …"
    pub fn personal_repo_for(&self, person: &str) -> Result<Option<String>, String>;
    pub fn shared_for(&self, groups: &[String]) -> impl Iterator<Item = (&str, &SharedRepo)>;
    pub fn shared_named(&self, name: &str, groups: &[String]) -> Option<&SharedRepo>;
    pub fn weakenings(&self) -> Vec<String>;               // allow_elevated, each write = true
}
pub struct SharedRepo;  // SharedRepo::new(repo).writable(bool); repo(), write()
#[non_exhaustive] pub enum RepoKind { Local, Remote }
impl RepoKind { pub fn of(repo: &str) -> Result<Self, String>; }
pub const PERSON_PLACEHOLDER: &str = "{person}";
pub const PERSON_DIGEST_PLACEHOLDER: &str = "{person_digest}";
pub const ORG_SHARED: &str = "org";
pub fn person_digest(person: &str) -> String;  // 16 hex digits of SHA-256
pub fn file_safe(name: &str) -> bool;          // 1-64 of letters, digits, . - _, not starting with .
```

`[assistant]` lives in the global `agents.toml` only (an Operation's `.ai6/agents.toml` with
one fails to resolve its Agents); a SQL roster keeps it as the one row of
`ai6_config_assistant`, keyed `assistant` (`ConfigRecord::Assistant`), and has the default
settings without it. `RepoKind::of` refuses empty values, control characters, Git
remote helpers (`::`) and schemes other than `file`, `ssh`, `git`, `http`, `https`; a local
path or `file://` is `Local` (made bare on first use), the rest and `user@host:path` are
`Remote` (must exist). `personal_repo_for` fills `{person_digest}` always and `{person}` only
for an identifier of 1-64 lowercase letters, digits, `.`, `_`, `@`, `-` not starting with `.`
(so identifiers differing only in case never share a repository); others are
refused, naming `{person_digest}`. `validate` refuses other placeholders, group names that are
not `file_safe` or are `org`, and repositories `RepoKind::of` refuses. `shared_for` yields the
groups' repositories the person is a member of, then `org`.

`load` reads `operations.toml` and `agents.toml`; missing files produce an empty roster and
invalid files fail closed. `from_parts` (and `builder().build()`) is the way in without
files: each Operation goes through `validate`, each role through `validate`, each Agent's
codename and clearance are checked at once, and a key given twice is refused. A code-built
Agent is complete as given (it inherits nothing); an Operation checkout's
`.ai6/agents.toml` may still extend or replace it. Operations are global: an Operation is not owned by a Station,
and its `station_labels` lists the Station labels a Station must carry to run its Missions.
`stations = ["id", ..]` optionally lists the Stations allowed to run them; a Mission's brief
may narrow that list but never widen it. `theater` binds the Operation to one Theater by ID.

An Agent is a codename; its `handler`, `model` and `effort` are the codename's current
occupant. An `AgentRole` (`[role.<id>]`) is a job description only: a `purpose` and a
`policy`, passed to the Session as initial context. Neither grants authorization. An
Agent's `roles` lists the roles it is qualified for; a qualified role name may carry a
suffix (`case-officer/implementation` resolves to `case-officer`).

`agent` accepts a codename or unambiguous alias and an optional Operation key. It reads
`.ai6/agents.toml` only for a registered Operation, merges inherited `env` and `limits` by
key, and rejects Agents outside the Operation's roster or above its clearance.
`select_agent` returns the highest-priority eligible Agent qualified for the role prefix
and carrying every tag. Agents whose handler is not among `available_handlers`, that are
`excluded` (the Station passes quarantined ones) or that are retired at `now` are skipped,
so the next qualified Agent serves as the backup; the error says no available Agent
matched, or, when only retired Agents qualified, `no qualified Agent for role "R"; 007
retired on 2027-01-31`. `retire_on` is a UTC date; from that day an Agent with
`on_retire = "desk"` works at `desk` and is still chosen, one with `"retired"` is never
chosen (its running Missions finish; the Station gives queued ones to the backup or keeps
them waiting). An invalid date fails resolving the Agent. `operations` yields stable codename order. `config_dir` resolves
`AI6_CONFIG_DIR`, then `XDG_CONFIG_HOME/ai6`, then the platform configuration directory.
`Agent.limits` is layered over the daemon-wide watchdog settings when a Mission starts or
resumes.

### Harness project settings (`ai6::directorates::m::harness`)

```rust
pub const PROJECT_DIRECTORIES: [&str; 2];      // ".claude", ".codex": every regular file, any depth
pub const PROJECT_FILES: [&str; 1];            // ".mcp.json"
pub const MAX_MANIFEST_FILE_BYTES: u64;        // 1 MiB
pub const MAX_MANIFEST_BYTES: u64;             // 16 MiB
pub const MAX_MANIFEST_FILES: usize;           // 4096
#[non_exhaustive] pub enum ProjectSettings { Ignore /* default */, Pinned, Load } // lowercase
#[non_exhaustive] pub struct HarnessPolicy {
    pub project_settings: ProjectSettings,
    pub pin: Option<coqui_peer::PeerId>,        // the manifest digest `pinned` needs
}
impl HarnessPolicy {
    pub fn pinned(pin: PeerId) -> Self;  pub fn with(project_settings: ProjectSettings) -> Self;
    pub fn validate(&self) -> Result<(), String>;    // pinned needs a pin
    pub fn weakenings(&self) -> Vec<String>;         // load; a SHA-1 pin
    pub fn decide(&self, root: &Path) -> ProjectDecision; // reads the manifest under pinned (blocking)
}
#[non_exhaustive] pub struct ProjectDecision { pub load: bool, pub mcp: bool,
    pub manifest: Option<PeerId>, pub refused: Option<String> }
#[non_exhaustive] pub struct ProjectManifest { pub files: BTreeMap<String, PeerId> } // path -> sha256
impl ProjectManifest {
    pub fn read(root: &Path) -> Result<Self, String>;
    pub fn digest(&self) -> PeerId;  pub fn digest_with(&self, algorithm: coqui_peer::Digest) -> PeerId;
}
```

The manifest is every regular file under `PROJECT_DIRECTORIES`, each of `PROJECT_FILES`, and
every file inside the working directory that a `command` or `args` value names (JSON and
TOML settings anywhere in it, `command:` lines of Markdown front matter;
`$CLAUDE_PROJECT_DIR` is the working directory). Its digest hashes `PATH NUL DIGEST LF`
lines in path order. It is read without following links (`O_NOFOLLOW | O_NONBLOCK` on unix,
`FILE_FLAG_OPEN_REPARSE_POINT` on Windows): a link, device, pipe or file past the limits is
an error, and the harness starts as `ignore`. The Station decides at each start and resume on
the blocking pool (`AgentStart::project_settings`, `project_mcp`), records the manifest
digest on the Operation's timeline (kind `harness`, `project-settings-pinned` or
`project-settings-ignored` with `reason`), and treats a harness on another host as `ignore`.
`decision.mcp` under `pinned` is whether the manifest holds `.mcp.json`. `ai6 harness pin DIR`
prints the manifest and its pin. Weakenings are printed at start.

## Configuration sources (`ai6::directorates::m::config`)

Where a Station's roster comes from. Theaters are not part of it: each Station reads its
own.

```rust
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConfigRevision;   // opaque; new(u64), get()
#[non_exhaustive]
pub struct ConfigSnapshot { pub revision: ConfigRevision, pub roster: Roster }
impl ConfigSnapshot { pub fn new(revision: ConfigRevision, roster: Roster) -> Self; }
pub trait ConfigSource: Send + Sync {
    fn load(&self) -> Result<ConfigSnapshot, RosterError>;
    /// Default: load and compare revisions.
    fn changes_since(&self, since: ConfigRevision) -> Result<Option<ConfigSnapshot>, RosterError>;
    /// Default: load. Every built-in source reads only its revision.
    fn revision(&self) -> Result<ConfigRevision, RosterError>;
    /// The records of one kind as JSON objects, for an administrator; default ReadOnly.
    fn definitions(&self, record: ConfigRecord) -> Result<ConfigDefinitions, RosterError>;
    /// Store / remove one record, checking the whole roster first; default ReadOnly.
    fn put(&self, record: ConfigRecord, key: &str, definition: &serde_json::Value)
        -> Result<ConfigRevision, RosterError>;
    fn remove(&self, record: ConfigRecord, key: &str)
        -> Result<Option<ConfigRevision>, RosterError>;   // None: there was none
}
#[non_exhaustive] #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConfigRecord { Operation, Agent, Role, Cover, Assistant }
impl ConfigRecord {
    pub const ALL: [Self; 5];
    pub const ASSISTANT_KEY: &'static str;     // "assistant": the one Assistant record
    pub const fn name(self) -> &'static str;   // "operation" | "agent" | "role" | "cover" | "assistant"
    pub fn from_name(name: &str) -> Option<Self>;
}
#[non_exhaustive] #[derive(Clone, PartialEq)]
pub struct ConfigDefinitions {
    pub revision: ConfigRevision,
    pub records: Vec<(String, serde_json::Value)>,   // by key
    pub writable: bool,                              // the source takes put/remove
}
/// Check-in key `config` of every Station (serde; empty fields are omitted).
#[non_exhaustive] #[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigStatus {
    pub revision: Option<String>,    // decimal: a digest can pass 2^53
    pub refused: Vec<String>,        // why the newest roster / loadouts.toml is refused
    pub needs_restart: Vec<String>,  // e.g. "theaters.toml", "cables.toml"
}
pub struct TomlConfigDir;   // ConfigSource (+ definitions, read-only); new(root), root()
pub struct MemoryConfigSource;   // ConfigSource (read-only); new(roster), replace(&self, roster)
/// `sqlite:PATH` / `sqlite://PATH` / `postgres://…` → SqlConfigSource; anything else →
/// TomlConfigDir. Unsupported for a SQL location without its feature.
pub fn open(location: &str) -> io::Result<Arc<dyn ConfigSource>>;
/// As `open`, a SQL roster connected with `options` (features `sqlite` or `postgres`).
pub fn open_with(location: &str, options: &SqlOptions) -> io::Result<Arc<dyn ConfigSource>>;

// Features `sqlite` or `postgres`; also at the crate root.
pub struct SqlConfigSource;   // ConfigSource with definitions/put/remove + Debug
impl SqlConfigSource {
    pub fn open(location: &str) -> io::Result<Self>;   // as SqlRegistry::open
    pub fn open_with(location: &str, options: &SqlOptions) -> io::Result<Self>;
}
```

A source hands out only checked rosters: an unacceptable configuration is an error
(`RosterErrorKind::Invalid`; a database or file that cannot be read is `Unavailable`). The
shape is coqui-access-kit's `Table` (load plus a cheap `changes_since`, optional put/remove)
for one value, the roster. `TomlConfigDir` is `Roster::load` over `operations.toml`,
`agents.toml` and `covers.toml` plus `Roster::check_agents`, so an Agent that cannot resolve
is refused as in SQL; its revision is a digest of those three files' contents, taken before
they are parsed, so `changes_since` reads them but parses nothing while they are unchanged;
`definitions` converts the files' tables to JSON (`Assistant` lists the global `[assistant]`
table as one record keyed `assistant`, none without it), and `put`/`remove` are `ReadOnly`.
`MemoryConfigSource` starts at revision 0 and moves to the next on each `replace`. Any
other store (a database, an HTTP API) builds its roster with `Roster::from_parts`. The
daemon reads the roster at start (`ServeConfig::with_config_source`; default
`TomlConfigDir` over the configuration directory) and polls `revision()` every
`DEFAULT_CONFIG_POLL` (5 s; `StationConfig::with_config_poll`, `ai6 serve --config-poll`,
0 off): a new revision is loaded and applied to requests, the scheduler and new Sessions
(running Sessions keep theirs); one that fails with `Invalid` is refused once per revision,
logged, audited (`config.refused`, actor and resource `station:<id>`) and reported at
check-in (`config.refused`) while the last good roster keeps running. `loadouts.toml` is read
again when its contents change (unless loadouts were set in code), with the same refusal
rule, and the Station then declares its pool accounts again, withdrawing its profile of
accounts it no longer declares. `theaters.toml` and `cables.toml` are read only at start: a
change is logged and listed in `config.needs_restart`. Command-line and `ServeConfig`
settings (listen address, Registry and roster locations, roles, labels, terminal, leases,
plugins, watchdog) also apply only at start.

`SqlConfigSource` keeps one table per record kind (`ai6_config_operations`, `_agents`,
`_roles`, `_covers`, `_assistant`): a row is a key and a JSON object with the keys of that
record's table in `operations.toml`, `agents.toml` or `covers.toml` (`deny_unknown_fields` as
there). `load` parses each with the file loader's types and checks, resolves Agents'
`extends` among the rows, adds the cover rows over the built-in covers and builds the roster
with `Roster::from_parts`; an Operation's trusted `.ai6/agents.toml` still applies on top.
The `assistant` row holds the keys of `[assistant]` and is checked by
`AssistantSettings::validate`; a row under any other key makes the roster invalid, so `put`
refuses one, and removing the row restores the default settings.
The revision is the `config` row of `ai6_clock`, read before the rows; `changes_since` reads
only it. `put` and `remove` change one record and check the whole roster in the same
transaction, refusing (and rolling back) a change that leaves it unacceptable, such as
removing an Agent another extends; each change advances the revision. A writer that edits
the tables directly must advance that row in the same transaction, or Stations do not see
the change. The database may be the Registry's.

## Theaters (`ai6::directorates::m::theaters`)

A Theater is where a Session works; a Station reaches it through a transport. Each Station
reads its own `theaters.toml` (`[theater.<id>]` with `access`, `labels`, `agents`, `egress`,
`harnesses`, `workdir`, `runner`, `requires` and `provision`; `docs/theaters.md` has the
table). The Station's own
host is always reachable as the implicit `local` Theater; declaring `[theater.local]` only
adds labels, facts or a runner.

```rust
pub const LOCAL_THEATER: &str; // "local"
#[non_exhaustive]
pub enum TheaterAccess { Local, Ssh(SshAccess), Transport(TransportAccess), Ephemeral(EphemeralAccess) }
impl TheaterAccess {
    /// The transport provider ID and its settings as TOML text (a `TransportConfig`);
    /// an error for an ephemeral template, which has none of its own.
    pub fn transport(&self) -> Result<(String, String), TheaterError>;
}
pub const DEFAULT_KEEP_FAILED: usize = 3;
pub const DEFAULT_KEEP_FAILED_HOURS: u64 = 24;
#[non_exhaustive]                            // TOML: access = { ephemeral = { … } }; not on `local`
pub struct EphemeralAccess { pub provider: String, pub config: serde_json::Map<String, serde_json::Value>,
    pub keep_failed: Option<usize>, pub keep_failed_hours: Option<u64> }
impl EphemeralAccess {
    pub fn new(provider: impl Into<String>) -> Self;
    pub fn with_setting(self, key: impl Into<String>, value: impl Into<serde_json::Value>) -> Self;
    pub fn keeping_failed(self, count: usize, hours: u64) -> Self;
    pub fn settings_toml(&self) -> Result<String, TheaterError>;   // a TheaterProviderConfig
}
#[non_exhaustive]
pub struct SshAccess {
    pub host: String, pub user: Option<String>, pub port: Option<u16>, pub identity: Option<String>,
    pub host_key: Option<String>,            // "sha256:HEX" of the host's public key
    pub trust_on_first_use: bool,            // weaker than a pin; reported
    pub connect_timeout_secs: Option<u64>,
}
impl SshAccess {
    pub fn new(host: impl Into<String>) -> Self;
    pub fn with_user(self, user: impl Into<String>) -> Self;
    pub fn with_port(self, port: u16) -> Self;
    pub fn with_identity(self, identity: impl Into<String>) -> Self;
    pub fn with_host_key(self, host_key: impl Into<String>) -> Self;
    pub fn trusting_on_first_use(self) -> Self;
    pub fn with_connect_timeout_secs(self, secs: u64) -> Self;
}
#[non_exhaustive]
pub struct TransportAccess { pub provider: String, pub config: serde_json::Map<String, serde_json::Value> }
impl TransportAccess {
    pub fn new(provider: impl Into<String>) -> Self;
    pub fn with_setting(self, key: impl Into<String>, value: impl Into<serde_json::Value>) -> Self;
}
#[non_exhaustive]
pub enum AgentMode { Auto, Local, RemoteHands }        // default Auto; as_str() -> "auto" | "local" | "remote-hands"
#[non_exhaustive]
pub enum Egress { Direct, Proxy(String), Station, Isolated }  // TOML: "direct", { proxy = URL }, "station", "none"
impl Egress { pub fn kind(&self) -> EgressKind; }
#[non_exhaustive]
pub enum EgressKind { Direct, Proxy, Station, Isolated }      // the URL-free kind that is published
#[non_exhaustive]
#[derive(Default)]                           // `runner = {}` in TOML: every default
pub struct RunnerSettings {                  // how to reach the `ai6 runner` on the Theater
    pub command: Option<String>,             // default: `ai6` on PATH; the Station's own binary on `local`
    pub state_dir: Option<String>,           // default: the runner's (`<AI6 state>/runner`)
    pub plugin_paths: Vec<String>,           // providers the runner loads, on the Theater
    pub terminal: Option<String>,            // default: the runner picks one that suits the handler (`pipe` for JSON, else `pty`)
}
impl RunnerSettings {
    pub fn with_command(self, command: impl Into<String>) -> Self;
    pub fn with_state_dir(self, state_dir: impl Into<String>) -> Self;
    pub fn with_plugin_path(self, path: impl Into<String>) -> Self;
    pub fn with_terminal(self, terminal: impl Into<String>) -> Self;
}
#[non_exhaustive]
pub struct Theater {
    pub id: String, pub access: TheaterAccess, pub labels: BTreeMap<String, String>,
    pub agents: AgentMode, pub egress: Egress, pub harnesses: Vec<String>, pub workdir: Option<String>,
    pub runner: Option<RunnerSettings>,
    pub requires: Option<Requirements>,      // what Sessions need there; unchecked when absent
    pub provision: Option<ProvisionSpec>,    // how `ai6 theater provision` installs it; never published
}
impl Theater {
    pub fn new(id: impl Into<String>, access: TheaterAccess) -> Self;
    pub fn with_label(self, key: impl Into<String>, value: impl Into<String>) -> Self;
    pub fn with_agents(self, agents: AgentMode) -> Self;
    pub fn with_egress(self, egress: Egress) -> Self;
    pub fn with_harness(self, harness: impl Into<String>) -> Self;
    pub fn with_workdir(self, workdir: impl Into<String>) -> Self;
    pub fn with_runner(self, runner: RunnerSettings) -> Self;
    pub fn with_requirements(self, requires: Requirements) -> Self;
    pub fn with_provision(self, provision: ProvisionSpec) -> Self;
    pub fn weakenings(&self) -> Vec<String>;    // settings looser than their defaults
    pub fn summary(&self) -> TheaterSummary;     // no capabilities: the Station adds them
}
#[non_exhaustive]
pub enum TheaterKind { Local, Ssh, Transport, Ephemeral }   // as_str() -> "local" | "ssh" | "transport" | "ephemeral"
#[non_exhaustive]
pub struct TheaterSummary {                  // what a Station publishes; no user, port, key path or proxy URL
    pub id: String, pub access: TheaterKind, pub host: Option<String>, pub provider: Option<String>,
    pub labels: BTreeMap<String, String>, pub capabilities: Option<Capabilities>,
    pub agents: AgentMode, pub egress: EgressKind, pub harnesses: Vec<String>,
    pub workdir: Option<String>,
    pub runner: bool,                        // a runner supervises harnesses there; omitted when false
    pub unavailable: Option<String>,
    pub weakened: Vec<String>,               // loose settings + "ambient credential on the Theater: …"
    pub requires: Option<Requirements>,
    pub report: Option<ProbeReport>,         // the Station's last probe; absent until one finishes
    pub made_for: Option<String>,            // the Mission an ephemeral Theater was made for
    pub resources: Option<Resources>,        // `local`: the Station's sample; else the newer of the
                                             // resources-only sample and the report's
    pub sessions: usize,                     // the Station's running Sessions there; omitted when 0
}
impl TheaterSummary { pub fn local() -> Self; }
pub fn is_sha256_pin(pin: &str) -> bool;    // "sha256:" and 64 hex digits
pub struct Theaters { /* declared Theaters */ }   // Default: none declared
impl Theaters {
    pub fn load(root: impl AsRef<Path>) -> Result<Self, TheaterError>; // missing file: none declared
    pub fn parse(text: &str) -> Result<Self, TheaterError>;
    pub fn insert(&mut self, theater: Theater) -> Result<(), TheaterError>;
    pub fn merge(&mut self, other: Theaters);                           // other wins on a shared ID
    pub fn reachable(&self) -> Vec<Theater>;                            // declared + implicit `local`
    pub fn find(&self, id: &str) -> Option<Theater>;
    pub fn summaries(&self) -> Vec<TheaterSummary>;                     // without capabilities
}
pub struct TheaterError(String);             // Display, Error
```

Theater IDs use letters, digits, `.`, `_` and `-`. An SSH host, user, a transport provider,
a harness and a runner `terminal` must be words that do not start with `-`; a port is 1
through 65535; a `host_key` is `sha256:` and 64 hex digits; runner paths and its command are
non-empty text, and unknown runner keys are refused. `ServeConfig` loads `theaters.toml` from the
configuration directory; `StationConfig::with_theater` adds one in code and wins on a shared
ID. A Session's launch record carries `station_id`, `theater_id` and `mode` separately.

A Station builds one transport per Theater at start (`TransportClient` over the provider the
Theater's `access` names; no host is contacted), publishes each Theater's `Capabilities` in
`TheaterSummary::capabilities`, and probes a remote Theater in the background until it answers
(`unavailable` carries the reason until then). On its own host the Station sets `terminal`
from its Terminal provider; a `local` Theater without a loaded `local` transport library
still hosts a harness (field mode) but offers no `run`. Weakened settings are logged at start.
Every five minutes it also runs `q::provision::probe` on each Theater whose transport can
`run` (not on a Windows host that declares no `requires`) and publishes the report: unmet
requirements, programs found, free space, and each ambient credential as a `weakened` line.
A harness the probe found and accepted is added to `harnesses`.

### Ephemeral Theaters (`ai6::directorates::q::ephemeral`)

A Guatiao kind (`ai6_theater_provider`; floor hash pinned in a test) for providers that make
a Theater per Mission. Calls block.

```rust
pub trait TheaterProvider: Send + Sync {
    fn capabilities(&self) -> Result<Value, ProviderError>;     // Capabilities of what it makes
    fn create(&self, request: &Value) -> Result<Value, ProviderError>; // TheaterRequest -> MadeTheater; same name = same Theater
    fn destroy(&self, name: &str) -> Result<(), ProviderError>; // one already gone is Ok
    fn list(&self, template: &str) -> Result<Value, ProviderError>; // MadeTheaters, oldest first
}
pub struct TheaterProviderConfig { pub settings_toml: String }    // the template's `config`
pub struct TheaterRequest { pub name: String, pub template: String, pub mission: String }
pub struct MadeTheater { pub name: String, pub template: String, pub mission: String,
    pub created_at: u64 /* default 0 */, pub transport: String, pub settings_toml: String }
pub struct MadeTheaters { pub theaters: Vec<MadeTheater> }
pub struct TheaterMaker;   // typed wrapper: new(Box<dyn TheaterProvider>), capabilities, create, destroy, list
```

`Plugins::theater_providers()` and `Plugins::theater_provider(id, config)` find and build
them; `ai6-theater-container` (ID `ephemeral-container`) is the container one. A Station
builds the provider of each `ephemeral` template at start. Starting a Mission whose
Operation is bound to a template has the provider make `<template>-<mission>` (characters
outside `[A-Za-z0-9._-]` become `-`; cut to 100 bytes with a digest), reached by the
transport `MadeTheater` names and inheriting the template's keys (`workdir` defaults to the
Operation's `path`); the Station runs `q::provision::provision` on it when the template has
`requires`, `provision` or `runner` (a failed step fails the start) and records a probe
report. The launch record names the made Theater. After every check-in the Station adopts
listed Theaters it does not know, destroys those whose Mission is `done` or `abandoned`, and
keeps those whose Mission `failed`, was `blown` or is unknown here, at most `keep_failed`
per template and for at most `keep_failed_hours`, newest first. A Theater with a Session
starting or running is never touched.

### Provisioning (`ai6::directorates::q::provision`)

```rust
pub const DEFAULT_INSTALL_TIMEOUT_SECS: u64 = 600;
pub struct RequiredProgram;                  // "claude>=2.1" or "git"; serde as that string; Display, FromStr
impl RequiredProgram {
    pub fn new(name: impl Into<String>) -> Result<Self, String>;           // 1-64 of [A-Za-z0-9._+-], not leading - or .
    pub fn at_least(self, minimum: impl Into<String>) -> Result<Self, String>; // digits and dots
    pub fn name(&self) -> &str; pub fn minimum(&self) -> Option<&str>;
    pub fn refuses(&self, version: &str) -> Option<String>;   // why a `--version` line is too old; None = accepted
}
#[non_exhaustive] #[derive(Default)]
pub struct Requirements { pub os: Option<String>, pub harnesses: Vec<RequiredProgram>,
    pub tools: Vec<RequiredProgram>, pub disk_mb: Option<u64> }   // TOML [theater.ID.requires], unknown keys refused
impl Requirements {
    pub fn with_os(self, os) -> Self; pub fn with_harness(self, RequiredProgram) -> Self;
    pub fn with_tool(self, RequiredProgram) -> Self; pub fn with_disk_mb(self, u64) -> Self;
    pub fn programs(&self) -> impl Iterator<Item = &RequiredProgram>;     // harnesses, then tools
    pub fn allows_harness(&self, harness: &str) -> bool;                  // listed, or none listed
    pub fn check(&self, report: &ProbeReport) -> Vec<Unmet>;
}
#[non_exhaustive] #[derive(Default)]
pub struct ProvisionSpec { pub runner: Option<String> /* path on the Station */, pub plugins: Vec<String>,
    pub install: BTreeMap<String, String> /* program -> sh -c command */, pub install_timeout_secs: Option<u64> }
impl ProvisionSpec { pub fn with_runner(self, path) -> Self; pub fn with_plugin(self, path) -> Self;
    pub fn with_install(self, program, command) -> Self; }
#[non_exhaustive] pub struct Unmet { pub name: String /* program, "os" or "disk" */, pub requirement: String, pub reason: String } // Display "requirement: reason"
#[non_exhaustive] #[derive(Default)]
pub struct ProbeReport { pub checked_at: u64, pub os: Option<String>, pub found: BTreeMap<String, String> /* name -> --version line */,
    pub free_mb: Option<u64>, pub resources: Option<Resources>, pub unmet: Vec<Unmet>, pub ambient: Vec<String>, pub error: Option<String> }
impl ProbeReport { pub fn failed(error: impl Into<String>) -> Self; pub fn is_fit(&self) -> bool; }
#[non_exhaustive] pub enum StepStatus { Ok, Changed, WouldChange, Failed }   // as_str(): ok | changed | would-change | failed
#[non_exhaustive] pub struct ProvisionStep { pub name: String, pub status: StepStatus, pub detail: String }
/// Never fails: an unprobeable Theater answers a report with only `error` set.
pub fn probe(client: &TransportClient, capabilities: &Capabilities, requires: Option<&Requirements>, dir: Option<&str>) -> ProbeReport;
/// CPUs (nproc), 1-minute load, MemTotal/MemAvailable; under cgroup v2 the container's
/// cpu.max quota and memory.max (less memory.current) narrow them; macOS via sysctl (no
/// available memory). Never cpu_percent. Err: no `run`, a Windows host, nothing recognised.
pub fn probe_resources(client: &TransportClient, capabilities: &Capabilities) -> Result<Resources, String>;
/// Steps in order: os, disk, tools, harnesses, runner, plugins. `check` writes nothing.
pub fn provision(client: &TransportClient, theater: &Theater, check: bool) -> Vec<ProvisionStep>;
```

Both block on the transport and need a POSIX `sh` there (`command -v`, `df -Pk`, `sha256sum`
or `shasum -a 256`, `chmod`, `mv`). The probe names ambient credentials (git credential store
or helper, SSH agent or private keys, GitHub CLI, Claude Code and Codex logins, provider keys
in the login environment) and never reads their values. A copied file is written to
`<dest>.ai6-partial` and moved over `<dest>`, so a running runner binary can be replaced; it
is copied only when its SHA-256 differs. `runner.command` must be a path (`~/` = the
transport's default directory); without `provision.runner` the Station's own executable is
used only when the Theater reports the Station's `os` and `arch`. The `ai6 theater
provision THEATER [--check] [--config-dir DIR] [--plugin-path P]...` command prints one line
per step and exits 1 when a step failed, or with `--check` when one would change.
`Plugins::theater_transport(&TheaterAccess)` builds the `TransportClient` it uses.

## Transports (`ai6::directorates::q::transport`)

How a Station reaches a Theater, as a Guatiao kind (`ai6_transport`; the forward handle is the
object kind `ai6_transport_forward`). Slot order is the ABI: `capabilities` is required and
every other operation is an appended slot with a default body that answers
`GUATIAO_ERR_UNSUPPORTED`, so a transport implements only what it can do. Calls block.

```rust
pub trait Transport: Send + Sync {
    fn capabilities(&self) -> Result<Value, ProviderError>;           // Capabilities; never contacts the host
    fn probe(&self) -> Result<Value, ProviderError>;                   // contacts the host; default = capabilities
    fn open_terminal(&self, spec: &Value) -> Result<Object<dyn TerminalSession>, ProviderError>; // terminal::Spec
    fn run(&self, spec: &Value) -> Result<Value, ProviderError>;       // RunSpec -> Output
    fn read_file(&self, spec: &Value) -> Result<Value, ProviderError>; // ReadSpec -> FileData
    fn write_file(&self, spec: &Value) -> Result<(), ProviderError>;   // WriteSpec
    fn list_dir(&self, path: &str) -> Result<Value, ProviderError>;    // DirListing, sorted by name
    fn forward(&self, spec: &Value) -> Result<Object<dyn TransportForward>, ProviderError>; // ForwardSpec
}
pub trait TransportForward: Send + Sync { fn info(&self) -> Result<Value, ProviderError>; fn close(&self) -> Result<(), ProviderError>; }
pub struct Capabilities { pub terminal: bool, pub run: bool, pub files: bool, pub forward: bool,
    pub os: Option<String>, pub arch: Option<String>, pub notes: Vec<String> }
pub struct TransportConfig { pub settings_toml: String }   // a provider with settings; it parses them and refuses unknown keys
pub struct RunSpec { pub argv: Vec<String>, pub cwd: Option<String>, pub env: Option<BTreeMap<String, String>>,
    pub stdin: Option<Bytes>, pub timeout_ms: u64 /* 0 waits */, pub max_output: u64 /* per stream; DEFAULT_MAX_OUTPUT */ }
pub struct Output { pub code: Option<i64>, pub signal: Option<i64>, pub stdout: Bytes, pub stderr: Bytes,
    pub timed_out: bool, pub truncated: bool }
pub struct ReadSpec { pub path: String, pub offset: u64, pub limit: u64 /* DEFAULT_READ_LIMIT */ }
pub struct FileData { pub data: Bytes, pub size: u64 }   // size of the whole file
pub struct WriteSpec { pub path: String, pub data: Bytes, pub append: bool, pub create_dirs: bool }
pub struct DirEntry { pub name: String, pub kind: String /* file | dir | link | other */, pub size: u64 }
pub struct ForwardSpec { pub remote_host: String /* 127.0.0.1 */, pub remote_port: u16 /* 0: host chooses */, pub target: String /* host:port on the Station */ }
pub struct ForwardInfo { pub remote_host: String, pub remote_port: u16 }
pub struct TransportClient;  // typed wrapper: new(Box<dyn Transport>), capabilities, probe, open_terminal, run, read_file, write_file, list_dir, forward
pub fn unsupported(what: &str) -> ProviderError;  pub fn is_unsupported(&ProviderError) -> bool;
pub fn describe(&ProviderError) -> String;         // the provider's own words
pub mod contract { pub struct Host<'a> { pub shell: &'a [&'a str], pub expand: fn(&str) -> String, pub scratch: &'a str }
    pub fn check(transport: Box<dyn Transport>, host: &Host<'_>); }   // the suite every transport passes
```

A model grows by fields that default; the floor hashes of both kinds are pinned in a test. A
missing file or directory is `GUATIAO_ERR_NOT_FOUND`. `contract::check` runs a transport
against its own `Capabilities`: what it claims works, what it does not claim answers
`GUATIAO_ERR_UNSUPPORTED`.

Providers, each with its own API header: `ai6-transport-local` (`run`, `files`),
`ai6-transport-ssh` (`terminal`, `run`, `files`, `forward`), `ai6-transport-adb` (`run`,
`files`, `forward`), `ai6-transport-container` (`run`, `files`) and `ai6-transport-qemu`
(`run`, `files`). `Plugins::transports()` and `Plugins::transport(provider, config)` find and
build them.

### Kit (`ai6::directorates::q::transport::kit`)

What the providers share, for a host reached through a command:

```rust
pub fn quote(word: &str) -> String;                      // one POSIX-shell word
pub fn command_line(argv: &[String], cwd: Option<&str>, env: Option<&BTreeMap<String, String>>)
    -> Result<String, String>;                           // cd 'cwd' && exec env 'K=V' 'argv0' 'arg'…
pub fn is_variable_name(name: &str) -> bool;
pub struct ProcessInput<'a> { pub stdin: Option<&'a [u8]>, pub timeout_ms: u64, pub max_output: u64 }
pub fn run_process(command: &mut Command, input: &ProcessInput<'_>) -> Result<Output, ProviderError>;
pub fn io_error(error: &io::Error, what: &str) -> ProviderError;   // NotFound -> GUATIAO_ERR_NOT_FOUND
pub trait Shell { fn sh(&self, script: &str, stdin: Option<&[u8]>, max_output: u64) -> Result<Output, ProviderError>; }
pub fn read_file(shell: &dyn Shell, spec: &ReadSpec) -> Result<FileData, ProviderError>;
pub fn write_file(shell: &dyn Shell, spec: &WriteSpec) -> Result<(), ProviderError>;
pub fn list_dir(shell: &dyn Shell, path: &str) -> Result<DirListing, ProviderError>;
pub const EXIT_NOT_FOUND: i64 = 44;
```

`run_process` kills a process at its timeout (not its descendants) and keeps at most
`max_output` bytes of each stream. The file functions are POSIX `sh` scripts over `Shell`
(`wc`, `tail`, `head`, `cat`, `mkdir`, `printf`): a script that exits `EXIT_NOT_FOUND` is
`GUATIAO_ERR_NOT_FOUND`, and a name holding a tab or a newline is not listed correctly.

### Remote-hands tools

When a Mission is placed in remote-hands mode, the Station starts a per-Session MCP endpoint
(feature `cables-mcp`, `127.0.0.1`, bearer token) with `theater_run`, `theater_read`,
`theater_write` and `theater_list`, and puts `AI6_THEATER_MCP_URL` and `AI6_THEATER_MCP_TOKEN`
in the handler's environment. A handler that sees both is in remote-hands mode: it must switch
off its own shell and file tools and register that server (the Claude and Codex handlers do).
`theater_run` passes every command (`command`, or `argv` quoted into one line) through the
green ink gate before running it (`decide` over `classify`, the Theater's repository read with
`git` over the same transport, the Station's `check_approval` answers): a held command returns
`{"error": "Green ink: …"}` naming the `kind` and `target` for `request_approval`, and nothing
runs.

## Placement (`ai6::directorates::m::placement`)

M's decision on where a Mission may run. It is a pure function of plain facts, so the
scheduler and the Universal Exports preview share it.

```rust
#[non_exhaustive]
pub struct OperationPolicy {
    pub operation: String,
    pub station_labels: BTreeMap<String, String>,
    pub stations: Vec<String>,        // empty allows every Station
    pub theater: Option<String>,      // bound Theater, if any
    pub checkout: bool,               // the Operation has a local checkout the Station must hold
    pub agents: Vec<AgentFacts>,
    pub placement: PlacementStrategy, // serde default Spread
}
#[non_exhaustive]
pub enum PlacementStrategy { Spread, Pack }   // default Spread; serde/as_str "spread" | "pack"
pub const DEFAULT_MIN_FREE_MEMORY_MB: u64;    // 512
pub const DEFAULT_MAX_CPU_PERCENT: u32;       // 95
#[non_exhaustive]
pub struct ResourceLimits { pub min_free_memory_mb: u64, pub max_cpu_percent: u32 } // Default: the two consts
impl ResourceLimits {
    pub fn new(min_free_memory_mb: u64, max_cpu_percent: u32) -> Self;
    /// "it has N MiB of memory free, below min_free_memory_mb = M" or
    /// "its CPU is N% busy, above max_cpu_percent = M"; an unreported figure never rejects.
    pub fn rejection(&self, resources: &Resources) -> Option<String>;
}
impl OperationPolicy {
    pub fn new(operation: impl Into<String>) -> Self;
    pub fn from_roster(roster: &Roster, operation_key: &str) -> Result<Self, RosterError>;
}
#[non_exhaustive]
pub struct AgentFacts { pub id: String, pub handler: Option<String>, pub priority: i64, pub roles: Vec<String>, pub tags: Vec<String> }
#[non_exhaustive]
pub struct StationFacts {
    pub station: String, pub healthy: bool, pub silent_for: u64, pub roles: Vec<String>,
    pub capacity: usize, pub load: usize, pub labels: BTreeMap<String, String>,
    pub checkouts: Vec<String>, pub handlers: Option<Vec<String>>, pub theaters: Vec<TheaterSummary>,
    pub resources: Option<Resources>,   // None from a Station that did not report them
    pub running: usize,                 // its published `running`; 0 when absent
    pub limits: ResourceLimits,         // its published `resource_limits`; default when absent
}
impl StationFacts { pub fn occupied(&self) -> usize; }  // max(load, running): what capacity is judged on
/// A host's or Theater's CPU and memory; an unreported value is None, never guessed.
/// `Eq` holds because every value is finite (JSON has no NaN; the samplers drop them).
#[non_exhaustive]
#[derive(Default)]
pub struct Resources {
    pub cpus: Option<f64>,               // a fraction under a container's CPU quota
    pub cpu_percent: Option<f64>,        // 0-100 since the previous sample
    pub load_1m: Option<f64>,            // none on Windows
    pub memory_total_mb: Option<u64>, pub memory_available_mb: Option<u64>,
    pub sampled_at: u64,                 // Unix seconds
}
impl Resources {
    pub fn cpu_busy(&self) -> Option<f64>;    // 0-1: cpu_percent, else load_1m / cpus
    pub fn memory_used(&self) -> Option<f64>; // 0-1, when total and available are known
}
impl StationFacts {
    /// `load` = Missions already leased to the Station; silent for longer than `window` = unhealthy.
    /// A key the check-in lacks claims nothing: no roles, capacity 0, no Theaters.
    pub fn from_record(record: &StationRecord, now: f64, window: Duration, load: usize) -> Self;
}
#[non_exhaustive]
#[derive(Default)]
pub struct PlacementRequest {
    pub stations: Vec<String>,        // the Mission's narrowing; never widens the Operation's list
    pub agent: Option<String>, pub role: Option<String>, pub tags: Vec<String>,
    pub deciding_station: Option<String>, // wins a tie on load
}
#[non_exhaustive]
pub struct PlacementChoice { pub station: String, pub theater: String, pub agent: String, pub handler: Option<String>, pub mode: ExecutionMode,
    pub score: f64 }                  // 0-4, thousandths; serde default 0; Eq holds (always finite)
#[non_exhaustive]
pub enum ExecutionMode { Field, RemoteHands }   // default Field; as_str() -> "field" | "remote-hands"
/// How a Session would run on `theater`, or the capability that is missing.
pub fn execution_mode(theater: &TheaterSummary) -> Result<ExecutionMode, String>;
/// Why the Theater's `requires` rule it out: unchecked, or an unmet os/tool/disk. Ok without
/// `requires`, and for an ephemeral template (fitted out when a Theater is made from it).
pub fn fitness(theater: &TheaterSummary) -> Result<(), String>;
/// In field mode only: the handler must be one of `requires.harnesses` (when any) and installed.
pub fn harness_verdict(theater: &TheaterSummary, mode: ExecutionMode, handler: &str) -> Result<(), String>;
#[non_exhaustive]
pub struct PlacementRejection { pub station: Option<String>, pub theater: Option<String>, pub agent: Option<String>, pub reason: String }
#[non_exhaustive]
pub struct Placement { pub choices: Vec<PlacementChoice>, pub rejections: Vec<PlacementRejection> }
impl Placement { pub fn best(&self) -> Option<&PlacementChoice>; }
pub fn place(policy: &OperationPolicy, request: &PlacementRequest, stations: &[StationFacts]) -> Placement;
/// Rules `agent` out everywhere while no account of its `pools` has room: "no account with
/// headroom in pools a, b; the first limit resets at T (Unix seconds)" (`frees_at`), else
/// "…; each account is limited or at its max_concurrent". Intake keeps the brief queued.
pub fn no_account_headroom(agent: &str, pools: &[String], frees_at: Option<f64>) -> PlacementRejection;
```

`place` returns every eligible (Station, Theater, Agent) choice, best first, and one
rejection for everything ruled out; the `reason` is a sentence an operator can act on.
Checks, in order: the Operation's `stations` list, the Mission's `stations`, the `execution`
role, health (a check-in no older than `DEFAULT_LEASE`), capacity (`occupied()`), the
Station's `limits` against its `resources`, then per Theater: not
`made_for` a Mission, the Operation's `theater` binding, reachability and capabilities
(`execution_mode`), `fitness`, the Station's `limits` against the Theater's `resources`
(not for `local` access, judged with the Station),
`station_labels` (matched as globs against the Station's labels overlaid by the Theater's)
and the Operation's checkout (on the Station's own host in field mode; on any other Theater,
or in remote-hands mode, the Operation must be bound to the Theater or the Theater must set a
`workdir`). Choices are pushed in a base order (Stations by `occupied()`, deciding Station,
ID; `local` Theater first; Agent priority) and then stable-sorted by `score` (ascending for
`Spread`, descending for `Pack`). `score` = occupied/capacity + `cpu_busy` + `memory_used`
(of the Theater's resources, the Station's for `local` access) + the Theater's `sessions`
/capacity, each capped at 1; an unreported figure adds 0.

`execution_mode` is field mode when the harness can run on the Theater and reach the
provider's API (`egress` not `none`; `forward` for `station`), and `agents` is not
`remote-hands`. The harness can run there when the Theater has a runner (on another host,
reached with `run`); without one, on the Station's own host with `terminal` and `files`, or
on another host for the detached wrapper with `run`, `files`, a reported POSIX `os` and a
declared harness. Otherwise it is remote-hands mode when the transport can `run`;
`agents = "local"` or `"remote-hands"` allow only that mode. A Theater that reported no
capabilities is rejected unless it is the Station's own host. The reason names what is missing.
A posted Agent is skipped for a missing handler on that Station (a Station that reported no
handlers is not excluded), so the next qualified Agent is the backup, and per Theater by
`harness_verdict` (with `station`, `theater` and `agent` set). A Station refuses to start a
Session the same two checks rule out. Order is the
least-loaded Station (the deciding Station wins a tie, then Station ID), the `local`
Theater first, then Agent priority. Reasons that apply to an Agent everywhere (not the
requested Agent, no handler configured, role or tag mismatch) have no `station`.

## Provider limits (`ai6::directorates::m::limits`)

```rust
pub const DEFAULT_LIMIT_FALLBACK_SECS: u64 = 1800;
pub const RESET_GRACE_SECS: i64 = 15;
#[non_exhaustive] pub enum OnLimit { Rotate /* default */, Wait, Backup }   // serde: "rotate" | "wait" | "backup"
impl OnLimit { pub fn as_str(self) -> &'static str }
pub fn parked_until(now: i64, resets_at: Option<i64>, fallback_secs: u64) -> i64
```

`parked_until` honours a reset time in the future plus `RESET_GRACE_SECS`; a missing or
already-past one waits `fallback_secs` so a limit that outlives its stated reset cannot
loop. An Operation sets `on_limit = "rotate" | "wait" | "backup"` and `limit_fallback_secs`
(above zero) in `operations.toml`; an unknown value or zero fails the roster load. Whatever
the policy, the Session's pool account is marked limited until the reset (or
`fallback_secs` from now) and freed. `Rotate`, when another account of the Agent's pools has
room, parks the Mission until now so the next scheduler pass resumes the same provider
Session on another account (an account kind that `carries_session`), or queues the Mission
again for the same Agent as a new attempt (Codex and `config-dir` kinds); otherwise it waits.
`Wait` parks the Mission and resumes the same provider Session; `Backup` posts the next
qualified Agent whose handler differs from the limited one as a new attempt, and parks when
none exists.

## Registry backend (`ai6::directorates::registry::backend`)

The Registry's operations are traits, one per record family, so the Station and Universal
Exports can run on any store. `RegistryBackend` is all of them together, implemented for
every type that implements each family (a blanket impl: nothing to write for it).

```rust
pub trait BriefStore;        // proposals, queued briefs, claims, leases, outcomes (store.rs)
pub trait StationStore;      // check-ins, Station settings, Headquarters election (store.rs)
pub trait AccessTokenStore;  // Universal Exports access tokens (store.rs)
pub trait CableStore;        // Cables, their delivery state and send quotas (store.rs)
pub trait AuditStore;        // audit records (store.rs)
pub trait AgentRoleStore;    // the per-Operation Agent-role overlay (store.rs)
pub trait AgentRecordStore;  // Agent track records (agent_records.rs)
pub trait ClearanceStore;    // clearance grants per Agent and Operation (clearance.rs)
pub trait ApprovalStore;     // green ink and class authorisations (approvals.rs)
pub trait LogbookStore;      // each Station's Head of Station reports (logbook.rs)
pub trait QuarantineStore;   // quarantines and incidents (quarantine.rs)
pub trait SessionStore;      // Session records and fenced leases (sessions.rs)
pub trait SleeperStore;      // Sleeper authorizations and fired windows (sleepers.rs)
pub trait SourceStore;       // Source cursors, ingested items, reports owed (sources.rs)
pub trait TimelineStore;     // the activity timeline (timeline.rs)
pub trait UsageStore;        // usage ledgers and daily spend (usage.rs)
pub trait AccountStore;      // pool account claims, quota and daily tokens (accounts.rs)
pub trait VettingStore;      // held briefs and vetting clearances (vetting.rs)
pub trait RegistryBackend: AccountStore + AgentRecordStore + BriefStore + StationStore + AccessTokenStore + CableStore
    + AuditStore + AgentRoleStore + ApprovalStore + ClearanceStore + LogbookStore + QuarantineStore + SessionStore
    + SleeperStore + SourceStore + TimelineStore + UsageStore + VettingStore + Send + Sync {}
impl<T: /* every family */ + Send + Sync> RegistryBackend for T {}

/// `sqlite:PATH` / `sqlite://PATH` / `postgres://…` / `postgresql://…` → SqlRegistry;
/// anything else → FileRegistry (a directory, not touched until used).
pub fn open(location: &str) -> io::Result<Arc<dyn RegistryBackend>>;
/// As `open`, a SQL Registry connected with `options` (features `sqlite` or `postgres`).
pub fn open_with(location: &str, options: &SqlOptions) -> io::Result<Arc<dyn RegistryBackend>>;
/// `open(AI6_REGISTRY)`, else the directory `AI6_HOME/registry` or the platform state dir;
/// a PostgreSQL location without a password takes `AI6_REGISTRY_PASSWORD`.
pub fn from_environment() -> io::Result<Arc<dyn RegistryBackend>>;
```

Every trait is re-exported at the crate root. The methods are synchronous; async callers run
them on a blocking thread. Calls through `dyn RegistryBackend` need no import; calling them
on a `FileRegistry` or `SqlRegistry` value needs the family trait in scope
(`use ai6::directorates::registry::backend::*`). The sections below give each family's
methods under the module that defines it. `FileRegistry` keeps them in one directory every
Station shares; `SqlRegistry` (features `sqlite`, `postgres`) in a database. A SQL location
without its feature is `Unsupported`. The daemon takes `Arc<dyn RegistryBackend>` through
`ServeConfig::with_registry` (default `backend::from_environment()`), Universal Exports
through `Gateway::with_registry` (its command uses `from_environment()`), and
`CableRelay::new` takes one too; the `ai6 quarantine` and `ai6 sleeper` commands open
`from_environment()`.

## SQL Registry (`ai6::directorates::registry::sql`, features `sqlite` or `postgres`)

```rust
#[derive(Debug)]
pub struct SqlRegistry;   // every Registry family; also at the crate root
impl SqlRegistry {
    /// `sqlite:PATH` or `sqlite://PATH` (a SQLite file), or a `postgres://` /
    /// `postgresql://` URL whose parameters pass to the server (such as
    /// `options=-c%20search_path%3Dai6` for a schema), besides `sslmode`, `sslrootcert`
    /// and `password_file`, read here. Creates or upgrades the tables.
    pub fn open(location: &str) -> io::Result<Self>;   // open_with(location, &SqlOptions::default())
    pub fn open_with(location: &str, options: &SqlOptions) -> io::Result<Self>;
}

#[derive(Clone, Debug /* password shown as *** */)]
pub struct SqlOptions;   // also at the crate root; Default = for_lease(DEFAULT_LEASE)
impl SqlOptions {
    pub fn for_lease(lease: Duration) -> Self;   // timeouts derived from the HQS lease
    pub fn with_password(self, password: impl Into<String>) -> Self;
}

/// The location with its password (userinfo or `password=`) replaced by `***`.
pub fn redact(location: &str) -> String;
/// What a location gives up, for a log line: sslmode=disable, sslmode=require, a password
/// in the location. Empty at the defaults and for directories and SQLite.
pub fn weakenings(location: &str) -> Vec<String>;
```

`open` fails `InvalidInput` for another location or an `sslmode` other than `verify-full`,
`require` or `disable`, `Unsupported` without the feature or for a schema newer than this
build (the version is in `ai6_schema`), and with the database's error (its cause included,
such as which certificate check failed) when it cannot be reached; a PostgreSQL location
connects at once. Writes also refuse, `Unsupported`, while the database's schema version is
newer than this build's.

TLS: `sslmode=verify-full` is the default: TLS (rustls, ring), with the server's certificate
checked against the platform's roots (`rustls-native-certs`), or, as libpq does, only
against `sslrootcert=PATH` (PEM) when given (`sslrootcert=system` keeps the platform's),
and its name or IP address checked. `sslmode=require` encrypts but accepts any certificate;
`sslmode=disable` sends everything in clear. Both are accepted and named by `weakenings`;
`ai6 serve` and `universal-exports` print them at start. A password comes from the location,
from `password_file=PATH` (its first line), or from `SqlOptions::with_password` when the
location has none (`ai6 serve` reads `AI6_REGISTRY_PASSWORD` and `AI6_CONFIG_PASSWORD`;
`backend::from_environment` reads `AI6_REGISTRY_PASSWORD`); a password in the location is
visible in the process list. Messages from the command show locations through `redact`.

Timeouts (`SqlOptions::for_lease(L)`, at least 1 s each): each session runs
`statement_timeout`, `lock_timeout` and `idle_in_transaction_session_timeout` of `L/4`, so a
Station that stalls inside a transaction (a paused VM, a half-open connection) loses its
session, and the clock row it held, well before a standby must take over; the connection
uses `connect_timeout` and `keepalives_idle` of `L/6` and `tcp_user_timeout` of `L/4`. A
location that sets one itself (the parameter, or `-c name=…` in `options`) keeps its own.
`ai6 serve` derives them from its `--headquarters-lease`. A lock or statement timeout maps
to `TimedOut`.

Each record family
has its own table (`ai6_briefs`, `ai6_brief_leases`, `ai6_headquarters`, `ai6_sessions`,
`ai6_approvals` and so on), holding the record as JSON text in the file Registry's shape
beside the columns a lookup needs, and a `revision` column. Every write transaction first
advances its row of `ai6_clock` (`registry`): that row lock serializes writers across
Stations as the directory lock does, so claims, brief leases, the Headquarters election and
term, and Session fencing keep the file Registry's guarantees, and its new value is the
revision stamped on each row written. SQLite runs writes as `BEGIN IMMEDIATE` with a 30 s
busy timeout and WAL. PostgreSQL runs writes `READ COMMITTED` and reads `REPEATABLE READ
READ ONLY`, whatever the server's default, and serializes concurrent migrations of one
schema with an advisory lock. Brief, Headquarters and Session leases and check-in times
are taken from the database's clock (`clock_timestamp()`, SQLite `unixepoch('subsec')`)
inside the transaction that writes them, so a Station whose clock is off cannot take a live
lease early; the clock-explicit test hooks keep their given time. A unique violation maps
to `AlreadyExists`, a lost PostgreSQL connection to `ConnectionAborted`; the next call
reconnects. Each handle drives its PostgreSQL connection on a runtime thread of its own and
waits on it with a parking executor, so calls work on any thread, an async worker included
(they block it, as file I/O does), and dropping a handle never blocks; one connection per
handle, one transaction at a time.

Where the stores differ: keys are stored as given rather than as file-safe names, so two
quarantine or usage keys that differ only in characters a file name cannot hold stay
apart; a finished brief has one state (`finish` of a brief that was never claimed moves it
out of the queue); submitting a proposal already approved or rejected is answered as a
repeat rather than queued again; a lapsed Headquarters lease is replaced in place, with no
`lease.lapsed.*` record; the transcript `finish` files is kept in `ai6_briefs.transcript`.
Bearer secrets are never stored: access tokens are kept by digest, credentials stay in Q's
secret backends.

## Bona fides (`ai6::directorates::registry::bona_fides`)

```rust
#[non_exhaustive] pub enum Origin { Operator, Source, Sleeper, Proposal, Review, RegistryDrop } // snake_case; as_str()
                                                       // Review: a Chief of Staff review Headquarters queued
#[non_exhaustive] pub struct Provenance {
    pub origin: Origin,
    pub principal: Option<String>,   // person ID | Source provider | sleeper name | proposing Agent; None for a drop
    pub reference: Option<String>,   // Source item | sleeper fingerprint | proposing provider Session
    pub item_url: Option<String>,
    pub received_at: i64,            // Unix seconds
    pub received_by: String,         // Station ID, or "universal-exports"
    pub digest: String,              // record_digest(brief), taken again when the Registry seals it
}
impl Provenance {
    pub fn new(origin: Origin, brief: &Brief, received_by: impl Into<String>, received_at: i64) -> Self;
    pub fn with_principal(self, impl Into<String>) -> Self;  pub fn with_reference(self, impl Into<String>) -> Self;
    pub fn with_item_url(self, Option<String>) -> Self;
    pub fn registry_drop(brief: &Brief, station: impl Into<String>, at: i64) -> Self;
    pub fn matches(&self, brief: &Brief) -> bool;   // digest still equals the record's
}
pub fn record_digest(brief: &Brief) -> String;  // "sha256:HEX" of the canonical JSON record without provenance
// Brief gains `pub provenance: Option<Provenance>` (serde default, omitted when None).
```

Every Registry keeps a brief's provenance apart from the record: the file Registry under
`provenance/<key>.json`, the SQL Registry in the `provenance` column of `ai6_briefs`. Reads
(`list_new`, `claim_new`, `read_claimed`, `promote_mission_proposal`,
`unreported_source_briefs`) set `Brief::provenance` from there and never from the record, and
drop a kept one whose digest no longer matches the record: any edit after sealing (its
Operation, assignment, Stations, link, data or text) leaves it without one. Finishing a brief
spends its provenance (file: moved to `STATE/<key>.provenance`; SQL: moved to
`finished_provenance`), where only `unreported_source_briefs` reads it, so the same record
queued again has none. `None` means the brief was put in the queue without one (by hand, or
`submit_brief` with none). The Station then records it on the
Mission as `Origin::RegistryDrop` with its own ID, and the Mission's Sessions start exposed
by `brief:registry_drop` (`BRIEF_REGISTRY_DROP`). A finished `source:` brief is reported to
its Source only when `reportable` holds: a `source` provenance for this record naming the
provider and item, and the brief's Operation equal to its Source data's. Anything with write access to the Registry's own
records can still forge them.

## Registry store (`ai6::directorates::registry::store`)

```rust
pub struct AuditRecord {
    pub id: String,
    pub actor: String,
    pub action: String,
    pub operation: String,
    pub resource: Option<String>,   // `station:<id>` or `config:roster`; required without an Operation
    pub target: String,
    pub at: f64,
}

pub struct StationSettings { /* validated settings for one Station */ }  // Serialize: station,
                                         // max_concurrent_missions, min_free_memory_mb, max_cpu_percent
impl StationSettings {
    pub fn try_new(station: impl Into<String>, max_concurrent_missions: usize) -> io::Result<Self>; // default ResourceLimits
    pub fn with_resource_limits(self, limits: ResourceLimits) -> io::Result<Self>; // max_cpu_percent > 100 refused
    pub fn resource_limits(&self) -> ResourceLimits;
    pub fn station(&self) -> &str;
    pub fn max_concurrent_missions(&self) -> usize;
}
pub const MAX_STATION_MISSION_CAPACITY: usize; // 1024
pub struct MissionProposal {
    pub id: String,
    pub operation_id: String,
    pub parent_mission_id: MissionId,
    pub proposer_agent_id: String,
    pub proposer_session_id: SessionId,
    pub brief: Brief,
    pub created_at: f64,
    pub vetting: Option<Vetting>,   // serde default, skipped when None: set by Headquarters
    pub parent_role: Option<String>, // serde default: the role the proposing Session's Mission was given
}
impl MissionProposal { pub fn with_parent_role(self, Option<String>) -> Self; }  // set by the Station, never the Agent
BriefStore::submit_mission_proposal(&MissionProposal) -> io::Result<()>   // a retry keeps the stored vetting
BriefStore::list_mission_proposals(operation: &str, limit: usize) -> io::Result<Vec<MissionProposal>>
BriefStore::promote_mission_proposal(operation: &str, proposal_id: &str, received_by: &str) -> io::Result<Brief>
BriefStore::reject_mission_proposal(operation: &str, proposal_id: &str) -> io::Result<MissionProposal>
BriefStore::set_proposal_vetting(operation: &str, proposal_id: &str, vetting: &Vetting) -> io::Result<MissionProposal>  // NotFound unless pending
```

`FileRegistry::open(root)` opens the file-backed Registry without creating or changing
files; `from_environment()` resolves `AI6_REGISTRY`, then
`AI6_HOME/registry`, then the platform state directory. `list_new()` returns valid
version-1 `Brief` values ordered by priority descending, creation time ascending, then
key. It skips malformed briefs, rejects unknown fields and
newer versions, and bounds scans to 1,000 files and individual briefs to 1 MiB. Listing
is read-only. `claim_new(key)` moves one queued brief to `claimed/` under the shared
`.ai6-registry.lock` but does not create a Station lease. Lease-aware operations are:

```rust
pub const DEFAULT_LEASE: Duration = Duration::from_secs(60);
record_audit(entry: &AuditRecord) -> io::Result<()>
list_audit(limit: usize) -> io::Result<Vec<AuditRecord>>
audit_records() -> io::Result<Vec<AuditRecord>>
record_event(event: &TimelineEvent) -> io::Result<()>
timeline_events(operation: &str) -> io::Result<Vec<TimelineEvent>>
station_settings(station: &str) -> io::Result<Option<StationSettings>>
set_station_settings(station: &str, settings: &StationSettings) -> io::Result<()>
submit_brief(brief: &Brief) -> io::Result<()>
claim_with_lease(key: Option<&str>, station: &str, duration: Duration, meta: BTreeMap<String, serde_json::Value>) -> io::Result<Option<Lease>>
lease(key: &str) -> io::Result<Option<Lease>>
read_claimed(key: &str) -> io::Result<Option<Brief>>
leases() -> io::Result<Vec<Lease>>
expired() -> io::Result<Vec<Lease>>
renew(key: &str, station: &str, duration: Duration) -> io::Result<bool>
recover(lease: &Lease, max_attempts: u32, by: &str) -> io::Result<Option<LeaseRecovery>>
hand_over(lease: &Lease, station: &str, duration: Duration, meta: BTreeMap<String, serde_json::Value>) -> io::Result<Option<Lease>>  // same claim and attempt; None if the lease changed or is live
finish(brief: &Brief, outcome: &serde_json::Value, ok: bool, transcript: Option<&[u8]>, station: Option<&str>) -> io::Result<bool>
```

`submit_brief` validates and atomically writes a unique Mission brief to `new/`; it
refuses a key already present in `new/`, `claimed/`, `done/` or `failed/`. Its
`Brief::provenance` is stored apart from the record (see Bona fides below).
Agent-authored Mission proposals are separately stored under `proposals/pending/<operation>/`
and never appear in `list_new()`. `promote_mission_proposal` moves an authorized proposal into
`new/` and is safe to retry; `reject_mission_proposal` files it under `proposals/rejected/`.
Universal Exports's authenticated, Operation-authorized review endpoints call these decision
methods, and Headquarters promotes a proposal itself only when its vetting passed, its
proposer Session was not exposed (no `exposure` in the brief's `data`) and a live class
authorisation of kind `proposal` covers it: proposer Agent (or the role the proposal asks for)
at that Agent's clearance, and the proposal's title against `target_pattern`. That approval
is a green ink request on the parent Mission (`ApprovalStore::approve_by_class`, one use),
audited as `mission.proposal.approved` with actor `class:ID`; the promoted brief is cleared of
`review`. Provider Session identity is bound by AI6, never supplied in the proposed brief.

`record_audit` stores a validated metadata-only `AuditRecord` under `audit/` and rejects
duplicate IDs. Each entry is bounded to 16 KiB and the listing scans at most 10,000
records. `list_audit` returns newest entries first and requires a result limit from 1 to
500; `audit_records` returns every stored record unsorted. Operation events contain actor,
action, Operation, target and Unix timestamp;
Station events use the explicit `resource = "station:<id>"` scope. Request prompts,
terminal data and credential values are not audit fields.

The activity timeline (`ai6::directorates::registry::timeline`) records what the audit log
does not:

```rust
pub const MAX_TIMELINE_EVENTS: usize = 10_000;
#[non_exhaustive]
pub struct TimelineEvent {
    pub id: String, pub at: f64,            // fresh UUID; Unix seconds
    pub kind: String,                       // "lifecycle" | "control" | "cable" | "sleeper"
    pub action: String,                     // a state, "<action>.<outcome>", "sent", "authorized", "revoked"
    pub operation: String,                  // whose timeline shows it
    pub station: Option<String>, pub mission: Option<String>,
    pub session: Option<String>, pub actor: Option<String>,
    pub detail: BTreeMap<String, String>,   // at most 8 safe names and IDs
}
impl TimelineEvent {
    pub fn new(kind, action, operation, at: f64) -> Self;
    pub fn station(self, ..) -> Self; pub fn mission(self, ..) -> Self;
    pub fn session(self, ..) -> Self; pub fn actor(self, ..) -> Self;
    pub fn detail(self, key, value) -> Self;
}
```

`record_event` refuses (`InvalidInput`) a kind, action or detail key that is not a bounded
lowercase name (`a-z0-9._-`, 64 bytes), an identity or detail value that is blank, over 256
bytes or holds control characters, more than 8 details, or a non-positive time; it stores the
event under `timeline/` named by time, and past `MAX_TIMELINE_EVENTS` removes the oldest.
`timeline_events(operation)` returns one Operation's events oldest first, skipping unreadable
files. `submit_cable` records a `cable`/`sent` event for a new Cable under the sender's
Operation (sender Station and Agent, Mission and Session, details `cable`, `scope`,
`recipients`; never the body or the target name), and `authorize_sleeper` /
`revoke_sleeper` record `sleeper`/`authorized` and `revoked` (detail `sleeper`). Universal
Exports records lifecycle transitions and control outcomes. A failed timeline write never
fails the Cable or authorization it describes. Events never hold prompts, messages, Cable
bodies, terminal bytes or secrets.

`StationSettings::try_new(station, capacity)` accepts a Station ID and a maximum
concurrent Mission count from 1 through 1,024. `StationStore::station_settings(station)`
reads the validated current value; `set_station_settings(station, settings)` persists it
under a collision-resistant Station-specific file and the shared Registry lock. A daemon
uses the live value for new Mission admission and scheduling; absent settings retain its
startup default, and unreadable settings fail closed. Existing provider Sessions are not
stopped when the limit is lowered. Every backend stores all four fields; a record missing one
is invalid.

`put_agent_role(operation, role)`, `list_agent_roles(operation)`, `agent_role(operation,
key)` and `remove_agent_role(operation, role_id)` manage validated, bounded role
overrides in `agent-roles/<operation>/`. These use the shared Registry lock for writes,
isolate every Operation, reject path-like identifiers and cap each Operation at 256 roles.
AI6 resolves the Operation override before the global role. Station check-ins publish the
global role catalog for authorized Universal Exports readers.

`read_claimed` reads a leased Brief without changing it. `claim_with_lease` returns
`None` when no brief matches. Renew returns `false` if the Station no longer owns the
claimed brief. `recover` returns `Requeued`, `Failed`, or `None` when another Station
changed the lease. `finish` returns `true` once the outcome is filed and `false` when the
caller's lease was lost. In the file Registry lease expiration uses each Station's wall-clock
Unix seconds, so shared Stations need synchronized clocks; `SqlRegistry` uses the database's
clock instead. Station liveness and Headquarters election use the same file
layout and `.ai6-registry.lock` (in a SQL Registry, its tables and clock row); the
scheduler does not yet use election for distributed placement.

```rust
pub fn check_in(&self, station: &str, info: BTreeMap<String, serde_json::Value>) -> io::Result<()>
pub fn stations(&self, within: Duration) -> io::Result<Vec<StationRecord>>
pub fn elect(&self, station: &str, ttl: Duration) -> io::Result<bool>
pub fn headquarters(&self) -> io::Result<Option<HeadquartersLease>>
pub fn station(&self) -> &str
pub fn seen(&self) -> f64
pub fn info(&self) -> &BTreeMap<String, serde_json::Value>
pub fn expires(&self) -> f64
```

`check_in` atomically replaces the Station's JSON record under `stations/`; caller values
for the reserved `station` and `seen` keys are ignored. Station IDs are non-empty, at most
256 UTF-8 bytes and contain no control characters. `stations(within)` returns fresh,
valid records in stable filename order and skips malformed, stale or oversized files.
Records are bounded to 1 MiB and scans to 1,000 Stations. `elect` returns `true` when it
acquires or renews the lease and `false` when another Station still holds it. An expired
record moves to `headquarters/lease.lapsed.<station>` before the new lease is written.
`headquarters` returns the current record even when its lease has expired.
`HeadquartersLease::term() -> u64` is the election term: it grows by one whenever a
different Station takes the lease and is unchanged by the holder renewing it. Headquarters
leases are bounded to 1 MiB and use wall-clock Unix seconds, so participating Stations
need synchronized clocks. The daemon publishes `roles`, `labels`, `capacity`, `running`,
the host's `resources` (sysinfo: CPU count, CPU percent from the second sample on, the
one-minute load average except on Windows, total and available memory; in this crate's own
tests every check-in repeats the process's first sample), its `resource_limits` (Registry
settings, else the defaults), each Theater's running `sessions`,
local Operation paths, available `handlers`, reachable `theaters` (with their capabilities) and each Operation's
placement policy (`operation_policies`, omitted with a log line when over 512 KiB), the
Agents whose retirement is near or due (`retirements`: `{agent, retire_on, on_retire,
status}`, also logged at start and on a roster reload), the configuration it runs (`config`, a
`ConfigStatus`) and the latest report in its logbook (`head_of_station`, a `StationReport`,
once one is filed) with each Station check-in every 15
seconds and right after it applies a reload, alongside host and
platform metadata. `StationConfig` defaults to both `Control` and `Execution`, no
pinned terminal and only the `local` Theater; `with_roles` replaces the role set, `with_label`
adds a placement label, `with_theater` declares a Theater in code,
`with_safe_house_hygiene(SafeHouseHygiene)` sets which directories of a kept safe house are
rebuildable caches and how long a terminal Mission's safe house is left alone first,
`with_loadouts(LoadoutCatalog)` replaces `loadouts.toml`,
`with_secret_backend(impl SecretBackend + 'static)` replaces `StationSecrets`,
`with_account_login(impl AccountLogin + 'static)` replaces `CommandLogin` for Codex pool
accounts, `with_account_homes(dir)` moves their per-Mission `CODEX_HOME`s from
`<state>/accounts` (cleanup removes only the directories it made, marked
`.ai6-account-homes`), `with_account_limit_horizon(Duration)` caps how long a limit keeps an
account out (default 8 days),
`with_headquarters_lease(Duration)` sets how long the Headquarters lease lasts between
renewals (default `DEFAULT_LEASE`, 60 s; zero makes startup fail): how long a standby
control Station waits before taking over, `with_config_poll(Duration)` sets how often it
checks its roster source and `loadouts.toml` (default `DEFAULT_CONFIG_POLL`, 5 s; zero turns
reloading off; loadouts set with `with_loadouts` are never reloaded), and `with_terminal` pins the terminal provider. Without a pin the Station uses an available
terminal provider that suits the Agent's handler (`choose_terminal` in the terminal API). A
pin that gives a JSON harness a terminal is refused at Mission start. Empty roles make daemon startup fail. The current Rust scheduler does not yet use election for
distributed placement. Station and Agent limits count assignments in the shared Registry.
The default Station ID is `<host>-<process-id>`; `--station` and `AI6_STATION_ID` set an
explicit identity.
`Brief::key()` and `Brief::prompt()` expose the key and prompt fields used by Registry intake.
`Brief.stations` (default empty) limits a Mission to those Stations; it can only narrow the
Operation's `stations` list. The scheduler places each brief with `placement::place` over
every Station's published facts: a brief naming no Agent gets `None` from
`resolve_brief_assignment` (it only checks some unquarantined posted Agent qualifies) and
`place` picks the Agent per Station from that Station's `handlers`; quarantined Agents are
dropped from the policy first; the first choice whose Agent and Operation are within
`max_concurrent` is taken. The Headquarters Station's own capacity plays no part.

## Session records (`ai6::directorates::registry::sessions`)

The shared record of every Agent Session, under `sessions/<id>.json` in the Registry. A
Station's `missions.json` dies with it; this record does not.

```rust
#[non_exhaustive] pub enum SessionStatus { Active, Parked, Ended, Lost }
impl SessionStatus { pub fn is_closed(self) -> bool }   // Ended or Lost
#[non_exhaustive] pub struct SessionLaunch {         // what another Station needs to re-attach
    pub handler: String, pub terminal: String, pub working_directory: String,
    pub model: Option<String>, pub effort: Option<String>,
    pub mode: ExecutionMode,                 // default Field
    pub runner: Option<String>,              // the runner's Session key; only these are adopted
}
impl SessionLaunch { pub fn new(handler, terminal, working_directory) -> Self;
    pub fn with_model(self, Option<String>, Option<String>) -> Self;
    pub fn with_mode(self, ExecutionMode) -> Self; pub fn with_runner(self, key) -> Self; }
pub const MAX_RECENT_EVENTS: usize = 20;
pub const MAX_RECENT_EVENT_CHARS: usize = 300;
#[non_exhaustive] pub struct SessionRecord {
    pub session_id: String,                  // AI6's ID (not the provider's)
    pub mission_id: String,
    pub provider_session_id: Option<String>,
    pub station: String,                     // the lease holder
    pub theater: Option<String>,
    pub attempt: u32,                        // from 1
    pub predecessor: Option<String>,         // AI6 ID of the Session this one replaced
    pub status: SessionStatus,
    pub parked_until: Option<f64>,           // Unix seconds
    pub lease_expires: f64,
    pub fence: u64,                          // fencing token of the current lease
    pub exposure: Vec<Exposure>,             // untrusted input read; only grows
    pub launch: Option<SessionLaunch>,       // how the harness was launched, to re-attach to it
    pub recent: Vec<String>,                 // last events seen, oldest first, bounded as above
    pub checkpoint: Option<SafeHouseCheckpoint>, pub checkpointed_at: Option<f64>,
    pub lost: Option<String>,                // why, with SessionStatus::Lost
    pub account: Option<AccountRef>,         // the pool account it runs on, when pooled
}
impl SessionRecord { pub fn leased_at(&self, now: f64) -> bool }   // false once closed
pub struct NewSession   // NewSession::new(session_id, mission_id, station)
                        //   .with_provider_session(id) .with_theater(id) .with_attempt(n, predecessor)
                        //   .with_exposure(Vec<Exposure>) .with_launch(SessionLaunch) .with_account(AccountRef)
pub struct SessionUpdate  // default() .provider_session(id) .status(SessionStatus) .parked_until(f64)
                          //   .exposed(Vec<Exposure>) (adds sources not yet recorded)
                          //   .launch(SessionLaunch) .recent(Vec<String>) .checkpoint(SafeHouseCheckpoint, at)
                          //   .account(AccountRef)
#[non_exhaustive] pub enum SessionError { NotFound, Exists, Ended, HeldBy(String), Stale, Io(io::Error) }

pub trait SessionStore {
    fn open_session(&self, new: NewSession, ttl: Duration) -> Result<SessionRecord, SessionError>
    fn session(&self, id: &str) -> io::Result<Option<SessionRecord>>
    fn sessions(&self) -> io::Result<Vec<SessionRecord>>          // stable ID order
    fn mission_exposure(&self, mission_id: &str) -> io::Result<Vec<Exposure>>  // every Session of the Mission, earliest first
    fn take_session(&self, id: &str, station: &str, ttl: Duration) -> Result<SessionRecord, SessionError>
    fn renew_session(&self, id: &str, station: &str, fence: u64, ttl: Duration) -> Result<SessionRecord, SessionError>
    fn update_session(&self, id: &str, station: &str, fence: u64, update: SessionUpdate) -> Result<SessionRecord, SessionError>
    fn lose_session(&self, id: &str, reason: &str) -> Result<SessionRecord, SessionError>
    fn latest_session(&self, mission_id: &str) -> io::Result<Option<SessionRecord>>  // highest attempt
}
```

`lose_session` marks a Session whose lease has lapsed `Lost` with the reason (cut to 300
characters) and moves its fencing token on, so the old holder's next write is `Stale`;
`HeldBy` while the lease is live, `Ended` once closed. A `Lost` Session is closed like an
`Ended` one: it cannot be taken or written. `recent` keeps the last 20 lines, each cut to
300 characters.

A Station acts on a Session only while it holds the lease. `open_session` creates the
record held by `new`'s Station; `take_session` renews the lease for the current holder, takes
a lapsed one, and returns `HeldBy` while another Station's lease is live (`Ended` once the
Session is over). A new holder's `fence` is the larger of the Headquarters term and the
previous token plus one, so it only grows. `renew_session` and `update_session` present the
token and holder: any mismatch is `Stale`, and nothing is written. IDs are 1-128 letters,
digits, `-`, `_` or `.`, not starting with `.`; records are bounded to 64 KiB. Times are
wall-clock Unix seconds, so Stations need synchronized clocks. The daemon records a Session
when it starts, takes the lease again to resume it, mirrors Mission state into the record,
renews its leases with the Station check-in, and closes and abandons a Mission whose token
is refused. The daemon's launch record keeps the lease and fence in `missions.json`
(`session`, `None` until the Session is recorded). It also writes the Session's launch, its recent
events (at most every 15 seconds) and, when the Operation sets
`workspace.checkpoint_remote`, a safe-house checkpoint every `checkpoint_secs`.

Recovery (`ai6::directorates::m::recovery`): when a Station's brief lease and its Session's
lease have both lapsed, the Headquarters Station either hands the brief's lease to a Station
that adopts the Session or marks the Session lost and queues the brief again (or files it,
`on_lost = "fail"`). A new attempt links to the lost Session, starts its safe house from the
lost one's checkpoint and is told what it was doing.

A live Station gives a Session up the same way when its runner (or detached-wrapper harness)
stays out of reach for the Operation's `lost_after_secs` and one more probe through the
Theater's transport fails (runner gone, Session unknown to it, Theater unreachable; for the
wrapper, also a quiet harness whose process is gone with no `exit`), or when a runner
restart ends the Session with a `lost` event. It writes the Session's notes, lets its Session
lease lapse and marks it lost, then lets its brief lease lapse and recovers it under
`on_lost` (no new Registry method: a lapsed lease is lost and recovered as for a dark
Station). Its Mission book says `abandoned` (or `failed` when filed, or without a brief),
and a `lost` event carries the reason. The Headquarters treats an expired brief lease whose
latest Session is already `lost` under `on_lost`. A Station handed the brief back re-queues
its own `abandoned` copy of the Mission (`MissionStore::retry`) and the new attempt, on the
same Theater, first closes the lost runner Session. Re-attach tries after a restart or an
adoption back off by `reattach_delay`; once they have failed for `lost_after_secs`, the
runner (or the Theater's transport) is probed and the Session given up the same way.

```rust
/// The live Station that should re-attach to `session`'s runner harness, placed like a new
/// Session but only on that Theater, in field mode, never on `session.station`. Errs, with
/// the reason, for a `local` Theater, a Session without a runner or recorded launch, or no
/// eligible Station.
pub fn adopter(policy: &OperationPolicy, request: &PlacementRequest,
               stations: &[StationFacts], session: &SessionRecord) -> Result<PlacementChoice, String>
pub const DEFAULT_LOST_AFTER_SECS: u64 = 45;              // Operation::lost_after_secs default
pub const FIRST_REATTACH_DELAY: Duration = 5 s;          // doubles per failed re-attach try
pub const MAX_REATTACH_DELAY: Duration = 60 s;
pub fn reattach_delay(failures: u32) -> Duration         // 5, 10, 20, 40, 60, 60, ... s
```

## Usage ledger and quarantine records (`ai6::directorates::registry::{usage, quarantine}`)

```rust
pub trait UsageStore {
    // FileRegistry: usage/<operation>/<mission>.json; additions are made under the Registry lock
    fn record_usage(&self, operation: Option<&str>, mission: &str, session: &str,
                    delta: &UsageTotals, now: i64) -> io::Result<MissionUsage>
    fn mission_usage(&self, operation: Option<&str>, mission: &str) -> io::Result<Option<MissionUsage>>
    fn operation_cost_on_day(&self, operation: Option<&str>, now: i64) -> io::Result<u64>  // micro-dollars, UTC day
}
pub trait QuarantineStore {
    // FileRegistry: quarantine/<agent>--<operation or _all>.json, plus quarantine/incidents/<agent>.json
    fn quarantine_agent(&self, record: &Quarantine) -> io::Result<Quarantine>   // an existing record is kept;
                                                                              // a new one counts Quarantined on the Agent's record
                                                                              // and removes its raised clearance grants there
    fn release_quarantine(&self, agent: &str, operation: Option<&str>) -> io::Result<bool>
    fn quarantines(&self) -> io::Result<Vec<Quarantine>>
    fn is_quarantined(&self, agent: &str, operation: Option<&str>) -> io::Result<bool>
    fn record_incident(&self, agent: &str, incident: Incident) -> io::Result<Vec<Incident>>  // history, 7 days kept
}
#[non_exhaustive] pub struct Quarantine { pub agent: String, pub operation: Option<String> /* None: every Operation */,
                                          pub reason: String, pub by: String, pub at: f64 }
impl Quarantine { pub fn new(agent, operation, reason, by, at) -> Self;
                  pub fn covers(&self, agent: &str, operation: Option<&str>) -> bool }
```

A quarantined sender's Cables are accepted but held (`CableState::Held`) until an
authorized release, whatever the receiver's inbound mode. `AgentQuery::excluding`
skips the named Agents; the Station passes the quarantined ones.

## Promotion and clearance grants (`ai6::directorates::m::promotion`, `ai6::directorates::registry::clearance`)

Autonomy is earned per Operation and signed by a person. M (the Headquarters Station, each
check-in round) reads each posted Agent's record; when it meets the next step it files a
green ink request of kind `clearance`, target `AGENT:CLEARANCE@OPERATION`, Mission
`promotion-AGENT`, Session `m`, idempotency key `TARGET#ROUND`. A person's approval (never a
class authorisation or an expiry) is spent first, then becomes a raising `ClearanceGrant` for
the Agent's configured model and clearance, written only if the Agent's grant is unchanged
since the round read it; so it raises once, and a lower written meanwhile stands. Storing a
quarantine removes the Agent's raises in that Operation (every Operation for a global one) in
the same step; the round takes back any left. A live lower grant (a person's) stops M filing
promotions for that Agent.

```rust
pub const CLEARANCE_KIND: &str = "clearance";
#[non_exhaustive] pub struct PromotionStep { pub missions: u64 /* 0 turns it off */,
    pub max_failure_rate: Option<f64> /* at most; (failed+blown)/(done+failed+blown) */,
    pub quiet_days: u64, pub quiet_on: Vec<AgentOutcome> }
impl PromotionStep { pub fn after(missions: u64) -> Self;
    /// counted: the current model's Sessions here (Missions, failure rate); incidents: the
    /// tallies the quiet period is judged on (every model here, and NO_OPERATION).
    pub fn shortfall(&self, counted: &AgentTally, incidents: &[&AgentTally], now: f64) -> Option<String> }
#[non_exhaustive] pub struct PromotionPolicy { pub field: PromotionStep /* desk -> field */,
                                               pub double_o: PromotionStep /* field -> 00 */ }
// Default: field 10 Missions, quiet 30 days on blown + quarantined;
//          00 25 Missions, max_failure_rate 0.1, quiet 60 days on every incident.
impl PromotionPolicy { pub fn due(&self, current: &str, ceiling: &str, counted: &AgentTally,
                                  incidents: &[&AgentTally], now: f64) -> Option<Promotion> }
#[non_exhaustive] pub struct Promotion { pub from: String, pub to: String,
    pub round: u64 /* done / missions: a refused step is asked again when it grows */, pub reason: String }
pub struct PromotionDefinition;   // [operation.ID.promotion.field] / ["00"]: missions,
                                  // max_failure_rate (0..=1), quiet_days, quiet_on = ["blown", ...]

#[non_exhaustive] pub struct ClearanceGrant { pub agent: String, pub operation: String,
    pub clearance: String, pub signer: String, pub at: f64, pub expires_at: Option<f64>,
    pub approval: Option<String> /* the green ink request that raised it; None: only lowers */,
    pub model: Option<String>, pub base: Option<String> /* a raise's occupant: configured model and
                                                          clearance it was signed for; base required */,
    pub reason: Option<String> }
impl ClearanceGrant {
    pub fn raise(agent, operation, clearance, approval, signer, at) -> Self;
    pub fn for_occupant(self, model: Option<String>, base) -> Self;   // required on a raise
    pub fn applies_to(&self, model: Option<&str>, configured: &str) -> bool; // a lower always; a raise
                                                                          // only for its occupant
    pub fn lower(agent, operation, clearance, signer, at) -> Self;
    pub fn expiring(self, at: f64) -> Self; pub fn because(self, reason) -> Self;
    pub fn raises(&self) -> bool; pub fn is_live(&self, now: f64) -> bool;
    pub fn validate(&self) -> io::Result<()>;   // Registry-key agent and operation, known clearance
}
pub trait ClearanceStore {
    // FileRegistry: clearance-grants/<operation>/<agent>.json; SQL: ai6_clearance_grants
    fn put_clearance_grant(&self, grant: &ClearanceGrant) -> io::Result<()>   // replaces; 10,000 at most
    fn replace_clearance_grant(&self, expected: Option<&ClearanceGrant>, grant: &ClearanceGrant)
        -> io::Result<bool>                    // compare-and-set under the lock or in one transaction
    fn remove_clearance_grant_if(&self, expected: &ClearanceGrant) -> io::Result<bool>
    fn clearance_grant(&self, operation: &str, agent: &str) -> io::Result<Option<ClearanceGrant>>
    fn clearance_grants(&self, operation: &str) -> io::Result<Vec<ClearanceGrant>>
    fn remove_clearance_grant(&self, operation: &str, agent: &str) -> io::Result<Option<ClearanceGrant>>
}
```

The Station works an Agent at its effective clearance (start, resume, green ink, class
authorisations, proposals, Agent records); a grant it cannot read counts as one holding it at
`desk`, and a raise counts only when its approval is M's request for exactly that Agent,
clearance and Operation, approved by a person, spent and not revoked. Each check-in carries
`agent_clearances`: `{OPERATION: {AGENT: {clearance, configured, aliases, unreadable?}}}`.

## Agent records (`ai6::directorates::registry::agent_records`)

Each Agent's track record, per Operation: counts, the latest time of each kind, counts by
the clearance it worked at, and the models that occupied the codename with what each did.
Counts only; Missions and audit stay the detail.

```rust
pub const NO_OPERATION: &str = "";   // Operation key: outside any Operation, and quarantines in every Operation
#[non_exhaustive] pub enum AgentOutcome { Done, Failed, Blown, Abandoned, Stalled, Lost, Quarantined,
                                          BudgetStop, GreenInkRejected }   // snake_case; as_str(), from_name()
impl AgentOutcome { pub const ALL: [Self; 9];
                    pub fn is_incident(self) -> bool         // not Done, Failed, Abandoned or Lost
                    pub fn is_session_outcome(self) -> bool  // Done, Failed, Blown, Abandoned, Stalled, Lost }
#[non_exhaustive] #[derive(Default)] pub struct AgentTally { pub done, failed, blown, abandoned, stalled,
    lost, quarantined, budget_stops, green_ink_rejected: u64, pub last: BTreeMap<AgentOutcome, f64 /* unix */> }
impl AgentTally { pub fn count(&self, o) -> u64; pub fn last(&self, o) -> Option<f64>;
                  pub fn latest(&self, outcomes: &[AgentOutcome]) -> Option<f64>;
                  pub fn failure_rate(&self) -> Option<f64> /* (failed+blown)/(done+failed+blown); lost not counted */ }
#[non_exhaustive] pub struct ModelTenure { pub model: Option<String> /* as launched; None: handler default */,
    pub from: f64, pub to: Option<f64> /* None: current */, pub tally: AgentTally /* its Sessions' outcomes */ }
#[non_exhaustive] #[derive(Default)] pub struct OperationRecord { pub tally: AgentTally,
    pub by_clearance: BTreeMap<String, AgentTally> /* 8 kept */, pub last_mission_at: Option<f64>,
    pub model_history: Vec<ModelTenure> /* 16 kept */ }
impl OperationRecord { pub fn tally_of_model(&self, model: Option<&str>) -> AgentTally /* the current
                       tenure's, if it is that model; else empty */ }
#[non_exhaustive] #[derive(Default)] pub struct AgentRecord { pub agent: String,
    pub by_operation: BTreeMap<String, OperationRecord> /* 128 kept */, pub counted: Vec<String> /* last 256 keys */ }
impl AgentRecord { pub fn new(agent) -> Self; pub fn in_operation(&self, op: &str) -> OperationRecord;
                   pub fn count(&mut self, report: &AgentOutcomeReport) -> io::Result<bool> /* false: key seen */ }
#[non_exhaustive] pub struct AgentOutcomeReport { pub agent: String, pub operation: Option<String>,
    pub outcome: AgentOutcome, pub key: String /* names the event, e.g. MISSION#ATTEMPT#OUTCOME */,
    pub clearance: Option<String>, pub model: Option<String> /* as launched */, pub at: f64 }
impl AgentOutcomeReport { pub fn new(agent, operation, outcome, key, at) -> Self;
                          pub fn at_clearance(self, c) -> Self; pub fn with_model(self, m: Option<String>) -> Self }
pub trait AgentRecordStore {
    // FileRegistry: agent-records/<agent>.json under the Registry lock; SQL: ai6_agent_records
    fn record_agent_outcome(&self, report: &AgentOutcomeReport) -> io::Result<AgentRecord>  // a seen key counts nothing
    fn agent_record(&self, agent: &str) -> io::Result<Option<AgentRecord>>
    fn agent_records(&self) -> io::Result<Vec<AgentRecord>>   // by codename; unreadable ones skipped
}
```

A Session outcome opens a new `ModelTenure` in its Operation when its launch model differs
from the current one; incidents do not. A record past 128 Operations, 8 clearances in one, or
1 MiB, is refused (`InvalidData`). Only the Station that holds a Mission's Session counts it:
each finished or stalled state (key `MISSION#ATTEMPT#OUTCOME`, model from the launch record),
a Session lost with its Station or runner as `lost` (`MISSION#ATTEMPT#lost`, never `failed`),
budget stops (`budget#MISSION#ATTEMPT#REVISION`) and refused green ink told to an Agent
(`green-ink#REQUEST`); the Registry counts each new quarantine. Chief of Staff review Missions
are not counted.

## Pool accounts (`ai6::directorates::registry::accounts`)

The shared state of every account of every pool: who uses it, its provider quota, until when
it is limited and the tokens used per day. One record per account (`accounts/<pool>/<account>.json`
in a `FileRegistry`, a row of `ai6_accounts` in a `SqlRegistry`), bounded to 256 KiB.

```rust
pub const UNKNOWN_USE_PERMILLE: u16 = 500;   // what an unreported account counts as
pub struct AccountRef { pub pool: String, pub account: String }   // Display "pool/account"; serde
impl AccountRef { pub fn new(pool, account) -> Self }
#[non_exhaustive] pub struct AccountClaim { pub station: String, pub mission: String, pub since: f64, pub expires: f64,
    pub operation: Option<String> }   // serde default None
#[non_exhaustive] pub struct QuotaWindow { pub name: String, pub used_permille: Option<u16>,
    pub resets_at: Option<f64>, pub status: Option<String>, pub observed_at: f64 }
impl QuotaWindow { pub fn new(name, observed_at: f64) -> Self; .used_permille(u16) .resets_at(f64) .status(s);
    pub fn use_at(&self, now: f64) -> u16 }   // 0 once reset; UNKNOWN_USE_PERMILLE when no figure
#[non_exhaustive] pub struct AccountProfile { pub handler: String, pub kind: String,
    pub operations: Vec<String>, pub priority: i64, pub max_concurrent: Option<usize>,
    pub labels: BTreeMap<String, String> }   // never a secret name or directory
impl AccountProfile { pub fn of(pool: &AccountPool, account: &str, operations: Vec<String>) -> Option<Self> }
#[non_exhaustive] pub struct AccountState { pub account: AccountRef,
    pub profiles: BTreeMap<String /* Station */, AccountProfile>,
    pub active: Vec<AccountClaim>, pub windows: Vec<QuotaWindow>, pub limited_until: Option<f64>,
    pub last_used: Option<f64>, pub daily_tokens: BTreeMap<String /* YYYY-MM-DD UTC */, u64> }
impl AccountState { pub fn new(AccountRef) -> Self;
    pub fn profile(&self) -> Option<AccountProfile>;   // first Station's settings, operations of every Station
    pub fn live_claims(&self, now) -> impl Iterator<Item = &AccountClaim>;
    pub fn is_limited(&self, now) -> bool; pub fn highest_use(&self, now) -> u16; pub fn tokens_on_day(&self, now: i64) -> u64 }
pub struct AccountCandidate { pub account: String, pub priority: i64, pub max_concurrent: Option<usize> }
pub struct AccountClaimRequest;   // ::new(&AccountPool, station, mission, ttl) .prefer(account) .excluding(account) .only(account)
                                  //   .operation(Option<String>) .in_use() (the first candidate, room or not)
impl AccountClaimRequest { pub fn pool(&self) -> &str; pub fn candidates(&self) -> &[AccountCandidate] }
pub fn choose_account<'a>(strategy: PoolStrategy, candidates: &'a [AccountCandidate],
    states: &BTreeMap<String, AccountState>, prefer: Option<&str>, now: f64) -> Option<&'a AccountCandidate>;

pub trait AccountStore {
    fn declare_account(&self, account: &AccountRef, station: &str, profile: &AccountProfile) -> io::Result<()>;  // that Station's profile
    fn withdraw_account(&self, account: &AccountRef, station: &str) -> io::Result<()>;  // drops that Station's profile only
    fn claim_account(&self, request: &AccountClaimRequest, now: f64) -> io::Result<Option<AccountState>>;
    fn release_account(&self, station: &str, mission: &str) -> io::Result<bool>;
    fn renew_account_claims(&self, station: &str, missions: &[String], ttl: Duration, now: f64) -> io::Result<()>;
    fn report_quota(&self, account: &AccountRef, windows: &[QuotaWindow]) -> io::Result<()>;   // replaces windows by name; 8 kept
    fn limit_account(&self, account: &AccountRef, until: f64) -> io::Result<()>;               // a later limit is kept
    fn clear_account_limit(&self, account: &AccountRef) -> io::Result<bool>;                   // whether it had one
    fn record_account_usage(&self, account: &AccountRef, tokens: u64, now: i64) -> io::Result<()>;  // 8 days kept
    fn accounts(&self) -> io::Result<Vec<AccountState>>;                                        // by pool, account
}
```

`claim_account` reads the candidates, drops lapsed claims and any earlier claim of the same
Station and Mission on any account (another pool's included), picks with `choose_account`
(or, `in_use`, takes the first candidate) and records the claim, all under the
Registry lock or in one SQL write transaction: two Stations racing for the last place of an
account get one winner. An account has room when it is not limited at `now` and has fewer
live claims than its `max_concurrent`; `prefer` wins whenever it has room. Then `Headroom`
takes the lowest `highest_use`, then fewest live claims, then the oldest `last_used`, then
the highest priority, then the name; `RoundRobin` the oldest `last_used`; `Priority` the
highest priority. `None` when no account has room. `renew_account_claims` extends the
Station's claims for the listed Missions to `now + ttl` and drops its others made before
`now`; a claim nobody
renews lapses, so a dark Station's accounts free up. Names are 1-64 letters, digits, `.`,
`_` or `-`, starting with a letter or digit.

The Station claims with a TTL of `DEFAULT_LEASE`, renews at each check-in (its held Missions
and those whose account is being handed over), releases with the Session's leases, records
quota from `AgentEvent::quota` and adds each usage delta's tokens. A limit is recorded until
the reset, at most the Station's horizon (8 days, `StationConfig::with_account_limit_horizon`).
A re-attached Session's recorded account is claimed again `in_use`. Each Station declares its
configured accounts at start with the Operations of its roster that may draw from each pool;
there is no reload, so a changed roster or loadout is declared at the next start.

## Green ink (`ai6::directorates::m::approvals`, `ai6::directorates::registry::approvals`)

A person's approval of one irreversible step. Policy:

```rust
pub const BUILTIN_KINDS: [&str; 6];        // push, merge, release, delete, clearance, safe-house-delete
pub const SAFE_HOUSE_DELETE: &str;         // "safe-house-delete": covers only exactly the same target
pub const DEFAULT_PROTECTED: [&str; 4];    // refs/heads/{main,master,release/*}, refs/tags/*
pub const DEFAULT_REQUEST_TTL: Duration;   // 24 h: an undecided request then expires as refused
pub const DEFAULT_APPROVAL_TTL: Duration;  // 1 h: an approval whose signer chose no lifetime
pub const GREEN_INK_REASON: &str;          // "green-ink", the blocked_reason of a waiting Mission
#[non_exhaustive] pub struct ActionScope { pub kind: String /* 1-32 of a-z0-9-_ */, pub target: String /* 1-256, no controls */ }
impl ActionScope {
    pub fn try_new(kind, target) -> io::Result<Self>;   // InvalidInput out of bounds
    pub fn pushed_ref(&self) -> String;                 // "[REMOTE:]REF"; bare name -> refs/heads/NAME
    pub fn remote(&self) -> Option<&str>;               // the REMOTE before the last ':'
    pub fn covers(&self, attempted: &ActionScope) -> bool;   // same kind; push/delete: same ref and
                                                        // remote (an unnamed remote matches any);
                                                        // else target equal ignoring case, '#', spacing
}
#[non_exhaustive] pub enum Requirement { NotRequired, GreenInk, Refused }
#[non_exhaustive] pub enum GreenInkPreset { Strict /* default */, Licensed }   // serde lowercase
pub struct GreenInkPolicy;   // Default = Strict
impl GreenInkPolicy {
    pub fn preset(preset: GreenInkPreset) -> Self;
    pub fn requirement(&self, clearance: &str, scope: &ActionScope) -> Requirement;
    pub fn is_protected(&self, reference: &str) -> bool;   // full ref, glob match; any reference
                                                         // with `*`, `?` or `[` is protected
    pub fn kinds(&self) / requesters(&self) -> impl Iterator<Item = &str>;
    pub fn protected(&self) -> &[String];
    pub fn request_ttl(&self) -> Duration;
    pub fn weakenings(&self) -> Vec<String>;   // every setting looser than strict
}
pub struct GreenInkDefinition;   // [operation.ID.green_ink]; resolve() -> Result<GreenInkPolicy, String>
pub fn clearance_at_least(have: &str, min: &str) -> bool;   // desk < field < 00; unknown is false
#[non_exhaustive] #[derive(Default, Clone, PartialEq, Eq, Debug)]
pub struct GitContext { pub dirs: Vec<String> /* cd, git -C, env -C, sudo -D, in order */,
    pub git_dir: Option<String> /* --git-dir, GIT_DIR */, pub work_tree: Option<String> }
pub trait Repository {   // what classify asks about the repository a command acts on
    fn full_name(&self, at: &GitContext, name: &str) -> Result<Option<String>, String>;   // rev-parse --symbolic-full-name
    fn config(&self, at: &GitContext, key: &str) -> Result<Vec<String>, String>;         // config --get-all; [] unset
}
pub fn classify(command: &str, repo: &dyn Repository)
    -> Result<Vec<ActionScope>, String>;   // the governed steps a shell command takes
```

`classify` reads a command line as a shell splits it (quotes, `;` `&&` `||` `|`, newlines,
parentheses, backticks) and, for each simple command, drops shell keywords (`if then elif else
while until do { !`), assignments (`GIT_DIR`, `GIT_WORK_TREE`, `GH_REPO` are used) and
wrappers with their own options (`sudo`/`doas` incl. `-D`, `env` incl. `-C` and `-S LINE`,
`timeout` and its duration, `nice`, `ionice`, `stdbuf`, `xargs`, `command`, `exec`,
`nohup`, `setsid`, `time`); `command -v` runs nothing. It follows `cd`/`pushd`/`export` for
later commands and reads `sh -c`, `bash -lc`, `pwsh -Command`, `cmd /c` and `eval`. It names,
deduplicated in order, always with the remote:
- each ref a `git push` updates (`push`, `REMOTE:refs/...`; `--force`, `+ref`, `-u` change
  nothing; `--all`/`--branches` `refs/heads/*`, `--tags` and `--follow-tags`/`push.followTags`
  `refs/tags/*` beside any refspecs, `--mirror` and `remote.R.mirror` `refs/*`; `tag NAME`,
  `heads/X`, `tags/X`, `remotes/X` as full refs; a bare destination takes its source's
  namespace, else the repository's) and each ref it removes (`--delete`, `-d`, `:ref`:
  `delete`). With no refspec it follows `remote.R.push`, then `push.default` (`simple` and
  `current` the current branch, `upstream`/`tracking` `branch.B.merge`, `matching`
  `refs/heads/*`, `nothing` none); with no remote, `branch.B.pushRemote`,
  `remote.pushDefault`, `branch.B.remote`, then `origin`. `-c KEY=VALUE` on the command wins.
  The last `--dry-run`/`--no-dry-run` and `-n` decide; a dry run takes no step. A git alias
  is expanded once (`!` aliases are read as commands).
- `gh pr merge N|#N|URL` (`merge`, `PR N`, or `PR OWNER/REPO#N` with `-R`/`--repo`,
  `GH_REPO` or a URL) unless `--disable-auto`; `gh release create TAG` (`release`,
  `tag TAG`); `gh api` that merges (`PUT|POST …/pulls/N/merge`, `-f` implying POST).
`Err` is a sentence for the Agent, and the gate holds the command: a ref the repository cannot
name (an unknown bare name, a detached `HEAD`), a repository it cannot read, a merge or release
naming no pull request or tag, a changing `gh api` call on releases, refs, tags, merges,
contents or branches (or a GraphQL mutation, or deleting a repository), arguments from
`xargs`, `git send-pack`, submodule pushes (`--recurse-submodules=on-demand|only`),
configuration through `--config-env` or `GIT_CONFIG_*`, and any other command that names
`git … push` or `gh … merge|release|api` (`find -exec`, `parallel`, `Start-Process`). Not
seen: arguments built at run time, `gh` aliases, other programs that push.

`strict` asks for every governed kind at every clearance and refuses requests from `desk`;
`licensed` also lets `00` push and merge unattended. A push needs green ink only to a
protected ref. Keys: `preset`, `kinds` (added to the built-ins), `requesters`
(clearances that may ask), `unattended = { CLEARANCE = [KIND, ..] }` (replaces the
preset's list per clearance named), `protected` (ref globs, replacing the default) and
`request_ttl_secs`. An invalid value fails the roster load naming the key; a Station logs
each weakening at start.

Records, under the Registry lock: `approvals/pending/OP/ID.json`,
`approvals/decided/OP/ID.json`, `approvals/missions/OP/MISSION.json` (a Mission's request
IDs, at most 64), `class-authorizations/OP/ID.json`.

```rust
pub const EXPIRY_SIGNER: &str;   // "expiry"
#[non_exhaustive] pub struct ApprovalRequest { pub id: String /* green-ink-<sha256> */, pub operation: String,
    pub mission: MissionId, pub session: SessionId, pub agent: String, pub scope: ActionScope,
    pub reason: String /* 1-2048, newlines allowed; never audited */, pub requested_at: f64, pub expires_at: f64,
    pub proposed_role: Option<String> /* serde default: the role a Mission proposal asks for */ }
impl ApprovalRequest {
    pub fn try_new(operation, mission, session, agent, scope, reason, idempotency_key: &str) -> io::Result<Self>;
    pub fn expiring(self, requested_at: f64, ttl: Duration) -> Self;   // required before storing
    pub fn proposing_role(self, Option<String>) -> Self;
}
#[non_exhaustive] pub enum Decision { Approved, Rejected, Expired }   // serde snake_case; as_str()
pub struct Revocation { pub by: String, pub at: f64 }
#[non_exhaustive] pub struct Approval { pub request: ApprovalRequest, pub decision: Decision,
    pub signer: String /* person, "class:ID" or "expiry" */, pub at: f64, pub expires_at: Option<f64>,
    pub note: Option<String>, pub revoked: Option<Revocation>, pub notified_at: Option<f64>,
    pub spent_at: Option<f64> /* a green ink credential was leased against it */ }
impl Approval { pub fn is_live(&self, now: f64) -> bool }   // approved, not revoked, not expired
#[non_exhaustive] pub struct Ruling { pub decision, pub signer, pub at, pub expires_at, pub note }
impl Ruling { pub fn approve(signer, at, expires_at) -> Self; pub fn reject(signer, at, note: Option<String>) -> Self;
              pub fn with_note(self, note) -> Self }
pub enum ApprovalStatus { Pending(ApprovalRequest), Decided(Approval) }   // request(), name()
#[non_exhaustive] pub struct ClassAuthorization { pub id: String /* class-<uuid> */, pub operation: String,
    pub kinds: Vec<String>, pub target_pattern: String /* glob */, pub agents: Vec<String> /* "*" = any */,
    pub roles: Vec<String>, pub clearance_min: String /* "field" */, pub max_uses: Option<u32>, pub uses: u32,
    pub expires_at: f64, pub signer: String, pub created_at: f64, pub revoked: Option<Revocation>,
    pub proposed_roles: Vec<String> /* serde default; kind proposal: roles a proposal may ask for besides its parent's */ }
impl ClassAuthorization {
    pub fn new(operation, kinds, target_pattern, signer, created_at, expires_at) -> Self;
    pub fn for_agents(self, Vec<String>) / for_roles(self, Vec<String>) / for_proposed_roles(self, Vec<String>)
        / with_clearance_min(self, c) / with_max_uses(self, u32) -> Self;
    pub fn is_live(&self, now: f64) -> bool;
    pub fn covers(&self, agent: &str, role: Option<&str>, clearance: &str, scope: &ActionScope) -> bool;
    /// No proposed role, or the parent Mission's `role`, or one in proposed_roles.
    pub fn allows_proposed_role(&self, request: &ApprovalRequest, role: Option<&str>) -> bool;
    pub fn validate(&self) -> io::Result<()>;   // needs a kind, an Agent or role, a glob, an expiry after the grant
}
pub trait ApprovalStore {
    fn request_approval(&self, request: &ApprovalRequest) -> io::Result<ApprovalStatus>;   // same ID returns the record
    fn approval(&self, operation, id, now: f64) -> io::Result<Option<ApprovalStatus>>;
    fn pending_approvals(&self, operation, now: f64) -> io::Result<Vec<ApprovalRequest>>;   // oldest first
    fn mission_approvals(&self, operation, mission, now: f64) -> io::Result<Vec<ApprovalStatus>>;
    fn live_approval(&self, operation, mission, scope: &ActionScope, now: f64) -> io::Result<Option<Approval>>;   // one that covers scope
    fn decide_approval(&self, operation, id, ruling: &Ruling) -> io::Result<Approval>;
    fn revoke_approval(&self, operation, id, by, at: f64) -> io::Result<Approval>;
    fn mark_approval_notified(&self, operation, id, at: f64) -> io::Result<()>;
    fn spend_approval(&self, operation, id, at: f64) -> io::Result<bool>;   // true once; false if spent or not live
    fn approve_by_class(&self, request, role: Option<&str>, clearance: &str, now: f64) -> io::Result<Option<Approval>>;
    fn put_class_authorization(&self, record: &ClassAuthorization) -> io::Result<()>;
    fn class_authorizations(&self, operation) -> io::Result<Vec<ClassAuthorization>>;   // oldest first
    fn revoke_class_authorization(&self, operation, id, by, at: f64) -> io::Result<ClassAuthorization>;
}
```

Every read that takes `now` first records pending requests past their expiry as `Expired`.
`decide_approval` returns `NotFound` for an unknown request, `AlreadyExists` when it was
decided otherwise or expired, and `InvalidInput` for an approval without a later expiry or
a refusal with one; the same decision again returns the record. `approve_by_class` counts
one use of the first live covering class authorisation (oldest first) and stores the
approval as already told; it returns `None` for a request already stored. Two things hold a
step taken without green ink: the harness gate (below) and leased credentials (Station
daemon, Credentials).

## Harness gate (`ai6::directorates::section_00::gate`)

```rust
pub const GATE_PROGRAM_ENV: &str;   // "AI6_GREEN_INK_GATE": the gate program, from the Agent's roster
                                    // env only (a Station drops it from start/resume requests and
                                    // logs a roster value as a weakening); unset, the ai6 executable
                                    // hosting the handler (Station or field runner)
pub const CABLE_URL_ENV: &str;      // "AI6_CABLE_MCP_URL"
pub const CABLE_TOKEN_ENV: &str;    // "AI6_CABLE_MCP_TOKEN"
#[derive(Debug, Clone, PartialEq, Eq)] pub enum GateVerdict { Allow, Hold(String /* for the Agent */) }
pub type Answers = Vec<(String /* status */, Option<String> /* why, for a refusal */)>;
pub fn decide(classified: Result<Vec<ActionScope>, String>,
              check: impl FnOnce(&[ActionScope]) -> Result<Answers, String>) -> GateVerdict;
pub fn deny(reason: &str) -> String;   // the PreToolUse deny decision
pub type GitOutput = (bool /* success */, String /* stdout */);
pub struct GitRepository<F>;   // a Repository read by running git through F (git's arguments)
impl<F: Fn(&[String]) -> Result<GitOutput, String>> GitRepository<F> { pub fn new(base: Option<String>, run: F) -> Self }
// feature `gate`:
pub struct GateClient;   // the Station's check_approval tool over the Session's Cable endpoint
impl GateClient {
    pub fn from_environment(environment: &BTreeMap<String, String>) -> Option<Self>;
    pub fn from_process_environment(url: Option<String>) -> Option<Self>;   // url, else AI6_CABLE_MCP_URL
    pub fn check(&self, steps: &[ActionScope]) -> Result<Answers, String>;
}
pub fn gate(command: &str, cwd: Option<&Path>, client: Option<&GateClient>) -> GateVerdict;
pub fn local_git(args: &[String]) -> Result<GitOutput, String>;   // git in a process here
pub fn pre_tool_use(input: &str, client: Option<&GateClient>) -> Option<String>;   // Claude Code hook output
```

`decide` allows a command that takes no governed step, or whose every step the Station answers
`approved` or `not_required`. Anything else holds it: `needs_approval` (the text names the
`kind` and `target` to pass to `request_approval`), `refused`, a step `classify` cannot name,
a check that fails or answers for other steps. `GitRepository` follows a `GitContext` from
`base` (`git -C DIR`, `--git-dir=`, `--work-tree=`); a directory with `$`, `` ` ``, `%`, a
leading `~`, or `-` cannot be followed, so the command is held. A name that is no ref is
`Ok(None)`, a directory that is no repository an error. `gate` is `decide` over a
`GitRepository` with `local_git` from `cwd` and `client`; without a client a governed step is
held. `GateClient::check` speaks MCP 2025-06-18 over HTTP (initialize,
initialized, `tools/call check_approval`, then `DELETE` of the MCP session), reading JSON or
server-sent events, 30 s per request. `pre_tool_use` takes the `PreToolUse` hook input
(`tool_input.command`, `cwd`) and returns
`{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":…}}`
for a held command and for input it cannot read or that names no command, `None` otherwise
(print nothing). `ai6 green-ink gate [--cable-url URL]` is that hook: it reads standard input,
asks the endpoint at `--cable-url` (else `AI6_CABLE_MCP_URL`) with `AI6_CABLE_MCP_TOKEN`,
prints the decision (a deny when stdin cannot be read or deciding fails) and always exits 0.

## Sources (`ai6::directorates::registry::sources`)

A Source is a Guatiao kind (`ai6_source`) for a tracker or CI system. All models are
Guatiao values; the kind grows by appending methods with defaults.

```rust
pub struct SourceConfig { pub settings_toml: String }  // the Operation's `sources.<provider>` table as TOML
pub struct SourceItem {
    pub id: String,            // provider-local, stable across polls, e.g. "owner/repo#12"
    pub title: String,         // untrusted
    pub body: String,          // untrusted
    pub url: Option<String>,
    pub labels: Vec<String>,
    pub author: Option<String>,
    pub updated: String,       // ISO 8601, as the provider spells it
}
pub struct SourcePoll { pub cursor: Option<String> }
pub struct SourceBatch { pub items: Vec<SourceItem>, pub cursor: Option<String> }
pub struct CxReport {
    pub item_id: String,
    pub mission_id: String,
    pub outcome: String,       // "done", "failed", ...
    pub body: String,          // already worded for the Operation's cover policy
    pub branch: Option<String>,
}
#[guatiao::kind(name = "ai6_source")]
pub trait Source: Send + Sync {
    fn poll(&self, request: SourcePoll) -> Result<SourceBatch, ProviderError>;
    fn report(&self, report: CxReport) -> Result<(), ProviderError>;
}
pub const SOURCE_BRIEF_PREFIX: &str = "source:";
pub const MAX_ITEM_TEXT_BYTES: usize; // 64 KiB of item text kept per brief
pub enum Ingest { Queued, Updated, Unchanged, InFlight, Finished }
pub struct FinishedSourceBrief { pub brief: Brief, pub outcome: serde_json::Value, pub ok: bool }
pub fn source_brief(operation: &str, provider: &str, item: &SourceItem) -> Brief;  // no provenance
pub fn reportable(brief: &Brief, provider: &str) -> bool;  // a `source` provenance naming provider and item

SourceStore::source_cursor(operation: &str, provider: &str) -> io::Result<Option<String>>
SourceStore::set_source_cursor(operation: &str, provider: &str, cursor: &str) -> io::Result<()>
SourceStore::ingest_source_item(operation: &str, provider: &str, item: &SourceItem, received_by: &str) -> io::Result<Ingest>
SourceStore::unreported_source_briefs() -> io::Result<Vec<FinishedSourceBrief>>
SourceStore::mark_source_reported(finished: &FinishedSourceBrief, reported: bool) -> io::Result<()>
```

A provider receives its settings as `SourceConfig::settings_toml` and validates them itself;
the Station removes its own `interval_secs` key first. `poll` and `report` block on the
network and are called off the async runtime. A provider returns an error for a rate limit
or a refused credential; the Station polls again at its next interval.

`source_brief` keys the brief `source_<provider>--<id>` (the brief's `source` is
`source:<provider>`), generates its title, and places the item's title, author, labels and
text between fenced markers introduced as untrusted data; the fence is lengthened until the
text cannot contain it. `ingest_source_item` queues the brief, updates a brief still in
`new/` when its fingerprint (title, text, labels, `updated`) changed, and leaves a claimed
(`InFlight`) or finished brief alone. Cursors are stored under `sources/`.
`unreported_source_briefs` lists finished source briefs without a report mark (at most five
attempts each); `mark_source_reported` records an attempt.

On the Station, the Headquarters holder runs a source desk: per Operation and provider it
polls on `interval_secs` (default 60 s, minimum 10 s), ingests the batch, then stores the
cursor; every 15 s it posts a `CxReport` for each unreported finished source brief. The
finished outcome carries `state`, `mission_id`, `session_ids`, `agent` and `branch`. Report
wording follows `Cover::disclosure()`: `Hidden` (covert), `Assisted` (partial) or `Full`
(overt).

## Sleepers (`ai6::directorates::registry::sleepers`)

An Operation's `sleepers.<name>` tables are standing orders. Names use letters, digits,
`-` and `_`; the roster refuses an invalid definition when it loads.

```rust
pub struct Sleeper {
    pub schedule: String,        // five-field cron in UTC
    pub title: String,
    pub prompt: String,          // operator-written; not fenced
    pub agent: Option<String>,
    pub role: Option<String>,
    pub priority: i64,
    pub stations: Vec<String>,   // narrows its briefs, like Brief.stations; empty adds no limit
    pub each_station: bool,      // one round per eligible Station per window (default false)
}
impl Sleeper {
    pub fn validate(&self) -> Result<(), String>;   // also refuses a `stations` entry that is no Station ID
    pub fn fingerprint(&self) -> String;   // SHA-256 over every field; any edit changes it
    pub fn due_window(&self, now: i64, authorized_at: i64) -> Option<i64>;
}
pub struct SleeperAuthorization { pub fingerprint: String, pub authorized_by: String, pub at: i64 }
pub enum Fire { Queued, AlreadyFired }
pub const SLEEPER_BRIEF_SOURCE: &str = "sleeper";
pub const MAX_WINDOW_AGE_SECS: i64; // 86_400

pub struct Cron;
impl Cron {
    pub fn parse(expression: &str) -> Result<Cron, CronError>;
    pub fn latest_at_or_before(&self, now: i64) -> Option<i64>; // Unix seconds, looks back 366 days
}
pub fn window_label(window: i64) -> String; // "20261025T0300Z"
/// The brief of one window; with `station`, that Station's round, narrowed to it.
pub fn sleeper_brief(operation: &str, name: &str, sleeper: &Sleeper, window: i64, station: Option<&str>) -> Brief;

SleeperStore::authorize_sleeper(operation: &str, name: &str, sleeper: &Sleeper, authorized_by: &str, at: i64) -> io::Result<()>
SleeperStore::sleeper_authorization(operation: &str, name: &str) -> io::Result<Option<SleeperAuthorization>>
SleeperStore::revoke_sleeper(operation: &str, name: &str) -> io::Result<bool>
SleeperStore::fire_sleeper(operation: &str, name: &str, sleeper: &Sleeper, round: (i64, Option<&str>), received_by: &str) -> io::Result<Fire>  // (window, station)
```

Cron fields take `*`, numbers, `a-b`, `,` lists and `/step`; day-of-week is 0-7 (0 and 7 are
Sunday); when both day fields are restricted a day matches if either does. A schedule that
never matches within a year (`0 0 30 2 *`) parses but never fires.

`fire_sleeper` submits a brief keyed `sleeper--<operation>.<name>.<window>` (with a Station,
`...<window>.<station>`, the ID made key-safe with a digest when it had to change) and reports
`AlreadyFired` when that key exists in `new`, `claimed`, `done` or `failed`, which is what
makes a window, or one Station's round in it, fire once across Stations. A brief's
`stations` is the sleeper's, or `[station]` for a round; its data's `sleeper` object names
`operation`, `name`, `window` and, for a round, `station`. For an `each_station` sleeper the
Headquarters holder fires one round for each Station whose last check-in placement accepts
for the Operation and the sleeper's Agent or role, within the sleeper's `stations`; load,
resources and quarantines are ignored there (they only delay a brief), and a Station dark
past `GONE_DARK_AFTER` gets no round until it checks in again. Authorizations are stored under `sleepers/`. The
Headquarters holder fires only sleepers whose stored fingerprint equals the current
definition, using windows after the approval time and no more than a day old.

## Station logbooks (`ai6::directorates::registry::logbook`)

Each Station's logbook keeps the newest `LOGBOOK_LENGTH` (20) reports its Head of Station
filed, oldest first, under `logbook/<station>.json` (SQL table `ai6_logbooks`). A round is
a Mission of role `head-of-station` whose brief, as this Station leased it, is this Station's
round of an authorized `each_station` Sleeper: the Registry kept `Origin::Sleeper` provenance
for it, its `data.sleeper` names this Station, the Mission's Operation and the Sleeper, that
Sleeper still exists with `each_station`, role `head-of-station` and the fingerprint the
provenance names, and its stored authorization has that fingerprint. The Station starts a round with the logbook as fenced context, every line
quoted (`> `), and marks its Session exposed (`STATION_LOGBOOK`) when the logbook holds reports.
Universal Exports refuses a brief whose role `is_head_of_station_role`, and a Mission proposal
asking for one is refused at the Cable endpoint.

```rust
pub const HEAD_OF_STATION_ROLE: &str = "head-of-station";
pub fn is_head_of_station_role(role: &str) -> bool;   // the role or one beneath it ("head-of-station/...")
pub const HEAD_OF_STATION_CHANNEL: &str = "heads-of-station";
pub const LOGBOOK_LENGTH: usize = 20;
pub const MAX_SUMMARY_BYTES: usize = 4096;
pub const MAX_FINDINGS: usize = 32;
pub const MAX_FINDING_BYTES: usize = 1024;
#[non_exhaustive] pub enum StationFindingKind { Disk, Theater, SafeHouse, Provider, Other } // kebab-case; as_str, FromStr, Display
#[non_exhaustive] pub enum Severity { Info, Warning, Critical }                     // lowercase; ordered
#[non_exhaustive] pub struct StationFinding { pub kind: StationFindingKind, pub severity: Severity, pub text: String }
impl StationFinding { pub fn new(kind, severity, text) -> Self; }
#[non_exhaustive] pub struct StationReport {
    pub station: String, pub at: i64, pub summary: String, pub findings: Vec<StationFinding>,
    pub mission: Option<String>, pub agent: Option<String>,
}
impl StationReport {
    pub fn new(station, summary, at: i64) -> Self;
    pub fn with_findings(self, findings: impl IntoIterator<Item = StationFinding>) -> Self;
    pub fn filed_by(self, mission, agent) -> Self;
    pub fn worst(&self) -> Option<Severity>;
    pub fn validate(&self) -> io::Result<()>;   // InvalidInput: Station ID, 1..=4096-byte summary, <= 32 findings of 1..=1024 bytes
}
LogbookStore::file_station_report(report: &StationReport) -> io::Result<()>   // drops the oldest past 20
LogbookStore::logbook(station: &str) -> io::Result<Vec<StationReport>>         // oldest first; empty when none
```

## Vetting (`ai6::directorates::m::vetting`, `ai6::directorates::registry::vetting`)

M's checks on a brief before it is placed. `vet` is pure; the Headquarters holder runs it on
each queued brief not yet judged, before assignment.

```rust
pub const DEFAULT_MAX_BRIEF_BYTES: usize = 65_536;
pub const DEFAULT_MAX_PER_HOUR: u32 = 20;              // untrusted briefs passed per Operation
pub const DEFAULT_MAX_PER_SOURCE_PER_HOUR: u32 = 10;   // per Brief `source`
pub const CHIEF_OF_STAFF_ROLE: &str = "chief-of-staff";
pub const PROPOSAL_SOURCE: &str = "ai6-agent-proposal";

#[non_exhaustive] pub enum FindingKind { Malformed, Duplicate, Rate, Budget, Screening, Review }
                  // serde snake_case; ::ALL; .as_str()
#[non_exhaustive] pub struct Finding { pub kind: FindingKind, pub detail: String /* ≤256 B, never brief text */, pub related: Option<String> }
impl Finding { pub fn new(kind, detail) -> Self; pub fn related(self, mission_or_key) -> Self; }
#[non_exhaustive] pub enum VettingVerdict { Pass, Hold, Refuse }   // serde snake_case
#[non_exhaustive] pub struct Vetting { pub verdict: VettingVerdict, pub findings: Vec<Finding> /* ≤16 */ }
impl Vetting {
    pub fn pass() -> Self;
    pub fn from_findings(findings: Vec<Finding>) -> Self;  // any Malformed → Refuse; any → Hold; none → Pass
    pub fn kinds(&self) -> BTreeSet<FindingKind>;
    pub fn summary(&self) -> String;                       // "duplicate,screening"
}
#[non_exhaustive] pub enum VettingScope { Untrusted /* default */, All, Off }  // serde lowercase
#[non_exhaustive]
pub struct VettingPolicy {   // Default: every check on
    pub enabled: bool,                         // false: everything passes
    pub max_brief_bytes: usize,                // Source brief: its quoted item text; else title + body
    pub max_per_hour: Option<u32>,             // None: no limit
    pub max_per_source_per_hour: Option<u32>,
    pub duplicates: bool,                      // same normalized URL or title as known work
    pub budget: bool,                          // day spend ≥ operation_cost_micro_usd_per_day
    pub screening: VettingScope,               // hidden characters, fence lines, phrases (below)
    pub phrases: Vec<String>,                  // added to the built-in manipulation patterns
    pub html_comments: bool,                   // screened text with an HTML comment is held
    pub review: VettingScope,                  // briefs a qualified chief-of-staff Agent reviews
    pub rate_hold_secs: Option<u64>,           // 24 h: a rate-only hold returns to the queue; None keeps it
    pub max_proposal_reviews_per_hour: Option<u32>,             // 20; past it a proposal is held (rate)
    pub max_proposal_reviews_per_session_per_hour: Option<u32>, // 5, per proposing Session
}
impl VettingPolicy { pub fn off() -> Self; pub fn weakenings(&self) -> Vec<String>; }
pub struct VettingDefinition;   // the `[operation.ID.vetting]` table; .resolve() -> Result<VettingPolicy, String>
                                // a rate limit of 0 is no limit; `vetting = "off"` (or "on") in TOML
#[non_exhaustive] pub struct KnownWork { pub reference: String, pub title: String, pub url: Option<String> }
impl KnownWork { pub fn of(brief: &Brief, reference: impl Into<String>) -> Self; }
#[non_exhaustive] #[derive(Default)]
pub struct VettingContext {
    pub untrusted: bool,
    pub work: Vec<KnownWork>,                  // queued ahead, held, claimed
    pub passed_last_hour: u32,
    pub passed_from_source_last_hour: u32,
    pub day_spend_micro_usd: Option<u64>,      // None: unread, never holds
    pub day_limit_micro_usd: Option<u64>,
    pub cleared: BTreeSet<FindingKind>,        // not reported again
}
/// `source:` prefix or PROPOSAL_SOURCE, or a provenance other than Operator, Sleeper or
/// Review: a registry drop, or none (missing, or the record changed since it was sealed).
pub fn is_untrusted(brief: &Brief) -> bool;
pub fn work_title(brief: &Brief) -> String;    // a Source brief's own item title, else the title
pub fn vet(brief: &Brief, policy: &VettingPolicy, context: &VettingContext) -> Vetting;
pub fn content_digest(brief: &Brief) -> String; // bona_fides::record_digest: what clearances bind to
```

Rate and screening apply to untrusted briefs only; size, duplicates and budget to every brief.
Screening finds: tag characters (U+E0000..E007F) outside a subdivision flag (U+1F3F4, 1–8 tag
letters or digits, U+E007F); bidi overrides (U+202D, U+202E) and embeddings or isolates left
unbalanced on their line; runs of two or more zero-width characters, variation selectors
(but U+FE0E, U+FE0F), fillers, soft hyphens or tags; any filler or invisible operator
(U+180E, U+2061..2064, U+115F, U+1160, U+3164, U+FFA0); HTML comments outside the quoted
item (`html_comments`). Lone direction marks are not findings. A Source brief's quoted text
counts two genuine fence lines; any other line matching `^\s*={3,}\s*(BEGIN|END)\b` is a
forged one. Phrases are matched on lowercase words with invisible characters removed, so
`ig\u{200B}nore previous instructions` still matches.

The Chief of Staff review, for briefs that pass the rules when the Operation has a qualified
`chief-of-staff` Agent and `review` applies:

```rust
pub const REVIEW_SOURCE: &str = "ai6-vetting";       // review briefs; never vetted themselves
pub const REVIEW_DATA_KEY: &str = "vetting_review";  // Brief.data key holding the ReviewTarget
#[non_exhaustive] pub enum ReviewTarget {            // serde snake_case, deny_unknown_fields
    Brief { key: String, digest: String /* content_digest when queued */ },
    Proposal { id: String },
}
impl ReviewTarget {
    pub fn brief(brief: &Brief) -> Self;             // the brief as it reads now
    pub fn review_id(&self) -> String;               // "review-" + 32 hex of SHA-256: one review per content
    pub fn review_key(&self) -> String;              // "ai6-vetting--" + review_id
}
pub fn is_review(brief: &Brief) -> bool;             // REVIEW_SOURCE and an Origin::Review provenance
pub fn review_target(brief: &Brief) -> Option<ReviewTarget>;  // None unless is_review
/// role chief-of-staff, priority of the reviewed brief, data: target + exposure ["brief:source"],
/// provenance Origin::Review (received_by, received_at; reference = review_id);
/// the brief and up to 50 other work titles quoted between markers neither contains.
pub fn review_brief(reviewed: &Brief, target: &ReviewTarget, operation: &str, purpose: &str,
                    work: &[KnownWork], received: (&str, i64)) -> Brief;
/// The JSON objects shaped {"verdict", "findings"?} in the final message's last 16 KiB: any
/// "hold" → Review findings "KIND: detail" (duplicate|scope|manipulation); exactly one
/// "clear" with no findings and only whitespace after it → pass; anything else (none, several,
/// another verdict, text after) → hold with "review-error: ...". Never admits on a bad answer.
pub fn read_review(text: &str) -> Vetting;
```

The review Mission's ID is its brief key `ai6-vetting--review-…`. Its Station starts it only
on its own host, with clearance `desk`, `AgentStart::answer_only`, no Cable MCP tools (so no
Cables, proposals or green ink), no credentials issued (no loadout, no pool account), only the
model-credential and path variables of its Agent's environment (and
`AI6_CODEX_TOOL_CHECK`), no project settings, and a
new 0700 directory `ai6-review-*` under the temporary directory, removed when the review
ends. Its `completed` event text is read with `read_review` once the Mission's role is
`chief-of-staff`, its brief `is_review` and its ID matches its target's: clear →
`clear_brief(key, digest, [Review], "review:AGENT")`; otherwise `hold_brief` when the queued
brief still has the reviewed digest (an edited brief gets a review of its own). A review that
ends without an applied answer holds what it reviewed with `review-error`; with the role
configured and no qualified Agent available, a brief is held with `review-unavailable`. A
review Mission does not start or resume when its Agent's environment, as the Operation's
own `.ai6/agents.toml` refines it, sets `PATH`, `CODEX_HOME`, any `*_BASE_URL` or
`AI6_CODEX_TOOL_CHECK` differently from the global roster (names without case; added,
changed or dropped): the configuration error names the Agent and the variables.

```rust
pub const MAX_HELD_PER_OPERATION: usize = 1_000;  // past it a brief stays queued
pub const MAX_HELD_BRIEFS: usize = 10_000;        // records one listing reads, across Operations
pub const MAX_HELD_RESULTS: usize = 500;
#[non_exhaustive] pub struct HeldBrief { pub operation: String, pub brief: Brief, pub vetting: Vetting, pub held_at: f64 }
impl HeldBrief { pub fn held_only_for_rate(&self) -> bool; }
#[non_exhaustive] pub struct Clearance { pub kinds: BTreeSet<FindingKind>, pub digest: String, pub by: String, pub at: f64 }
impl Clearance { pub fn kinds_for(&self, brief: &Brief) -> BTreeSet<FindingKind>; } // empty unless digest matches

VettingStore::hold_brief(operation, key, vetting: &Vetting, at: f64) -> io::Result<Option<HeldBrief>> // None: not queued
VettingStore::held_briefs(operation, limit /* 1..=500 */) -> io::Result<Vec<HeldBrief>>         // oldest first
VettingStore::release_held(operation, key, by, at) -> io::Result<Brief>   // again: same brief; AlreadyExists once claimed/finished
VettingStore::refuse_held(operation, key, by, reason /* ≤1024 B */, at) -> io::Result<Brief>  // idem
VettingStore::requeue_held(operation, key) -> io::Result<bool>          // back to new/ with no clearance
VettingStore::clearance(key) -> io::Result<Option<Clearance>>
VettingStore::clear_brief(key, digest, kinds: &[FindingKind], by, at) -> io::Result<Option<Clearance>>
                                         // NotFound unless queued; None when its content is no longer `digest`
VettingStore::promote_cleared(operation, proposal_id, received_by, kinds, by, at) -> io::Result<Brief>
                                         // promote_mission_proposal + clear_brief in one step
```

A held brief is in `held/KEY.json` (SQL: state `held`, column `held`); it is never claimed,
`submit_brief` refuses its key and Source polling treats it as in flight. `release_held`
returns it to `new/` and clears the kinds it was held for, for its content digest
(`vetting/KEY.json`, SQL column `clearance`), so the same checks do not hold that content
again; a clearance for another digest counts for nothing and is replaced by the next. The
clearance is kept apart from the brief's own JSON and dropped when the brief finishes.
`refuse_held` files it as failed with outcome `{"state": "refused", "mission_id": KEY,
"reason", "refused_by", "refused_at", "vetting"}`, so a Source still gets its CX report. The
file Registry writes `held/` before it removes `new/`, and settles a brief found in both.

## Identity and handler API

`MissionId::try_new(impl Into<String>)` and `SessionId::try_new(impl Into<String>)`
validate non-empty values up to 256 UTF-8 bytes with no control characters. Both also
implement `FromStr` and `Display`. A Mission can group several provider Sessions; a
Session ID is issued by the provider and stays stable when that Session resumes.

With `station-tls` (`ai6::station::identity`), a Station's mutual TLS is
[`coqui-peer-tls`](https://github.com/Boriken-Dev/coqui-peer-tls) and `coqui-peer`, and this
module fixes where AI6 keeps things and the protocol it speaks:

```rust
pub const STATION_ALPN: &[u8] = b"ai6-station/1";
pub const IDENTITY_FILE: &str = "identity.pem";
pub fn open_identity(state_dir: &Path) -> Result<Identity, StationIdentityError>;
pub fn open_peers(state_dir: &Path, policy: &Policy) -> Result<TrustStore, StationIdentityError>;
pub fn trust_peers(store: TrustStore) -> Trust;
pub fn client_config(identity: &Identity, server: &PeerId, policy: &Policy)
    -> Result<Built<ClientConfig>, StationIdentityError>;
pub fn server_config(identity: &Identity, trust: &Trust, policy: &Policy)
    -> Result<Built<ServerConfig>, StationIdentityError>;
```

A `PeerId` is the SHA-256 of the certificate, printed `sha256:<hex>` and read in any spelling
(`sha256:HEX`, bare hex, colon pairs, grouped base32). `Identity`, `Trust` (`Pinned`, `Ca`,
`PinnedOrCa`, `AnyWithKeyProof`), `CaPolicy`,
`Policy`, `TrustStore`, `WaitingEntry` and the rest are re-exported from those crates; their
API headers describe them. `open_identity` stores the certificate and private key in
`identity.pem` (Unix mode `0600`). `Policy::strict()` is the default: TLS 1.3, SHA-2 pins;
`Policy::compatible()` and `legacy()` loosen it and `Built::weakened` lists what is looser.
`client_config` pins the server's fingerprint; `server_config` requires a client certificate
and proof of its key, then applies `trust`. A peer not in the store is refused and, once it
has proved its key, recorded under `peers/waiting` with the address it came from; waiting is
bounded per source address, in total, and by how often one peer is written down again (the
policy's `waiting`: `per_source`, `total` and `rerecord`). An operator lists
them with `ai6 peer waiting` (fingerprint and source address) and accepts one with
`ai6 peer accept <fingerprint>`. The default AI6 state location can be overridden with
`AI6_STATE` or `AI6_HOME`.
`AccessTokenRecord` is the device-token metadata value. It holds the
canonical UUID revocation ID, SHA-256 digest, authoritative Directory person ID, and issue,
expiry and optional revocation times; its debug output redacts the digest.
`AccessTokenStore` (`store_access_token`, `find_access_token`, `revoke_access_token`);
`FileRegistry` stores and validates records in the `access-tokens/` and `access-token-digests/`
directories, returns only active records, and serializes revocation under the shared
Registry lock.

## Q Branch loadouts (`ai6::directorates::q`)

```rust
pub const MAX_LOADOUT_BYTES: usize;
#[non_exhaustive]
pub enum QErrorKind { Io, TooLarge, InvalidUtf8, Parse, Schema }
pub struct QError;
impl QError {
    pub const fn kind(&self) -> QErrorKind;
    pub fn path(&self) -> Option<&Path>;
    pub const fn line(&self) -> Option<usize>;
    pub const fn column(&self) -> Option<usize>;
    pub const fn offset(&self) -> Option<usize>;
    pub fn reason(&self) -> Option<&str>;      // a pool or account and the setting at fault; never input
}
pub struct Loadout;
impl Loadout {
    pub fn agent(&self) -> &str;
    pub fn sandbox_name(&self) -> Option<&str>;
    pub fn mcp_names(&self) -> &[String];
    pub fn secret_names(&self) -> &[String];
    /// Declares only the account's secret (agent "pool:<id>/<name>"), so a broker leases it
    /// alone; None for a config-dir account.
    pub fn for_account(pool: &AccountPool, account: &Account) -> Option<Self>;
}
#[non_exhaustive] pub enum AccountKind { ClaudeOauthToken, AnthropicApiKey, AnthropicAuthToken,
    OpenaiApiKey, CodexAccessToken, ConfigDir }   // as_str()/parse(): "claude-oauth-token", …, "config-dir"
impl AccountKind {
    pub fn env(self) -> Option<&'static str>;          // CLAUDE_CODE_OAUTH_TOKEN, ANTHROPIC_API_KEY, ANTHROPIC_AUTH_TOKEN
    pub fn login_flag(self) -> Option<&'static str>;   // --with-api-key, --with-access-token
    pub fn reads_secret(self) -> bool;                 // all but ConfigDir
    pub fn carries_session(self) -> bool;              // the env kinds: a Session may move accounts
}
pub const CREDENTIAL_VARIABLES: &[&str];   // every provider credential variable a pooled Session is stripped of
#[non_exhaustive] pub enum PoolStrategy { Headroom /* default */, RoundRobin, Priority }   // "headroom" | "round_robin" | "priority"
pub struct Account;   // Account::new(name, kind) .secret(name) .dir(path) .env(var) .priority(i64) .max_concurrent(n) .label(k, v)
impl Account { pub fn name(&self) -> &str; pub fn kind(&self) -> AccountKind;
    pub fn secret_name(&self) -> Option<&str>;        // `secret`, else the account's name; None for ConfigDir
    pub fn login_dir(&self) -> Option<&Path>; pub fn priority_value(&self) -> i64;
    pub fn concurrency(&self) -> Option<usize>; pub fn labels(&self) -> &BTreeMap<String, String>;
    pub fn variable(&self, handler: &str) -> Option<String>;   // env, else the kind's, else CLAUDE_CONFIG_DIR/CODEX_HOME
    pub fn weakening(&self, pool: &str) -> Option<String> }    // ConfigDir and the Codex login kinds
pub struct AccountPool;   // AccountPool::new(id, handler) .operations(globs) .strategy(s) .account(a) .login_command(argv)
impl AccountPool { pub fn id(&self) -> &str; pub fn handler(&self) -> &str;
    pub fn operation_globs(&self) -> &[String]; pub fn balancing(&self) -> PoolStrategy;
    pub fn accounts(&self) -> impl Iterator<Item = &Account>; pub fn get(&self, name: &str) -> Option<&Account>;
    pub fn login(&self) -> &[String];                  // default ["codex"]
    pub fn admits(&self, operation: Option<&str>) -> bool;
    pub fn validate(&self) -> Result<(), String> }   // the catalog also refuses an account whose secret is green ink
pub enum PoolChoice<'a> { Unpooled, Pools(Vec<&'a AccountPool>), Refused(String) }
/// The three-way rule: a pool is usable when it serves `handler`, its globs admit
/// `operation`, and `allowed` (the Operation's `pools`; None = every pool) names it;
/// `pinned` (the Agent's `pool`) narrows to one. Refused names each pool and why.
pub fn choose_pools<'a>(pools: impl IntoIterator<Item = &'a AccountPool>, handler: Option<&str>,
    operation: Option<&str>, allowed: Option<&[String]>, pinned: Option<&str>) -> PoolChoice<'a>;
pub trait AccountLogin: Send + Sync {
    fn login(&self, pool: &AccountPool, account: &Account, home: &Path, credential: &str) -> Result<(), String>;
}
/// Runs `<login_command> login <flag>` with CODEX_HOME=home and the credential plus a newline
/// on stdin, output discarded; killed after the timeout (default 60 s).
#[derive(Default)] pub struct CommandLogin;   // .with_timeout(Duration)
pub struct SandboxProfile;
impl SandboxProfile {
    pub fn name(&self) -> &str;
    pub fn backend(&self) -> &str;
    pub fn read_write(&self) -> &[String];
    pub fn read_only(&self) -> &[String];
    pub fn network(&self) -> &[String];
}
pub struct McpServer;
impl McpServer {
    pub fn name(&self) -> &str;
    pub fn command(&self) -> &[String];
    pub fn deny(&self) -> &[String];
}
pub struct LoadoutCatalog;   // Default: empty
impl LoadoutCatalog {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, QError>;
    pub fn with_pool(self, pool: AccountPool) -> Result<Self, QError>;   // validates; replaces the same ID
    pub fn pools(&self) -> impl Iterator<Item = &AccountPool>;
    pub fn pool(&self, id: &str) -> Option<&AccountPool>;
    pub fn for_agent(&self, agent: &str) -> Result<&Loadout, QError>;
    pub fn sandbox_for(&self, loadout: &Loadout) -> Option<&SandboxProfile>;
    pub fn servers_for(&self, loadout: &Loadout) -> Result<Vec<&McpServer>, QError>;
    pub fn loadouts(&self) -> impl Iterator<Item = &Loadout>;
    pub fn sandboxes(&self) -> impl Iterator<Item = &SandboxProfile>;
    pub fn mcp_servers(&self) -> impl Iterator<Item = &McpServer>;
    pub fn secret(&self, name: &str) -> SecretDefinition;   // its [secret.<name>], or every default
    pub fn secrets(&self) -> Vec<SecretDefinition>;         // every name a loadout, table or pool account names
}
impl FromStr for LoadoutCatalog { type Err = QError; }
pub struct SecretDefinition;
impl SecretDefinition {
    pub fn new(name: impl Into<String>) -> Self;  // env = NAME upper, -/. as _; source File("secrets/<name>")
    pub fn name(&self) -> &str; pub fn env(&self) -> &str;
    pub fn green_ink(&self) -> bool; pub fn kinds(&self) -> &[String];   // empty: any approval kind
    pub fn targets(&self) -> &[String];                   // globs on the approval's target; empty: any
    pub fn unlocked_by(&self, kind: &str, target: &str) -> bool;
    pub fn green_ink_weakening(&self) -> Option<String>;  // Some for green ink without targets
    pub fn source(&self) -> &SecretSource;
}
#[non_exhaustive] pub enum SecretSource { Env(String), File(PathBuf) }   // a relative file: relative to the config dir
impl SecretSource { pub fn weakening(&self) -> Option<String>; }          // Some for Env: local harnesses inherit it
```

Loadouts use strict `[loadout.<agent>]`, `[sandbox.<name>]`, `[mcp.<name>]`,
`[secret.<name>]` (`env`, `green_ink`, `kinds` and `targets` (globs, only with `green_ink`), `from = { env = "…" }
| { file = "…" }`; never a value) and `[pool.<id>]` (`handler`, `operations`, `strategy`,
`login_command`, accounts `[pool.<id>.account.<name>]` with `kind`, `secret`, `dir`, `env`,
`priority`, `max_concurrent`, `labels`) TOML tables; a document needs a loadout or a pool. Parsing is bounded to one MiB, denies unknown
fields and invalid or duplicate names, and validates references before returning a catalog.
Errors distinguish I/O, size, encoding, TOML syntax and schema failures; diagnostics expose
source positions but never document contents. The catalog is inert: it does not expand
environment variables, resolve paths, launch MCP servers or create sandboxes. A Station
loads `loadouts.toml` from its configuration directory (or `StationConfig::with_loadouts`)
and applies only the secrets: see "Credentials" under the Station daemon.

Secret references can be leased through `SecretBroker`:

```rust
pub const DEFAULT_SECRET_LEASE_TTL: Duration;
pub const MAX_SECRET_LEASE_TTL: Duration;
pub struct SecretLeaseRequest;
impl SecretLeaseRequest {
    pub fn try_new(station: impl Into<String>, mission_id: impl Into<String>)
        -> Result<Self, SecretError>;
    pub fn with_ttl(station: impl Into<String>, mission_id: impl Into<String>, ttl: Duration)
        -> Result<Self, SecretError>;
    pub fn station(&self) -> &str;
    pub fn mission_id(&self) -> &str;
    pub const fn ttl(&self) -> Duration;
}
pub struct SecretRequest;
impl SecretRequest {
    pub fn lease_id(&self) -> &str;
    pub fn station(&self) -> &str;
    pub fn mission_id(&self) -> &str;
    pub const fn ttl(&self) -> Duration;
}
pub struct SecretBackendFailure;
impl Default for SecretBackendFailure;
impl SecretBackendFailure { pub const fn new() -> Self; }
pub trait SecretBackend: Send + Sync {
    fn acquire(&self, request: &SecretRequest, names: &[String])
        -> Result<BTreeMap<String, SecretValue>, SecretBackendFailure>;
    fn revoke(&self, lease_id: &str) -> Result<(), SecretBackendFailure>;
}
pub struct SecretBroker;
impl SecretBroker {
    pub fn new(backend: impl SecretBackend + 'static) -> Self;
    pub fn with_max_ttl(self, max_ttl: Duration) -> Result<Self, SecretError>;
    pub fn lease(&self, loadout: &Loadout, request: SecretLeaseRequest)
        -> Result<SecretLease, SecretError>;
    /// Some of the loadout's names; an undeclared one is AccessDenied and nothing is acquired.
    pub fn lease_names(&self, loadout: &Loadout, names: &[String], request: SecretLeaseRequest)
        -> Result<SecretLease, SecretError>;
}
/// The Station's own backend: reads each value at lease time from its SecretSource (a file
/// loses one trailing newline); missing or empty fails; revoke is a no-op.
pub struct StationSecrets;
impl StationSecrets {
    pub fn new(definitions: impl IntoIterator<Item = SecretDefinition>) -> Self;
    pub fn relative_to(self, dir: &Path) -> Self;   // resolve relative files against dir (the Station's config dir)
}
impl SecretBackend for StationSecrets;
pub struct SecretValue;
impl SecretValue {
    pub fn new(value: impl Into<String>) -> Self;
    pub fn with_exposed<T>(&self, use_value: impl FnOnce(&str) -> T) -> T;
}
pub struct SecretLease;
impl SecretLease {
    pub fn station(&self) -> &str;
    pub fn mission_id(&self) -> &str;
    pub fn expires_at(&self) -> Instant;
    pub fn is_active(&self) -> bool;
    pub fn names(&self) -> Vec<String>;
    pub fn with_secret<T>(&self, name: &str, use_value: impl FnOnce(&str) -> T)
        -> Result<T, SecretError>;
    pub fn revoke(&self) -> Result<(), SecretError>;
}
#[non_exhaustive]
pub enum SecretError { InvalidIdentity, InvalidTtl, Backend, InvalidResponse, AccessDenied, Expired }
pub fn claude_mcp_config(catalog: &LoadoutCatalog, loadout: &Loadout)
    -> Result<serde_json::Value, QError>;
```

Backends receive only the configured loadout's secret names, Station, Mission ID, an
opaque lease ID and a bounded TTL. They return exactly those names as `SecretValue`s;
unexpected keys fail closed. Both `SecretValue` and lease debug output redact values.
Values are zeroized when dropped. `with_secret` holds the lease lock during the callback
so revocation cannot race an active read; the callback must be short and must not re-enter
that lease. Explicit `revoke` clears local values first and retries backend revocation
after failures. Lease expiry is monotonic and backend implementations must enforce their
own TTL and bounded revocation timeout. Secret values must stay out of argv, logs,
transcripts, Cables and reports.

`claude_mcp_config` emits only the exact loadout's stdio servers as `mcpServers` entries
with a `command` and `args` array. It rejects server names Claude cannot represent and
per-server deny lists because this host config cannot enforce them. It does not include
secrets, environment variables or identity metadata and does not start the servers.

Sandbox launch policy can be translated into an inert bubblewrap vector:

```rust
pub struct SandboxRequest;
impl SandboxRequest {
    pub fn new(worktree: impl Into<PathBuf>, command: impl IntoIterator<Item = String>) -> Self;
    pub fn with_allowed_root(self, root: impl Into<PathBuf>) -> Self;
    pub fn with_executable(self, executable: impl Into<String>) -> Self;
}
#[non_exhaustive]
pub enum SandboxError {
    UnsupportedBackend, UnsupportedPlatform, UnsupportedNetworkPolicy,
    InvalidCommand, InvalidPath, InvalidWorktree, MissingWorktreeMount, DuplicateMount,
}
impl SandboxProfile {
    pub fn bubblewrap_argv(&self, request: &SandboxRequest) -> Result<Vec<String>, SandboxError>;
}
```

This translator is Linux-only and never invokes a process. The worktree must mount at
`/workspace`; canonicalized additional mounts must remain inside a caller-supplied allowed
root. Parent mounts precede child mounts, network access is isolated, and requested host
allowlists fail closed because this profile cannot enforce them. Invalid UTF-8 paths,
missing paths, escapes and duplicate destinations are rejected. The daemon does not yet
apply this vector to AgentHandler startup.

## Cables (`ai6::directorates::registry::cables`)

`CableScope` defines Agent unicast and Station, Theater, global or named-channel
broadcast. `CableTarget::try_new(scope, name)` requires a name except for global.
`CableRecipient::try_new(agent, session_id, state)` validates the relay-resolved
recipient. `CableDraft::new(sender_agent, sender_operation, sender_station, target, body,
recipients, idempotency_key, id, created_at, expires_at)` creates the required fields;
builder methods add the optional Theater, Mission, Session, conversation and reply
identities. `Cable::try_new(draft)` validates a complete envelope. Bodies are limited to
16 KiB UTF-8, expiry must be later than creation and no more than 24 hours later, and
recipient `(agent, session_id)` pairs must be unique. Serde uses the Registry's field names
and rejects missing, unknown or invalid fields. `Debug` redacts Cable bodies.
`with_recipient_state(agent, session_id, state)` returns a new Cable for a permitted
delivery transition, `None` for a recipient not in the audience, or `CableError` for an
invalid transition. The relay must derive sender identity from the authenticated Agent
Session; these value types do not authorize sends or resolve broadcast audiences.

`CablePrincipal::try_new(agent, operation, theater, station, session_id, mission_id)` is for
trusted session adapters only; request objects have no sender identity. `CablePolicy`
holds deny-by-default `CableGrant`s and per-Agent `CableInboundMode` values. A grant
names an Agent (or `*`), a `CableAction` (`Send`, `Receive`, `Discover`, `Release`) and one exact
`CableTarget`. `CableRelay::new(Arc<dyn RegistryBackend>, policy, endpoint_source, channels)`
uses the shared Registry, an authoritative Agent/Session directory and named channel
members. `with_limits` configures bounded fan-out (1–1,000) and persistent sender/scope
token buckets. `send(principal, CableSendRequest)` checks sender and every receiver,
deduplicates the audience, snapshots recipient Sessions, applies deliver/hold/refuse and
stores an idempotent Cable. Same-Station unicast uses Station grants; cross-Station
unicast uses explicit Agent grants. Theater, global and channel broadcasts require exact
scope grants. `discover` exposes only same-Station peers allowed by both sides' grants.
`inbox` rechecks current receive permissions on queued deliveries, marks delivered records,
and returns a `CableDelivery` without the audience list or idempotency key.
`held(principal, limit)` lists held Cable metadata only under matching Release grants and
omits bodies. `release(principal, cable_id)` requires the same exact scope grant, rechecks
each recipient's current Receive grant, delivers still-authorized recipients and refuses
ones whose grant was revoked. `CableHeldDelivery` identifies held recipients; `CableRelease`
reports delivered/refused counts.
`acknowledge` is idempotent for delivered messages. `reply` checks that the original was
delivered to the caller and derives its destination and reply threading from the stored
record. `update_policy` applies revocations to queued messages on their next inbox read.
The default lifetime is 15 minutes, maximum 24 hours and default fan-out limit is 100.
`CableRelayError` keeps unavailable and unauthorized targets indistinguishable.
`with_audit_sink` emits sender/target/decision/time metadata only; bodies are omitted.
Inbound bodies are untrusted data and do not grant authority.

An assistant acting for a person (Moneypenny) sends with
`CablePrincipal::with_on_behalf_of(person) -> Result<Self, CableError>` (1-256 characters, no
control characters; `on_behalf_of()` reads it). The relay then stamps the Cable
(`CableDraft::with_on_behalf_of`, `Cable::on_behalf_of() -> Option<&str>`; serde field
`on_behalf_of`, default `None`, omitted when `None`), keeps the audience to endpoints of the
principal's Operation other than its own Agent, and carries the person in
`CableAuditRecord::on_behalf_of`. A reply to such a Cable reaches only the Session that sent
it, never the Agent's other conversations (its `session_id` is required). Recipients see
`CableDelivery::on_behalf_of` and `CableDelivery::sent_as` (`"<from_agent> for <person>"`),
both omitted from JSON when the Cable was not sent for a person. Its sender bucket is keyed
by Agent and person (`sender:<agent>:person:<person>`), so each person has their own.

`CableRelay::with_person_audit(write: impl Fn(&CableAuditRecord) -> Result<(), String>)`
installs the durable log of Cables sent for a person: the relay calls it with the Cable's
record (decision `accepted`, its ID) before `submit_cable`, also on an idempotent retry, and
refuses the send (`Unavailable`, an `audit_failed` record to the sink) when it errs or none is
installed. The daemon writes it to the Registry audit log (action `cable.send`, actor the
person, the sending Operation, resource `station:<id>`, target
`<agent> <decision> <scope>:<name> <cable id or ->`); its audit sink writes refused sends for a
person the same way, best effort; other relay records stay metadata.

`CableEndpoint::with_conversation_only(self) -> Self` (`conversation_only() -> bool`) marks
an assistant conversation's endpoint: it is never discovered and never in the audience of a
send to an Agent, Station, Theater, channel or everyone; only a reply to a Cable it sent
reaches it. `inbox_from(principal, operation, limit)` reads the inbox as `inbox` does, for
Cables sent in `operation` only: the others are neither delivered nor counted against
`limit`.

The daemon reads strict `cables.toml` from its config directory. Missing configuration
denies every action. `[[grants]]` entries name `agent`, `action`, `scope`, and optional
`name`; `[inbound]` selects `deliver`, `hold`, or `refuse` by Agent; `[channels]` maps
channel names to Agent lists. `max_fanout`, `sender_rate`, `sender_burst`, `scope_rate`,
and `scope_burst` bound routing. `ServeConfig::with_station_id` or `AI6_STATION_ID` supplies
the Station identity; the default is `<host>-<random>`, created once and kept in `station-id`
in the AI6 state directory so a restarted daemon stays the same Station.

`CableStore::submit_cable(&cable)` stores (in a `FileRegistry`) the envelope under `cables/` with atomic
replacement and the same `.ai6-registry.lock` used by Registry claims. A retry with the
same sender and idempotency key returns the stored record when immutable request content
matches; conflicting content returns `AlreadyExists`. `find_cable(sender, key)` and
`get_cable(id)` read accepted records. `cable_inbox(agent, session_id, limit, now)` lists
unexpired queued/delivered records for an Agent Session, sorted by creation time and ID;
`limit` is 1–1,000 and `now` is optional Unix time for deterministic callers.
`update_cable_delivery(id, agent, session_id, state, now)` serializes recipient state
changes under the Registry lock and automatically expires overdue deliveries.
`held_cables(limit, now)` lists unexpired envelopes containing at least one held recipient
for the scoped relay to authorize.
`take_cable_quotas(buckets, now)` atomically consumes sender and scope token buckets from
`cable-quotas.json`; each bucket is `(key, rate_per_second, burst_capacity)`, and its key
is SHA-256 hashed before it is stored. The file backend bounds scans to 10,000 Cable files
and records to 2 MiB; `SqlRegistry` keeps Cables in `ai6_cables` and the buckets, keyed the
same way, in `ai6_cable_quotas`, with the same bounds.

`ai6::directorates::section_00::agent` defines `AgentStart`, `AgentResume`, `AgentMessage`, `AgentEvent`, the
`AgentHandler` Guatiao kind `ai6_agent_handler`, and the `AgentSession` object kind
`ai6_agent_session`. Handlers own harness launch, initial-context delivery, resume,
message submission and event decoding. AI6 supplies an `AgentTerminal` object bridge
over its selected `Terminal` provider. `AgentHandler::stdio(&self) -> Result<HarnessStdio,
ProviderError>` is an appended slot (default `Ok(HarnessStdio::Terminal)`): a handler whose
harness speaks JSON over standard input and output answers `Json` (the `claude` and `codex`
handlers do), and the Station gives it a provider without a terminal. `AgentStart`, `AgentResume`, `AgentMessage`,
`AgentEvent`, `UsageLimit` and `TokenUsage` are also serde `Serialize` and `Deserialize`
(field names as declared): a runner's API carries them as JSON.

```rust
pub const LIMIT_EVENT: &str = "limit";
pub struct UsageLimit { pub resets_at: Option<i64> /* Unix seconds */, pub window: Option<String> }
pub struct TokenUsage { pub input: u64, pub output: u64, pub cache_read: u64, pub cache_write: u64,
                        pub cost_micro_usd: u64, pub cumulative: bool }
pub struct QuotaReport { pub window: String /* five_hour, weekly, … or primary/secondary */,
                         pub used_permille: Option<u16>, pub resets_at: Option<i64>,
                         pub status: Option<String> /* allowed, allowed_warning, rejected */ }
pub struct AgentEvent { pub kind: String, pub text: Option<String>, pub data: Option<Vec<u8>>,
                        pub limit: Option<UsageLimit>, pub usage: Option<TokenUsage>,
                        pub tools: Vec<String>,
                        pub quota: Vec<QuotaReport> /* serde default: empty */ }
// AgentStart and AgentResume carry `max_cost_micro_usd: Option<u64>`: a spend ceiling a
// harness may enforce itself (Claude Code gets `--max-budget-usd`); AI6 enforces budgets anyway.
// They also carry `project_settings: bool` and `project_mcp: bool` (serde default false):
// whether the harness may load its working directory's configuration, and its `.mcp.json`
// servers, decided by the Station from the Operation's harness policy on every start and
// resume. `project_settings: false` means Claude Code runs with `--setting-sources user` and
// Codex with the working directory marked an untrusted project; Claude Code keeps
// `--strict-mcp-config` unless both are true.
// And `answer_only: bool` (serde default false), set for a Chief of Staff review: the harness
// only reads and answers. Claude Code: `--restricted --tools= --disallowedTools <all of its own,
// web, sub-agent and mcp__*> --setting-sources user --strict-mcp-config --mcp-config
// {"mcpServers":{}}`, and no AI6 servers or gate. Codex: `-c mcp_servers={}` plus `-c` for
// `features.shell_tool=false`, `web_search="disabled"`, `features.multi_agent=false`,
// `agents.enabled=false`, `features.apps=false`, `skills.max_context_tokens=1`,
// `tools.experimental_request_user_input={enabled=false}`, every other enabled feature and
// every MCP server of the user's switched off and a tool check before start (see
// `ai6-agent-codex`; Codex 0.164 fails it), the project marked untrusted, thread
// `sandbox: "read-only"`, `approvalPolicy: "never"`, `config: {"web_search": "disabled"}`, and
// no gate.
// And `assistant: Option<AssistantMode>` (serde default None), set for Moneypenny: wins over
// `answer_only` and is built from the mode's parts below.
// AgentResume also carries `initial_context: Vec<String>` (serde default empty): the standing
// part of the start-up context (the Agent's context, its role's purpose and policy; for
// Moneypenny her instructions, never the conversation recap), which Claude Code is given again
// as `--append-system-prompt` (2.1.310 reuses the prompt it recorded with the Session until
// the Session is compacted, then builds it from the resuming launch); Codex keeps a thread's
// instructions and ignores it.
pub struct AssistantMode { pub mcp_servers: Vec<McpServerRef>,
                           pub interpreters: Vec<String> /* serde default */,
                           pub skills: Vec<String> /* serde default */,
                           pub network: AssistantNetwork /* serde default */,
                           pub files: Vec<String> /* serde default; directories it may write */,
                           pub context_file: Option<String> /* serde default; absolute, outside the working directory */ }
pub struct McpServerRef { pub name: String /* moneypenny */,
                          pub env_prefix: String /* AI6_MONEYPENNY_MCP: <prefix>_URL, <prefix>_TOKEN */ }
pub struct AssistantNetwork { pub policy: String /* limited (default) | denied | open */,
                              pub allowed_hosts: Vec<String> }
impl AssistantMode { pub fn moneypenny() -> Self; pub fn validate(&self) -> Result<(), String>;
                     pub fn with_cables(self) -> Self; } // adds server `cables`,
                                                         // prefix AI6_ASSISTANT_CABLE_MCP
impl AssistantNetwork { pub fn reaches_nothing(&self) -> bool; } // denied, or limited with no host
// Claude Code: `--restricted --tools= --disallowedTools <its shell, file, web, sub-agent and
// Skill tools, no mcp__> --allowedTools mcp__<name>,… --setting-sources user
// --strict-mcp-config --mcp-config {"mcpServers":{<name>:{"type":"http","url":"${P_URL}",
// "headers":{"Authorization":"Bearer ${P_TOKEN}"}}}}`, no gate. Codex: `-c mcp_servers=
// {<name>={url="…",bearer_token_env_var="P_TOKEN"}}` (replacing the user's servers), the same
// `-c` settings that switch its built-in tools off as answer-only, the project untrusted, and
// the answer-only thread policy. Both refuse to start (UNSUPPORTED) with
// interpreters, skills or a network reaching more than the servers, and (BAD_VALUE) when the
// environment lacks a server's URL or token; tokens never reach a command line.
// `validate` also checks `files` (1 to 8 `/`-separated names of letters, digits, `.`, `-`,
// `_`, none starting with `.`) and that `context_file` is absolute. With `files`, Claude Code
// gets `--tools Read,Write,Edit,Glob,Grep --permission-mode dontAsk`, the deny list less
// those five, and `--allowedTools mcp__<name>,…,Read,Glob,Grep,Edit(/<dir>/**),…`; a
// `context_file` is `--append-system-prompt-file`; every assistant Session has
// `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1`. Codex refuses `files` and `context_file`
// (UNSUPPORTED).
```

`ai6::directorates::section_00::assistant` is the contract between Universal Exports and a
Station for Moneypenny's Sessions (all also re-exported at the crate root):

```rust
pub const ASSISTANT_MCP_ENV_PREFIX: &str = "AI6_MONEYPENNY_MCP";
pub const ASSISTANT_MCP_URL_ENV: &str = "AI6_MONEYPENNY_MCP_URL";
pub const ASSISTANT_MCP_TOKEN_ENV: &str = "AI6_MONEYPENNY_MCP_TOKEN";
pub const ASSISTANT_MCP_SERVER: &str = "moneypenny";
pub const ASSISTANT_MCP_PATH: &str = "v1/moneypenny/mcp";      // relative to the gateway origin
pub const ASSISTANT_CABLE_MCP_ENV_PREFIX: &str = "AI6_ASSISTANT_CABLE_MCP";
pub const ASSISTANT_CABLE_MCP_URL_ENV: &str = "AI6_ASSISTANT_CABLE_MCP_URL";
pub const ASSISTANT_CABLE_MCP_TOKEN_ENV: &str = "AI6_ASSISTANT_CABLE_MCP_TOKEN";
pub const ASSISTANT_CABLE_MCP_SERVER: &str = "cables";
pub const MAX_ASSISTANT_CABLE_GRANTS: usize = 64;
pub const MAX_ASSISTANT_MESSAGE_BYTES: usize = 32 * 1024;
pub const MAX_ASSISTANT_CONTEXT_BYTES: usize = 24 * 1024;     // all context items together
pub const MAX_ASSISTANT_EVENT_BYTES: usize = 64 * 1024;       // longer event text is cut
pub const MAX_ASSISTANT_EVENTS: usize = 64;                   // per report
pub const EVENT_ACCEPTED, EVENT_REJECTED, EVENT_TEXT, EVENT_TOOL, EVENT_COMPLETED,
          EVENT_FAILED, EVENT_LIMIT, EVENT_INTERRUPTED, EVENT_CLOSED: &str; // "accepted", …
pub const EVENT_MEMORY_CHANGES: &str = "memory_changes"; // text: unified diff; `change`: the files
pub const EVENT_NOTICE: &str = "notice";       // text: work saved elsewhere than main, a repository not opened
pub const EVENT_CABLE_PROPOSAL: &str = "cable_proposal"; // `cable`: held, body included
pub const EVENT_CABLE_SENT: &str = "cable_sent";      // `cable`: sent, `cable_id` set
pub const COMMIT_MEMORY: &str = "memory"; pub const COMMIT_SHARED: &str = "shared";
pub const MAX_ASSISTANT_CHANGE_BYTES: usize = 64 * 1024;  // text of one change or commit
pub const MAX_ASSISTANT_CHANGE_FILES: usize = 32;
pub struct AssistantJob { pub job_id: String, pub conversation_id: String, pub agent_id: String,
    pub message: String, pub context: Vec<String> /* default: standing context */,
    pub recap: Option<String> /* default, omitted when None: for a harness started afresh only */,
    pub mcp_token: String,
    pub token_expires_at: f64, pub session_id: Option<String> /* default */,
    pub close: bool /* default */,
    pub interrupt: bool /* default */,
    pub exposed: bool /* default: Conversation.exposed at queue time, the one record of it */,
    pub person_id: Option<String> /* default, omitted when None; required with workspace, commit or cables */,
    pub workspace: Option<AssistantWorkspace> /* default, omitted when None */,
    pub commit: Option<AssistantCommit> /* default, omitted when None */,
    pub cables: Vec<AssistantCableGrant> /* default, omitted when empty */,
    pub cable_confirm: Option<AssistantCableDecision> /* default, omitted when None */,
    pub cable_discard: Option<AssistantCableDecision> /* default, omitted when None */ }
                                                     // Debug redacts the token
impl AssistantJob { pub fn validate(&self) -> Result<(), String>; }
pub struct AssistantWorkspace { pub personal: String /* the person's repository, expanded */,
    pub shared: BTreeMap<String, String> /* default; checkout name (group or "org") -> repository */,
    pub author_name: String, pub author_email: String /* the person's */ }
pub struct AssistantChange { pub digest: String /* hex SHA-256 of the files, then the bases */,
    pub files: BTreeMap<String, Option<String>> /* path -> new text, None removes */,
    pub base: BTreeMap<String, Option<String>> /* default, omitted when empty: path -> hex SHA-256
        of the text it was made from, None = did not exist; a commit is refused when it differs */ }
impl AssistantChange { pub fn new(files) -> Self /* no base */; pub fn based(files, base) -> Self;
    pub fn digest_of(&files) -> String; pub fn digest_over(&files, &base) -> String;
    pub fn validate(&self, memory: bool) -> Result<(), String>; } // bases only of its files, 64 hex
pub struct AssistantCommit { pub kind: String /* COMMIT_MEMORY | COMMIT_SHARED */,
    pub repo: String, pub change: AssistantChange, pub message: String /* trailer included */,
    pub author_name: String, pub author_email: String }
impl AssistantCommit { pub fn validate(&self) -> Result<(), String>; }
impl AssistantWorkspace { pub fn validate(&self) -> Result<(), String>; }
pub fn repository_path(path: &str) -> bool; // 1-8 `/`-separated names, none starting with `.`
pub struct AssistantCableGrant { pub operation: String, pub send: bool, pub read: bool }
pub struct AssistantCableDecision { pub id: String, pub digest: String }
pub struct AssistantCable { pub id: String /* desk's held ID, or the Cable ID when sent at once */,
    pub operation: String, pub target: String /* agent:007, station:hq, global, reply:<cable> */,
    pub recipient: String /* the audience; for a reply agent:<sender of the answered Cable> */,
    pub ttl_seconds: Option<u32> /* default, omitted when None: the relay's 15 minutes */,
    pub body: Option<String> /* proposals only */, pub digest: String /* request_digest */,
    pub cable_id: Option<String> /* once sent */ }
impl AssistantCable { pub fn request_digest(&self) -> String; } // SHA-256 hex of operation,
    // target, recipient, ttl and body, each prefixed with its byte length and `:`
pub struct AssistantReport { pub station_id: String, pub conversation_id: String,
    pub job_id: Option<String>, pub session_id: Option<String>, pub events: Vec<AssistantEvent> }
pub struct AssistantEvent { pub kind: String, pub text: Option<String>, pub tools: Vec<String>,
    pub change: Option<AssistantChange> /* default, omitted when None */,
    pub cable: Option<AssistantCable> /* default, omitted when None */ }
impl AssistantEvent { pub fn new(kind: &str, text: Option<String>) -> Self; } // cuts text
pub fn valid_identity(value: &str) -> bool;          // 1-256 bytes, no control characters
```

All are serde types with unknown fields refused. `validate` checks identities, a message
of at most `MAX_ASSISTANT_MESSAGE_BYTES` (blank only with `close`, `interrupt`, `commit`,
`cable_confirm` or `cable_discard`, of which a job sets one), the context bound (context and
recap together), a token of 32-512 bytes without whitespace, a workspace (repositories of a
known `RepoKind`, checkout names as `file_safe` names, an author without control characters,
`<` or `>`), a commit (a known kind; the change's 1-32 files within
`MAX_ASSISTANT_CHANGE_BYTES`, each `repository_path`, under `memory/` for `COMMIT_MEMORY`, its
digest matching; a message of 1-4096 bytes), and at most `MAX_ASSISTANT_CABLE_GRANTS` Cable
grants for distinct valid Operations, which need `person_id`. A job's answer is one report
with its `job_id` and one `accepted` (for a commit, text saying where it landed) or `rejected`
event (text: the reason); the harness's own events follow in reports without a `job_id`.

A handler reports a provider usage or rate limit as an event of kind `LIMIT_EVENT` with
`limit` set, in place of the `completed` or `failed` event the same output would
otherwise produce; `limit` is `None` on every other event. `resets_at` is `None` when the
provider names no reset time. The daemon parks the Mission (or posts a backup Agent) and
closes the Session.

`usage` is what the provider reported with the event (Claude Code on `result`, Codex on its
token-usage notification); `cumulative` says whether the figures are running totals for the
provider process (`true`) or only what the last step added. `UsageTracker` turns either into
additions, so a resumed process is never counted twice. `tools` names the tools an event
calls (kind `tool` for Claude Code), which is how a call to a listed tool exposes a Session.
The daemon also queues `budget` (warning or limit), `exposed` (text: the source) and
`quarantined` events for the Mission.

## Terminal API (`ai6::directorates::q::terminal`)

`Spec`, `ExitInfo`, `Terminal`, `AgentTerminal` and `TerminalSession` define the
Guatiao terminal contracts. `Terminal` is a provider kind discovered by AI6;
`AgentTerminal` is the object bridge AI6 wraps around that provider for a handler.
The `ai6_terminal` and `ai6_terminal_session` kind floor hashes are pinned in tests.
`ExitInfo::code` is the signed 32-bit exit code the standard library reports, so Windows'
`0xC000013A` is `-1073741510`; `pub fn exit_code_text(code: i64) -> String` renders it for a
person, a negative code followed by its hex (`-1073741510 (0xC000013A)`), as the Claude Code
and Codex handlers do in their exit reasons.

Which provider a harness runs in follows from what its handler declares
(`AgentHandler::stdio`), never from a provider name the Agent or handler gives:

```rust
pub trait Terminal {                          // kind ai6_terminal
    fn spawn(&self, spec: &Value) -> Result<Object<dyn TerminalSession>, ProviderError>;
    fn tty(&self) -> bool { true }            // appended slot: false when the program gets plain pipes (`pipe`, the detached wrapper)
}
pub enum HarnessStdio { Terminal /* default */, Json }  // crosses as "terminal" | "json"
pub const DEFAULT_TERMINAL: &str = "pty";     // preferred for a Terminal harness
pub const DEFAULT_PIPE: &str = "pipe";        // preferred for a Json harness
pub struct TerminalOffer { pub id: String, pub tty: bool }
#[non_exhaustive]
pub enum TerminalRefusal { NoneAvailable, NoPipe, PinnedHasTerminal { provider: String } } // Display: the reason
pub fn choose_terminal(stdio: HarnessStdio, offers: &[TerminalOffer], pinned: Option<&str>)
    -> Result<String, TerminalRefusal>;
```

`choose_terminal`: a pin is used as is, except that a `Json` harness pinned to a provider
offered with `tty: true` is `PinnedHasTerminal`; a pin not among `offers` is returned for
construction to report. Unpinned, `Json` gets `DEFAULT_PIPE`, else the first offer without a
terminal, else `NoPipe` (`NoneAvailable` with no offers); `Terminal` gets `DEFAULT_TERMINAL`,
else the first offer with a terminal, else the first offer, else `NoneAvailable`.

The daemon logs the handler and terminal provider of each harness it starts or resumes:
`ai6: Mission "<id>": started the <handler> harness in the <terminal> terminal` (`resumed`;
`runner Session "<key>" runs` when a field runner chose the terminal), and
`ai6: assistant conversation <digest>: …` for Moneypenny. A Mission's Session record also keeps
both, in `SessionLaunch { handler, terminal }`; an assistant Session has no record.

A handler whose harness writes one JSON value per line reads it with `JsonLines`, because a
pseudo-terminal is not a pipe: ConPTY on Windows renders a console, so its output carries
mode and title sequences, may move the cursor instead of writing a line break, and breaks a
line longer than the terminal is wide (CRLF, then a cursor move).

```rust
pub const STREAM_COLS: u16 = 32767;          // widest terminal every provider accepts (ConPTY: i16)
pub const MAX_JSON_LINE: usize = 64 << 20;   // a held, unfinished value past this is handed over as text
pub fn strip_controls(bytes: &[u8]) -> Vec<u8>; // drops CSI, OSC and two-byte escapes; keeps line breaks
pub enum StreamLine {
    Json { value: serde_json::Value, text: Vec<u8>,
           rebuilt: bool /* joined from a broken line, or a non-SGR control removed inside */ },
    Text(String),
}
pub struct JsonLines; // Default
impl JsonLines {
    pub fn new() -> Self;
    pub fn push(&mut self, bytes: &[u8]);               // output as read
    pub fn next_line(&mut self) -> Option<StreamLine>;   // a whole line, if one has arrived
    pub fn finish(&mut self) -> Option<StreamLine>;      // after end of output: what is held; call until None
}
```

Lines are cut at line breaks and at cursor moves to another row (CUP, HVP, CNL, CPL, VPA,
IND, NEL, RI), controls and CR removed. A CSI ends at its final byte, or before the first byte
no CSI holds (a line break included); past 64 parameter bytes it is no sequence and only its
`ESC [` goes. An OSC ends at BEL, `ESC \` or before a line break. A line starting with `{`
or `[` that does not parse yet is held, untrimmed, and joined with the lines after it; it is
parsed again only when a piece closes its outermost bracket (strings and escapes tracked).
A line that is a whole value of its own ends the held one as text. Anything else is `Text`,
trimmed, never empty. Handlers spawn with `cols: STREAM_COLS` so that few lines wrap at all.

## Mission lifecycle (`ai6::directorates::m::mission`)

`MissionStore` validates lifecycle transitions, associates provider `SessionId` values
with an AI6-owned `MissionId`, records safe-house disposition and optional `CoverReport`,
and increments a revision for changes. Repeating the current state is a no-op and does not
increment the revision. `MissionState` serializes as `queued`, `working`, `blocked`,
`stalled`, `parked`, `quarantined`, `done`, `failed`, `blown` or `abandoned`; `blown` means a cover policy
violation when the Operation uses `on_blown = "fail"`. `parked` means a provider usage limit
paused the work: `Mission.parked_until` (Unix seconds, `None` unless parked) is
when it resumes. `MissionStore::park(id, until)` parks a `Working`, `Blocked`, `Stalled` or
already parked Mission (parking again moves the time; `transition` refuses `Parked` because
it needs a time); `due_parked(now)` lists parked Missions at or past their time;
`requeue_for(id, agent_id)` queues an interrupted Mission again for another Agent.
`retry(id, agent_id: Option<String>)` queues a `Stalled`, `Done`, `Failed`, `Blown` or
`Abandoned` Mission again as a new attempt, for `agent_id` when given and otherwise its
Agent; it keeps `session_ids` and the safe house report, and refuses every other state
with `InvalidTransition`. A
parked Mission leaves `Parked` through `Working`, `Failed`, `Abandoned` or `requeue_for`,
and any transition clears `parked_until`.
`quarantined` means Medical or an operator stopped the work because its Agent is
quarantined: `MissionStore::quarantine(id)` moves a `Working`, `Blocked`, `Stalled` or
`Parked` Mission there (repeating it is a no-op), it leaves only through `requeue_for`
(back to `Queued`, for the next Agent) or `Abandoned`, and `reassign(id, agent)` gives a
queued Mission another Agent. `block_for(id, reason)` blocks an active Mission and records
`Mission.blocked_reason` (`"budget"`; cleared by the next state change); `expose(id, sources)`
records `Mission.exposure`, the untrusted input the Mission itself came from;
`record_provenance(id, provenance)` records `Mission.provenance` (`Option<Provenance>`, the
first record stands; `None` for a Mission started on the Station rather than from the queue).
`set_waiting(id, Option<String>) -> Result<bool, _>` records `Mission.waiting`, why a queued
Mission is held back (its Agent retired or quarantined with no other qualified Agent; `false`
when unchanged; cleared by `reassign`, `requeue_for` and every state change); the Station
also writes a `mission`/`waiting` timeline event once per new reason.
`MissionStore::set_cover_report(mission_id, report)` persists the cover audit. A Mission
returns this value as its optional `cover` field beside `safe_house`. `Mission.role_id`
stores the canonical configured Agent role independently of `agent_id` and provider
`session_ids`; selected handler/model/effort settings persist for reattachment.
`MissionStore::open(path)` persists a JSON snapshot atomically after
each mutation. The daemon opens `missions.json` under `safehouse::state_dir()`; this is
runtime Mission state, separate from the durable Registry. The snapshot
stores non-secret handler, terminal, work-directory, model and effort settings for each
started Mission (the terminal is the one the Station chose at start), including the cover policy, starting commit and identity; environment values
are never persisted. The snapshot format is version 1; any other version is refused. On
startup, `Working` Missions
become `Blocked` and the scheduler retries reattachment through the same provider Session ID.
The daemon's loopback
`/internal/missions` routes expose Mission state to local control components; Universal Exports
remains the authenticated public API.

## Operation cover (`ai6::directorates::m::cover`)

`Covers::default()` supplies `covert`, `partial` and `overt`. `Covers::load(config_dir)`
loads `covers.toml`, with custom entries replacing built-ins of the same name;
`parse_toml(source)` validates the same schema in memory. `get(name)` returns a policy or
`CoverError`; `names()` returns sorted names; `insert(cover)` adds one in code, returning
the policy it replaced. `Cover::new(name, description)` hides everything (no Agent author,
trailer, model name, footer or announcement); the `with_*` methods set each `covers.toml`
key, and `with_forbidden` fails on an invalid pattern.
`Cover::briefing(identity, agent, model)` creates the initial Agent instructions and
`check_commits(commits, identity, model)` returns ordered `CoverViolation` values for
identity, attribution trailer, model mention, announcement, forbidden-pattern and footer
rules. Forbidden patterns match case-insensitively. `CoverReport` records the policy,
number of commits, `checkable`, `blown` flag and violations; the daemon persists it to the
Mission after checking the safe house's changes since the Mission's starting revision. A
safe house whose provider keeps no history reports `checkable = false` (and `blown = false`):
that is "not checkable", not a pass.

```rust
pub struct CoverViolation { pub commit: String, pub rule: String, pub detail: String }
pub struct CoverReport { pub policy: String, pub commits: usize, pub checkable: bool, pub blown: bool, pub violations: Vec<CoverViolation> }
pub struct Covers;
impl Covers {
    pub fn load(config_dir: impl AsRef<Path>) -> Result<Self, CoverError>;
    pub fn parse_toml(source: &str) -> Result<Self, CoverError>;
    pub fn get(&self, name: &str) -> Result<&Cover, CoverError>;
    pub fn names(&self) -> Vec<&str>;
    pub fn insert(&mut self, cover: Cover) -> Option<Cover>;
}
impl Cover {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self;
    pub fn with_agent_author(self, allowed: bool) -> Self;       // agent_author
    pub fn with_trailers(self, trailers: impl IntoIterator<Item = impl Into<String>>) -> Self;
    pub fn with_suggestion(self, line: impl Into<String>) -> Self;  // suggest
    pub fn with_model(self, allowed: bool) -> Self;              // model
    pub fn with_footer(self, footer: impl Into<String>) -> Self;
    pub fn with_announce(self, allowed: bool) -> Self;           // announce
    pub fn with_forbidden(self, patterns: impl IntoIterator<Item = impl AsRef<str>>) -> Result<Self, CoverError>;
    pub fn name(&self) -> &str;
    pub fn disclosure(&self) -> Disclosure; // Hidden, Assisted or Full: what may be said outside the Operation
    pub fn briefing(&self, identity: Option<&CoverIdentity>, agent: &str, model: &str) -> String;
    pub fn check_commits(&self, commits: &[Change], identity: Option<&CoverIdentity>, model: &str) -> Vec<CoverViolation>;
}
```

## Station daemon (`ai6::station::server`)

`DEFAULT_MAX_CONCURRENT_MISSIONS` is `4`. Without a pin, a Station chooses a harness's
terminal provider with `Plugins::choose_terminal` and the handler's `stdio`; a refusal fails
the Mission's start with `400` and the reason, leaving it queued. `serve` and
`serve_with_plugins` use these defaults. The capacity-aware variants accept an explicit limit and reject zero:

```rust
serve_with_capacity(address: SocketAddr, max_concurrent_missions: usize, shutdown: F)
serve_with_plugins_and_capacity(address: SocketAddr, plugin_paths: &[PathBuf], max_concurrent_missions: usize, shutdown: F)
serve_with_config_dir_and_capacity(address: SocketAddr, plugin_paths: &[PathBuf], config_dir: &Path, max_concurrent_missions: usize, shutdown: F)
```

`ServeConfig::new(address)` defaults to the configured roster path, built-in Mission
capacity, `StationConfig::default()` and no Universal Exports publisher.
`with_plugin_paths`, `with_config_dir`, `with_config_source`, `with_registry`,
`with_station_id`, `with_station_config`, `with_max_concurrent_missions`, `with_watchdog`
and, with feature `publisher`, `with_publisher` set those options before calling
`serve_with_options`. The configuration directory (`with_config_dir`, default
`config_dir()`) holds the Station's Theaters, loadouts and Cable settings, and its roster
unless `with_config_source(Arc<dyn ConfigSource>)` names another source; the roster is
read once at start. `with_registry(Arc<dyn RegistryBackend>)` replaces the
`FileRegistry::from_environment()` default.

Each returns `std::io::Result<()>`; `F: Future<Output = ()> + Send + 'static`. The
background scheduler starts assigned queued Missions and leases file-backed Registry
briefs, resolving their Operation and Agent before creating durable Mission records.
Global, Station and Agent concurrency limits apply to Session starts; transient failures
retry after five seconds. Active leases renew every 30 seconds. Expired leases from other
Stations are recovered after three attempts, and invalid assignments or terminal Missions
are filed under `failed/` or `done/` with an outcome record (in a SQL Registry, as the
`failed` or `done` state of their `ai6_briefs` row). The scheduler's Registry calls run on
Tokio's blocking pool, so a slow Registry does not stall the async workers.

`pub async fn serve<F: Future<Output = ()> + Send + 'static>(address: SocketAddr,
shutdown: F) -> std::io::Result<()>` binds the local service without agent plugins.
`serve_with_plugins(address, plugin_paths, shutdown)` also scans configured Guatiao
library paths for `ai6_agent_handler` and `ai6_terminal` providers. The caller owns signal
handling; the binary passes Ctrl-C as the graceful-shutdown future. The service offers
`GET /healthz`, `GET /internal/agent-handlers`, `GET /internal/agent-roles`,
`GET /internal/terminals`,
`GET /internal/plugins`, `GET /internal/intake/briefs`, and local Mission lifecycle
routes under `/internal/missions`. The intake endpoint lists valid queued file briefs
without claiming them; the background scheduler owns leasing and execution. Each brief
gets a deterministic Mission ID in lease metadata, and renewal allows the same scheduler
to recover its mapping after restart. Unsupported or invalid assignments are filed as
failures.
`POST /internal/missions/{mission_id}/start` resolves the Mission's Agent from the roster
and starts the Agent's handler in the Station's terminal provider. Mission creation may
name an Agent directly or request a role and tags; AI6 selects the highest-priority
qualified Agent whose handler is available, respecting the Operation's roster and
clearance. AI6 drains provider events
in a background worker and updates Mission state independently of Universal Exports polling;
`GET .../events/next?timeout_ms=` reads from a bounded daemon event queue. The
`POST .../messages`, `POST .../interrupt` and `POST .../resume` routes operate the
provider Session. Resume passes the same provider-issued Session ID back to its handler
and rejects a handler that returns a different ID. After a process restart, resume rebuilds
handler settings from the persisted operation and current Agent roster. The optional
`environment` map in a resume request supplies fresh, non-persisted values such as new
short-lived credentials. Active safe houses are re-attached through their provider, which
verifies them against the Mission report (the `git` provider checks worktree, repository,
branch and base commit).
`GET /internal/missions/{mission_id}/cables` discovers grant-visible active peers;
`GET .../cables/inbox?limit=` reads authorized deliveries; `POST .../cables` sends a
Cable without caller-selected sender identity; `GET .../cables/held?limit=` lists body-free
held metadata under Release grants; `POST .../cables/{cable_id}/release` releases eligible
recipients; `POST .../cables/{cable_id}/ack` and
`POST .../cables/{cable_id}/reply` acknowledge or reply to a delivered Cable. The relay
snapshots only currently managed Sessions as recipients, and the internal API is loopback
only.
With `cables-mcp`, each managed Session receives its own loopback-only, random-token
authenticated Streamable HTTP MCP endpoint. Claude and Codex handlers receive the endpoint
URL and token through their launch environment; the bearer token is not placed in process
arguments or persisted. Tools expose peer discovery, scoped Cable send, inbox read,
scoped held-Cable listing/release, acknowledgement and linked replies. Held metadata omits
the body; an explicit Release grant is required and current Receive grants are checked
again before release. The endpoint and live Cable identity are removed when
the Agent operation closes. Missing Cable grants continue to deny every action.
The same endpoint offers `request_approval {kind, target, reason, idempotency_key}`, which
answers at once with `{request_id?, status, expires_at?, message?}`: `not_required` when the
Operation's green ink policy lets the Agent's clearance take the step, `approved` when a live
class authorisation covers it (never for a Session exposed under the `approvals`
restriction; the use is audited as `class_authorization.used` by `class:ID`), `rejected` or
`expired` for a decided request, otherwise `pending`. A pending request blocks the Mission
with `blocked_reason = "green-ink"` and pauses its Watchdog; terminal output and operator
messages do not end that wait, and a turn that completes while it lasts ends the Session
without finishing the Mission. Each scheduler pass delivers decisions from the Registry:
an `AgentMessage` to a live Session, or a resume whose message carries the decisions when
the Session ended, then the Mission returns to Working once nothing it asked for is
pending. Requests and deliveries are `green_ink` timeline events; the lifecycle carries the
reason (`green-ink`, `budget`) to Universal Exports.
The Station audits `green_ink.requested` (actor `agent:CODENAME`) for each new pending
request, `green_ink.expired` (actor `expiry`) when it tells the Agent a request expired, and
`class_authorization.used` (actor `class:ID`); each target is `REQUEST_ID KIND TARGET`, never
the reason.
Head of Station tools: a round (a Mission of role `head-of-station` whose brief, as this Station leased it, is this Station's
round of an authorized `each_station` Sleeper: the Registry kept `Origin::Sleeper` provenance
for it, its `data.sleeper` names this Station, the Mission's Operation and the Sleeper, that
Sleeper still exists with `each_station`, role `head-of-station` and the fingerprint the
provenance names, and its stored authorization has that fingerprint) also lists
`station_report`, `file_report`, `tidy_safe_house` and `provision_theater` on its endpoint,
whether started or resumed; any other Session's endpoint has no such tools, so a call is an
MCP error. Every call checks again that the Mission is not over and still such a round, so
revoking the Sleeper takes the tools away.
`station_report` answers JSON: `station`, `at`, `capacity`, `missions` not over (`mission`,
`operation`, `agent`, `role`, `state`, `session`), `disk` (`path`, `free_mb`, `total_mb` of the
state directory), `resources`, `theaters` (each published `TheaterSummary` plus `unfit`, why
placement would not run a Session there), `safe_houses` (each kept safe house still on disk:
`mission`, `operation`, `state`, `live`, `path`, `branch`, `size_bytes`, `size_complete` (false
past 500,000 entries), `age_secs` (since it or anything at its top level changed), `caches`
(`path` relative, `size_bytes`; outermost directories named in the hygiene list; `.git` is
never walked, links never followed), `exported` (nothing unexported, nothing uncommitted, and
every ignored path inside a cache directory), `unexported` and `ignored` (at most 20 each),
`dirty`, `note` when the provider cannot tell), `plugins_weakened` and `limits` (the cache list
and age and the unattended, green ink and never lists the Station enforces); no environment
value, token, loadout or brief text. Commit subjects and branch names in it were written by
other Missions' Agents, so a successful call marks the Session exposed (`STATION_REPORT`). `file_report {summary, findings: [{kind, severity, text}]}` files a `StationReport` (filed
by the Mission and its Agent) in the logbook, records a `head_of_station`/`reported` timeline
event, and sends it as a Cable to channel `heads-of-station` (`HEAD_OF_STATION_CHANNEL`) when
the Cable policy allows; it answers `{filed, at, cable}` where `cable` says `sent to N
recipients` or why not (an exposed Session sends none).
`tidy_safe_house {mission, action, reason?}` acts on the kept safe house of a Mission of this
Station that is over (terminal and no Session here; otherwise refused, as is a Mission this
Station does not keep, the round itself, or an action other than `caches` and `remove`), after
its provider re-attaches to it. The tidy holds the Mission's start reservation (`starting`)
from its first check to its last change, so a start, retry or resume of it is refused
meanwhile (and a tidy of a Mission being started is refused), and checks again that the
Mission is over right before removing anything. `caches` removes the outermost directories the
hygiene names once the safe house has been left alone for the cache age (else refused); with
a provider that answers `exports()`, only a directory listed in `ignored` (ignored whole, so
nothing in it is tracked) is removed and the rest are answered in `kept`. `remove` discards it
through the provider when `exports()` shows nothing unexported, nothing uncommitted and every
ignored path inside a cache directory; otherwise (or when `exports()` fails or is
unsupported) it asks for green ink with `Desk::request_always`, kind `SAFE_HOUSE_DELETE`
(`safe-house-delete`, compared as exact text), target `safe house MISSION on Station ID`,
filed under the Operation that owns the safe house's Mission and on that Mission (the round's
Session and Agent ask; idempotency key the target), so its policy and approvers decide, its
`unattended` list never skips the person and no class authorisation approves it. The
request does not block the round: any later call, from this round or a later one, finds a
pending or rejected request and answers it without asking again, or finds the live, unspent
approval, spends it and discards. A removal records `kept = false` on the Mission. Answers
`{mission, status, removed?, kept?, freed_bytes, green_ink?: {request_id, operation, status,
expires_at}}` with `status` `caches-removed`, `removed` or the green ink status. Audited with resource `station:ID`, actor `agent:CODENAME`:
`head_of_station.caches_removed` (target `MISSION PATHS`) and `head_of_station.removed`
(target `MISSION exported` or `MISSION approved by REQUEST`).
`provision_theater {theater, apply = false}` runs `q::provision::provision` on a Theater this
Station reaches with `check = !apply`, only the steps its `requires`, `provision` and
`runner` declare; with `apply` it then probes again and records the report
(`Transports::set_report`) and, when a step changed, audits `head_of_station.provisioned`
(target `THEATER STEP...`). Answers `{theater, applied, steps: [ProvisionStep], fit}`; a
failed step's `detail` never carries what the command printed. An ephemeral template, a
Theater made for a Mission and one without a transport are refused, and `apply` is refused
while a Mission of this Station that is not over has the Theater in its launch record.
`check_approval {steps: [{kind, target}, ..]}` (at most 64) answers the harness gate without
recording anything: a JSON array of `{kind, target, status, request_id?, message?}` with
`status` `not_required`, `approved` (a live approval of the Mission covers the step,
`ApprovalStore::live_approval`), `needs_approval` or `refused` (the Agent's clearance may not
ask).
Credentials: a Station with a loadout catalog (`loadouts.toml` in the configuration
directory, or `StationConfig::with_loadouts`) leases the Agent's loadout secrets at every
Session start and resume, last before the launch (one-hour lease, from
`with_secret_backend` or `StationSecrets` relative to the configuration directory, which
logs each env-sourced secret as a weakening at start) and puts each value in the start or
resume request's `environment` under its `env`, after removing the loadout's variables an
earlier Session held. A resume that re-attaches to a runner's live harness issues nothing.
A backend refusal of an ordinary secret fails the start, and leases acquired by a failed
issue are revoked. A `green_ink` secret is leased only while the Mission holds a live
approval (`ApprovalStore::mission_approvals`) of one of its `kinds` whose target matches one of
its `targets`, once per approval ever (`ApprovalStore::spend_approval`, so another Station or a
restart does not lease it again; a Station that loses the race revokes its lease), with the
approval's remaining time, at most an hour, as the TTL; otherwise its variable is absent. At
start the Station logs as weakenings a green ink secret without `targets` and one served by
its own backend (`StationSecrets`), whose values a revocation cannot take back. A start or
resume request's `environment` cannot set `AI6_GREEN_INK_GATE`. Each scheduler pass revokes, at the backend, every lease past its TTL
and each green ink lease whose approval has expired or been revoked (a
`credential`/`revoked` timeline event naming the secrets and the approval); an approval the
Registry cannot read keeps its lease until the next pass. An exposed Session under the `secrets` restriction gets none. Leases are
revoked when the Session's operation is finished, stopped, finalized or fenced out, on a
failed start or resume, and when a later issue for the Mission replaces them. Each issue is a
`credential`/`leased` timeline event (`secrets` = names, `approval` = request ID). Values are
never written: the launch record and publication carry variable names only, the detached
wrapper passes the environment through a FIFO, and a runner keeps it in memory.
Pool accounts: when the Agent's handler has pools (`choose_pools` with the Operation's
`pools` and the Agent's `pool`), loadout secrets whose `env` is a credential variable are not
issued (logged), and after the loadout the issue removes `CREDENTIAL_VARIABLES` and every
account's variable from the environment, claims an account (`AccountStore::claim_account`,
preferring the Mission's last Session's account; a resume of a Codex or `config-dir` Session
claims only that account, and a resume of an environment-kind Session only environment-kind
accounts) and hands it over: an env kind as a leased value, a Codex login
kind by `AccountLogin` into `<homes>/<mission>/<pool>--<account>` (with a `config.toml`
setting `cli_auth_credentials_store = "file"`; only when the harness runs on the Station's
own host), `config-dir` as its directory. A refused pool, no account with room or a failed
hand-over fails the start with the reason. The Session record carries the account; the
claim is released with the leases, renewed at each check-in, and homes of Missions that are
over are removed then. `account`/`claimed`, `account`/`limited` and `account`/`waiting`
timeline events carry names only. A resume that re-attaches to a runner's harness issues
nothing but claims the account the Session record names (`account`/`reclaimed`).
On graceful shutdown, event workers interrupt and close active Sessions and move Working
Missions to Blocked while preserving their safe houses; a Session on a runner is left
running and its Mission stays Working, so the next start re-attaches to it (see Field
runner below). Mission records persist across
daemon restarts, and a blocked Mission can reattach to its provider Session through its
handler. A parked Mission resumes through the same path once `parked_until` passes
(`POST .../resume` also resumes it early). Recovery is described under Session records and
Provider limits above. `ai6::directorates::m::roster`
loads global `operations.toml` and `agents.toml`; local `.ai6/agents.toml` is read only for
a registered Operation. Agent watchdog `limits` inherit and merge by field, then override the
daemon's `WatchdogConfig`. A local Operation whose `workspace` names a safe-house provider
(or whose path is a Git checkout, which selects `git`) gets an isolated safe house under
AI6's state directory. Untouched ones are removed when the Mission ends; ones holding
changes are kept and included in the Mission report. `workspace.isolate = false` works in
the Operation's directory instead. An unknown Operation key is refused at load time.
A Mission whose Operation is bound to a Theater that only a transport reaches runs on the
Station in remote-hands mode: the harness starts in `<state>/remote-hands/<mission>` (kept
when the Mission ends), has no safe house and so no cover check, and acts on the Theater
through the Theater tools. A `working_directory` is refused there. On a Theater with a
runner the harness runs under the runner (see Field runner below). On another POSIX host
with no runner, field mode runs the Station's handler with the detached wrapper as its
terminal (launch record `terminal_provider = "detached"`): the harness starts over the
transport under `setsid` (or `nohup`) in `~/.ai6/detached/<mission>-<id>/` (`env`, FIFO
`in`, `out`, `pid`, `exit`), reads resume at the last byte taken after a dropped link, there
is no pty (resize does nothing) and the host's login environment is kept with the
Session's added. A new start for the Mission ends a wrapper an earlier one left. A
Mission on another host works in the bound Operation's path or the Theater's `workdir`
(absolute; POSIX or Windows paths are accepted on any Station), with no safe house.
The Cable endpoint reaches a harness on another host through a reverse forward on the
Theater's loopback, re-opened on the same port when a read had to wait for the link.
This private listener is for
daemon health, provider discovery and Universal Exports-to-daemon operations; user-facing
Station and Mission operations belong to Universal Exports's authenticated API.

## Field runner (`ai6::station::runner`)

`ai6 runner serve` is the binary's Theater mode: it supervises harnesses on the Theater they
run on, so a Session outlives its Station. It loads AgentHandler and Terminal providers like a
Station and has no scheduler, Registry or roster.

```rust
pub const CALL_NOT_RUNNING: u8 = 3;            // exit status of `ai6 runner call` with no runner
#[non_exhaustive]
pub struct RunnerInfo { pub url: String, pub pid: u32, pub version: String }   // runner.json
impl RunnerInfo { pub fn read(state_dir: &Path) -> io::Result<Self>; }       // absent while none serves
#[non_exhaustive]
pub struct RunnerConfig { /* state_dir, listen (default 127.0.0.1:0), plugin_paths, terminal */ }
impl RunnerConfig {
    pub fn new(state_dir: impl Into<PathBuf>) -> Self;
    pub fn with_listen(self, address: SocketAddr) -> Self;   // loopback only: serve refuses any other
    pub fn with_plugin_paths(self, paths: impl IntoIterator<Item = PathBuf>) -> Self;
    pub fn with_terminal(self, terminal: impl Into<String>) -> Self;
}
pub fn default_state_dir() -> io::Result<PathBuf>;           // `<AI6 state>/runner`
pub async fn serve<F>(config: RunnerConfig, shutdown: F) -> io::Result<()>;  // F: Future<Output = ()> + Send + 'static
pub fn call(state_dir: &Path, method: &str, path: &str, body: Option<&serde_json::Value>)
    -> Result<(u16, serde_json::Value), String>;             // Err: no runner answers
pub fn spawn_detached(program: &Path, arguments: &[OsString], state_dir: &Path) -> io::Result<RunnerInfo>;
```

`serve` takes `runner.lock` (a second runner on the same directory gets `AddrInUse`), creates
`token` once (mode `0600` on Unix), writes `runner.json` once it listens and removes it on a
clean stop. Its API needs `Authorization: Bearer <token>` and speaks JSON: `GET /v1/health`,
`GET|POST /v1/sessions`, `GET /v1/sessions/{key}` (handler, terminal, `session_id`, `ended`,
`last`, `cursor`), `GET .../events?after=N&timeout_ms=T` (waits up to 30 s; answers
`{events: [{seq, event}], session_id, complete}`), `POST .../messages`, `POST .../interrupt`,
`POST .../close?after=N` and `PUT .../cable` (`{url, token}`). A start names a key (letters,
digits, `.`, `_`, `-`), the handler, an optional terminal and `{"start": AgentStart}` or
`{"resume": AgentResume}`.

Every event is appended to `sessions/<key>/events.jsonl` with its sequence number (from 1)
before any Station can read it. `after` acknowledges everything up to `N` and is stored in
`cursor`; a read without it starts from the cursor, so a Station that comes back replays what
it missed (at least once: the last event it took may come again). A `completed`, `failed`
or `limit` event ends the Session and stops the harness. A runner that restarts ends every
Session it had running with a `lost` event saying so (a Station gives such a Session up as
lost); ended Sessions older than seven days
are removed when a runner starts. When the start environment carries `AI6_CABLE_MCP_URL` and
`_TOKEN`, the runner starts a loopback relay with its own URL and token, gives the harness
those, and relays to the Station's endpoint, which `PUT .../cable` replaces; with none set it
answers 503. Stopping the runner stops its harnesses.

`call` reads `runner.json` and `token` and sends one request (`ai6 runner call METHOD PATH`
prints `{"status": N, "body": …}`, takes a body on standard input and exits
`CALL_NOT_RUNNING` when no runner answers). `spawn_detached` starts
`program runner serve <arguments>` in its own process group (Unix) or detached (Windows),
with output in `runner.log`, and answers once that process serves; on Windows it inherits
the caller's inheritable handles, such as an output pipe.

A Station uses the runner of the Theater a Session runs on when the Theater declares one
(`Theater::runner`). On its own host it reads `runner.json` and the token and calls the API
directly, starting the runner with `spawn_detached` (its own binary unless `command` is set)
when none answers. On another host it runs `<command> runner call [--state-dir D] METHOD
PATH` through the Theater's transport for every request (`command` defaults to `ai6`), and
`<command> runner serve --detach` with the settings' plugin paths and terminal when none
answers. A read that cannot reach the runner answers "no event yet", so a dropped link costs
only time; the Station re-opens its Cable forward and repoints the relay once a read gets
through again. Past the Operation's `lost_after_secs` out of reach it probes once more and,
if the runner is gone (or answers 404 for the Session), gives the Session up as lost (see
Recovery). The start records the runner Session's key in the launch record before the
harness starts. Shutdown and losing the Session lease let go without stopping the harness;
the Mission stays Working, so a restarted Station re-attaches to the same runner Session (a
parked Mission is resumed instead) and repoints the Cable relay at its own endpoint.
Closing the Session acknowledges what the Station took, so nothing is replayed after it.
Watchdog terminal-output checks (blocked prompt, loops) do not see a harness on a runner;
its events still count as activity.

## Universal Exports publication (`ai6::station::publisher`)

`Config::try_new(origin, token)` accepts a bare HTTPS origin or an HTTP loopback origin;
its `Debug` output redacts the token. `ServeConfig::with_publisher(config)` enables
publication through `serve_with_options`. The `ai6` command reads
`AI6_EXPORTS_URL` and `AI6_EXPORTS_STATION_TOKEN` together; leaving both unset disables
publication, and setting only one is a startup error. Redirects are disabled. A bounded
background thread orders Session registration, lifecycle updates and raw terminal chunks
on one event cursor. A separate authenticated long-poll receives validated Guatiao input
and resize frames from `/v1/station/inputs/next`; AI6 dispatches them only to the live terminal
bound to the matching Station, Mission and provider Session. The gateway's user-scoped
write grant and lease checks remain the authorization boundary. Requests time out after
two seconds; outbound writes are not retried, while input polling retries after errors.
A queue overflow or outbound gateway error disables publication and is reported by
`GET /internal/publisher`; it does not block Mission execution. Terminal chunks and the
pre-registration output buffer are each bounded to 1 MiB. Output read before the provider
Session ID is observed is queued until registration; overflowing that buffer disables
publication.

With a publisher, the Station also takes operator controls (cancel, retry, resume,
interrupt) queued for it: a third thread long-polls
`/v1/station/controls/next?station=<id>`, hands each DipTel control to the daemon one at a
time and posts the outcome to `/v1/station/controls/{id}/result`. The daemon applies it
only to a Mission in its own book under the control's Operation, while it holds that
Mission's Session lease with its current fencing token, and, when the control names a
provider Session, while that Session is still the Mission's latest; otherwise it rejects
with a reason. It acts through the loopback paths: `cancel` is the `abandoned` transition
(interrupts and closes the live Session, finishes the safe house keeping the work),
`retry` closes a live stalled Session, ends its Session record and calls
`MissionStore::retry` (an Agent must be configured for the Operation, qualified for the
Mission's role and not quarantined), `resume` is `/resume` with the message, and
`interrupt` is `/interrupt` on a working Session. When no live Session reported the new
state, the daemon publishes it as a lifecycle event on the Mission's latest provider
Session (`queued` after a retry, `abandoned` after a cancel). A provider's refusal is
reported as "the provider refused the request", never in the provider's words.

With a publisher, the Station is also an assistant desk for Moneypenny: a fourth thread
long-polls `/v1/station/assistant/next?station=<id>&wait_ms=1000` for `AssistantJob`s and
another posts `AssistantReport`s to `/v1/station/assistant/reports` (best effort, 256 queued,
three tries). For each job the desk answers `accepted` or `rejected` (invalid job, Agent not in
the roster or quarantined, handler not loaded here, four harnesses already running). It keeps
one harness per conversation, started with the roster Agent `job.agent_id` (handler, model,
effort; environment reduced to what reaches the model, as a Chief of Staff review's),
`AssistantMode::moneypenny()`, clearance `desk`, no project settings, in the conversation's
directory `<temp>/ai6-assistant-<random>/<conversation digest>` (the parent made once per
desk, readable by the Station's user only, beside `<temp>/ai6-assistant-<random>.lock`, which
the desk makes and locks before it makes the directory and keeps locked; a starting desk
removes every `<temp>/ai6-assistant-*` directory whose lock it can take, or which has none and
is an hour old, with the context files and git directories in it, and lock files an hour old
whose directory is gone and nobody holds; the same path on every
start of that conversation,
because Claude Code resumes a Session only from the directory that recorded it; removed when
the harness closes), and the MCP URL `<gateway origin>/v1/moneypenny/mcp` plus the job's token
in its environment. A later job goes to the running harness as a message, unless its token
expires within two minutes, when the harness is closed and resumed with the new token; a job
for a conversation whose harness is still closing waits up to ten seconds for it, then is
rejected. A harness idle for ten minutes is closed; the next job resumes its provider Session
(the job's `session_id`, else the one the desk remembers) with the Agent's context and the
job's `context` again as `AgentResume::initial_context`, or starts afresh with them and the
job's `recap` when that fails: when `resume` errs, or when the resumed harness fails (a
`failed` event or a read error) before its `session_id()` is known. Either failure is logged
(`ai6: assistant conversation <digest>: resuming provider Session <id> failed, …`) and not
reported to the conversation. Handler events `text`, `tool`, `completed`, `failed` and
`limit` are reported; `failed` and `limit` close the harness at once, freeing its slot, and
a harness busy answering that reports nothing for ten minutes is closed after a `failed`
event saying so. A closed harness reports `closed` with the reason (`idle`, `the harness
stopped`, `the Station is stopping`, …). An `interrupt` job calls the running harness's
`AgentSession::interrupt` while it is answering (nothing when it is idle or not running there;
`rejected` with the reason when the handler fails), and that turn's `completed` is reported as
`interrupted`; the harness stays up for the next message. Its directory is removed after `close`, tried again
for up to 2 s while Windows still holds it. Reports go in the order they were made, and a
message's answer comes first: from before the harness gets a message until the reports thread
has sent its `accepted` or `rejected`, the conversation's reports wait (at most 256), then
follow it. When Universal Exports refuses a job's `accepted` report with 403, 404 or 409 (the
job moved to another Station or was given up), the desk closes the harness it started for that
conversation, and for a message nothing more of that conversation is reported until the
harness has closed. An assistant Session is not a Mission: no
Operation, board entry or usage ledger record.

A job with a `workspace` gets, through the `git` safe-house provider (`SafeHouseSpec::clone`,
`CheckpointSpec::branch`): `personal/`, a clone of `workspace.personal` on `main` (a missing
local or `file://` repository is made bare first; a first use writes and commits
`memory/MEMORY.md`); `shared/<name>/` per shared repository, cloned afresh at each harness
start (one that cannot be opened is skipped with a `notice`); every git directory in
`<conversation digest>.git/{personal,shared/<name>}` beside the conversation's directory, so
none is inside it (`SafeHouseClone::git_dir`); `scratch/`; `CLAUDE.md` (the
desk's instructions, then `@personal/memory/MEMORY.md`) and `AGENTS.md` (the same with
`MEMORY.md` inlined, at most 32 KiB); and `<conversation digest>.context.md` beside the
directory with the `AGENTS.md` text. The harness's mode is `moneypenny()` with `files =
["personal", "scratch"]` and that `context_file`. At each turn's start the desk copies
`personal/memory/`; when the turn's `completed` arrives, and again after the harness closes,
it puts that copy back, reports what she changed there as one `memory_changes` event (before
`completed`; a change beyond the bounds, or a file that is not UTF-8, is dropped with a
`notice`), and commits the rest of `personal/` (`memory/` and paths with a name starting with
`.` left out: `CheckpointSpec::exclude`; nothing is saved when her memory could not be put
back, with a `notice`) to `main` as the person, message
`moneypenny: update <paths>` (or `save her work`) with the trailer `Via: Moneypenny
(conversation <id>)`; work `main` refuses twice lands on `moneypenny/<conversation digest>`
with a `notice`. A `commit` job of `COMMIT_MEMORY` for a conversation running here is written
into its `personal/` and copy and committed exactly (`CheckpointSpec::files`: nothing else a turn
in progress wrote goes with it); otherwise, and for `COMMIT_SHARED`,
the desk clones the commit's repository into a temporary directory, commits the files'
text there exactly without writing them, pushes, and removes it. The desk never reads, writes
or removes a file in the conversation's directory through a symbolic link, junction or other
reparse point (refused, or skipped when it walks a tree), and the provider checks links out as
plain files.

Exposure is Universal Exports' record (`Conversation.exposed`); the desk reads it with `GET
/v1/station/assistant/exposure?station&conversation` (`{"exposed"}`) when a turn ends and
raises it with `POST` (`{"station","conversation","source"}`) for what only it sees
(shared repositories are trusted text and expose nothing): a harness call that can answer
with a file `.moneypenny/outside-text/` in the personal repository names exposes the turn at
that call (source `personal:<path> (read by <tool>)`): a `Read` or `Edit` of it, a `Grep` or
`Glob` whose search covers it (paths compared after `.`/`..`, the filesystem's own spelling
and case), or a call whose input (`AgentEvent::data`) the desk cannot read. A
`memory/MEMORY.md` importing one (`@path`, followed five deep) exposes the conversation as
its harness starts. Other reads do not. The record is a directory: one entry per file, holding
the path and named by the Git object ID of that text. A save of an exposed conversation
records each file it adds or changes in the same commit (`CheckpointSpec::record`, the paths
taken from what is committed); a turn's end removes the entries of files no longer there
(`CheckpointSpec::files`, those entries only). So conversations adding to it at once never
replace each other's entries, and a file is never on `main` without its entry; the desk reads
the record again at each turn's start and after each save. A failed `GET` counts as exposed; a memory change of a turn the desk found exposed
is dropped with a `notice` when the `POST` fails.

A job with a `workspace` or a `commit` must name `person_id`, and the Station refuses one whose
repositories its own roster does not give: the personal repository must be
`personal_repo_for(person_id)` of its `[assistant]`, each shared one the repository its roster
gives that name (`org` or a group), and a commit's repository the person's (memory) or one
of the shared ones.

On a Station running elevated (`ai6::station::elevation::is_elevated() -> io::Result<bool>`,
feature `daemon`: root on Unix, a High or
System mandatory level from `%SystemRoot%\System32\whoami.exe /groups` on Windows; asked once,
an error counts as elevated) every job is `rejected` naming `assistant.allow_elevated` unless
the roster sets it; with it set, each harness start logs `ai6: weakening:
assistant.allow_elevated = true: …`. The roster's assistant weakenings
(`AssistantSettings::weakenings`, plus this Station running elevated when allowed) are logged
at start and on a roster change, and published at check-in as `assistant_weakened`.

With `cables-mcp`, a job whose `cables` is not empty also gets the conversation a Cable
endpoint (loopback, its own bearer token, accepted until the job's `token_expires_at`) as a
further server, `mode.with_cables()`, with `AI6_ASSISTANT_CABLE_MCP_URL`/`_TOKEN` in the
harness environment. Its tools are `discover_agents`, `send_cable`, `read_cables`,
`acknowledge_cable` and `reply_to_cable`, each naming an `operation`. Each call is made as the
principal (Agent `job.agent_id`, the call's Operation, this Station, Session
`assistant-<conversation digest>`, no Mission) `with_on_behalf_of(job.person_id)`;
`cables.toml` grants for that Agent, the quotas and fan-out apply as for any Agent. Before
every call the desk asks the same `GET` with `&operation=<id>`, which then also answers `send`
and `read`, the person's `cable.send` and `cable.read` there now; without an answer the call
is refused. A send or reply needs the job grant's `send` and `send` now, a read or
acknowledgement `read` in both, discovery either; otherwise the answer is the relay's own
indistinguishable refusal. Reads use `inbox_from` the call's Operation; a reply or
acknowledgement only touches a Cable of that Operation delivered to the conversation. Retry
keys are scoped to the conversation. The Session is in the relay's endpoint directory,
conversation-only, while the harness runs, so replies to its Cables reach it and nothing else
does. A later job's grants replace the endpoint's; exposure only turns on. A read that returns
a body first raises the conversation's exposure with the same `POST` (source
`cable:<sender>`, one per sender) and withholds every body ("Cable inbox is unavailable")
unless each is acknowledged. Exposed, `send_cable` and `reply_to_cable` send nothing: the
request (refused if its body holds a character `vetting::is_invisible` names) is held in
memory for an hour, at most 32 per conversation (the 33rd is refused, none is dropped), a
`cable_proposal` event carries it as an `AssistantCable` (desk ID, Operation, target,
recipient, TTL, body, `request_digest`), and she is told it waits for the person. A
`cable_confirm` job is `rejected` once its `token_expires_at` has passed; otherwise it sends the
held request when its digest matches, it is under an hour old and the endpoint's current
grants still give `send`, and is `rejected` with the reason otherwise or when the harness has
closed (held requests never survive it). A `cable_discard` job drops one. Every Cable sent for
the person is reported as `cable_sent`.

## Safe houses (`ai6::directorates::q::safehouse`, `ai6::station::safehouse`)

A safe house is the isolated working directory a Mission's Agent works in. Providers are
Guatiao libraries of two kinds; the Station loads them where the Session runs, and the
daemon runs no version-control command itself. Slot order is the ABI: append, never reorder.

```rust
#[guatiao::kind(name = "ai6_safehouse")]
pub trait SafeHouseProvider: Send + Sync {
    fn open(&self, spec: &Value) -> Result<Object<dyn SafeHouseSession>, ProviderError>; // SafeHouseSpec
}
#[guatiao::kind(object, name = "ai6_safehouse_session")]
pub trait SafeHouseSession: Send + Sync {
    fn info(&self) -> Result<Value, ProviderError>;    // SafeHouseInfo
    fn changes(&self) -> Result<Value, ProviderError>; // ChangeLog, newest first; empty without history
    fn finish(&self) -> Result<Value, ProviderError>;  // SafeHouseReport; idempotent
    /// Optional (default GUATIAO_ERR_UNSUPPORTED): CheckpointSpec in, SafeHouseCheckpoint out.
    /// Saves the work so far where another Station can fetch it, leaving the worktree alone.
    fn checkpoint(&self, spec: &Value) -> Result<Value, ProviderError>;
    /// Optional (default GUATIAO_ERR_UNSUPPORTED): SafeHouseExports out, what of the work
    /// exists only here. Unsupported reads as "all of it".
    fn exports(&self) -> Result<Value, ProviderError>;
    /// Optional (default GUATIAO_ERR_UNSUPPORTED): removes an isolated safe house whatever it
    /// holds (directory, and the Mission's branch); in place it is refused.
    fn discard(&self) -> Result<(), ProviderError>;
}
pub struct SafeHouseExports {
    pub unexported: Vec<String>, // "<short id> <subject>" of changes since the base no remote holds
    pub dirty: bool,             // uncommitted changes, untracked files included
    pub ignored: Vec<String>,    // relative paths version control ignores; a directory ignored whole is one entry
}
impl SafeHouseExports {
    // nothing unexported, not dirty, and every ignored path is one `rebuildable` accepts
    pub fn is_exported(&self, rebuildable: impl Fn(&str) -> bool) -> bool;
}
pub struct SafeHouseSpec {
    pub operation_id: String, pub mission_id: String, pub title: String,
    pub source: String,    // a local path; with `clone`, a repository URL or path
    pub base: String,      // revision to start from; ignored by providers without revisions
    pub branch: String,    // template: {id} {slug} {operation}
    pub isolate: bool,     // false: work in `source` itself
    pub state_root: String,
    pub reopen: Option<SafeHouseReport>, // re-attach to a safe house kept by an earlier run
    pub checkpoint: Option<SafeHouseCheckpoint>, // start a new one from a lost attempt's work
    pub clone: Option<SafeHouseClone>, // a clone of `source` on branch `base` instead; overrides the rest
}
pub struct SafeHouseClone {
    pub path: String,          // the working directory; must not exist
    pub create_source: bool,   // first make a missing local/file:// `source` an empty bare repository
    pub git_dir: Option<String>, // the git directory here instead of `.git` in `path`; none left there
}
pub struct CheckpointSpec {
    pub remote: String,    // Git remote name or URL
    pub message: String,   // already under the Operation's cover
    pub author_name: Option<String>, pub author_email: Option<String>, // the cover identity
    pub branch: Option<String>,   // commit in place and fast-forward this branch instead (a clone)
    pub fallback: Option<String>, // with `branch`: where the work goes when that branch refuses it
    pub files: Option<Vec<CheckpointFile>>, // with `branch`: commit exactly these, nothing of the working directory
    pub record: Option<String>, // with `branch`, no `files`: a directory in which the same commit records each file it adds or changes
    pub exclude: Option<Vec<String>>, // with `branch`, no `files`: paths left out with everything under them
}
pub struct CheckpointFile { pub path: String /* repository_path */, pub text: Option<String> /* None removes */,
    pub base: Option<String> /* hex SHA-256 it must hold on the remote branch, "" absent; None unchecked */ }
pub struct SafeHouseCheckpoint { pub remote: String, pub branch: String, pub commit: String, pub base: Option<String> }
pub struct SafeHouseInfo {
    pub path: String, pub branch: Option<String>, pub base: Option<String>,
    pub supports_history: bool, // false: cover is "not checkable"
    pub isolated: bool,
}
pub struct Change { pub id: String, pub author: String, pub committer: String, pub message: String }
pub struct ChangeLog { pub changes: Vec<Change> }
pub struct SafeHouseReport { pub path: String, pub branch: Option<String>, pub base: Option<String>, pub kept: bool, pub changes: Vec<String> }
```

`finish` removes an untouched isolated safe house and keeps one that holds work; it leaves
a safe house in place alone. Floor hashes are pinned by tests in `safehouse.rs`; a model
grows by optional fields only. Providers: `git` (`ai6-safehouse-git`) and `copy` (`ai6-safehouse-copy`, no history).

`station::safehouse::SafeHouse::open(&dyn SafeHouseProvider, &SafeHouseSpec)` wraps a
session: `path()`, `branch()`, `base()`, `supports_history()`, `is_isolated()`,
`changes()`, `finish()`, `checkpoint(&CheckpointSpec)`, `exports() -> Result<Option<SafeHouseExports>, _>`
(`None` when the provider cannot tell), `discard()` and `active_report()`.
`SafeHouseHygiene` (`Default`: `DEFAULT_CACHE_DIRS` = `target`, `node_modules`, `.dart_tool`
after `DEFAULT_CACHE_AGE`, 7 days) has `with_caches(names) -> Result<Self, SafeHouseError>`
(plain directory names only: not empty, `.`, `..`, `.git`, nor holding `/`, `\` or `:`),
`with_cache_age(Duration)`, `caches()`, `cache_age()` and `is_cache(name)`. The Station
offers a lost attempt's checkpoint only when it opens a new isolated safe house. Only isolated safe houses are recorded on
the Mission and finished by the daemon. `state_dir()` resolves `AI6_STATE`, then
`AI6_HOME/state`, then `%LOCALAPPDATA%/AI6` on Windows, then `XDG_STATE_HOME/AI6` or
`~/.local/state/AI6`.

## Plugin discovery (`ai6::station::plugins`)

`Plugins::scan(paths: &[PathBuf]) -> Plugins` filters Guatiao libraries for
`ai6_agent_handler`, `ai6_terminal`, `ai6_safehouse`, `ai6_source` and the three access kinds
before mapping them and retains the registry for the daemon process lifetime. `handlers()`,
`terminals()`, `safehouses()`, `sources()`, `authenticators()`, `directories()` and
`authorizers()` list providers and their availability. `handler(provider, config)`,
`terminal(provider, config)`, `safehouse(provider, config)`, `source(provider, config)`,
`authenticator(provider, config)`, `directory(provider, config)` and
`authorizer(provider, config)` construct typed providers, validating a supplied `Value`
against the provider's schema. `None` builds a provider's default instance, or the provider
with every setting at its default (the `claude` and `codex` handlers); a provider with a
required setting answers `PluginBuildError::ConfigurationRequired`.
`scan_summary()` reports loaded, skipped, failed and unreadable paths.
`terminal_offers()` lists the available terminal providers that build with their defaults,
each with its `Terminal::tty`, and `choose_terminal(stdio, pinned)` applies `choose_terminal`
(see the terminal API) to them; a Station and a runner choose a harness's terminal with it.

```rust
impl Plugins { pub fn scan_with(paths: &[PathBuf], policy: &PluginPolicy) -> Plugins; } // scan() = default policy
impl Plugins {
    pub fn terminal_offers(&self) -> Vec<TerminalOffer>;
    pub fn choose_terminal(&self, stdio: HarnessStdio, pinned: Option<&str>)
        -> Result<String, TerminalRefusal>;
}
pub struct PluginPolicy;   // Default: report, no pins
impl PluginPolicy {
    pub fn new(pins: Option<PinSet>, verified_dir: impl Into<PathBuf>) -> Self; // profile: pin set's, else strict with pins, report without
    pub fn load(pins: Option<&Path>, default_pins: Option<&Path>, profile: Option<PluginProfile>,
                verified_dir: impl Into<PathBuf>) -> io::Result<Self>; // a named pin set must exist
    pub fn with_profile(self, profile: PluginProfile) -> Self;
    pub fn profile(&self) -> PluginProfile;  pub fn pins(&self) -> &PinSet;
}
pub struct PluginScanSummary { pub profile: String, pub loaded, skipped, failed, unreadable: Vec<String>,
    pub checked: Vec<LibraryCheck>, pub refused: Vec<String>, pub weakened: Vec<String> }
pub struct LibraryCheck { pub path: String, pub digest: String /* sha256:HEX */,
    pub verdict: String /* accepted: ... | unpinned | refused: ... */, pub loaded_from: Option<String> }
pub fn pin_entry(path: &Path) -> io::Result<LibraryPin>;  // the digest entry for an AI6 provider library
pub fn log_scan(summary: &PluginScanSummary);           // refusals and weakenings to stderr

// ai6::directorates::registry::bona_fides
pub enum PluginProfile { Strict, Report, Off }           // lowercase; FromStr; default_for(pins: bool)
pub struct PublisherKey;                                 // "ed25519:" + 64 hex; from_bytes, as_bytes
pub enum Require { Either /* default */, Both }
pub struct LibraryPin { pub file: String, pub pin: Option<coqui_peer::PeerId>, pub publisher: Option<PublisherKey>, pub require: Require }
impl LibraryPin { digest(file, pin) / publisher(file, key) / with_pin / with_publisher / requiring_both }
pub struct PinSet { pub profile: Option<PluginProfile>, pub libraries: Vec<LibraryPin> /* [[library]] */ }
impl PinSet {
    pub fn new() -> Self;  with_profile;  with_library;
    pub fn from_toml(text: &str) -> Result<Self, String>;
    pub fn load(path: &Path) -> io::Result<Option<Self>>;   // None when the file is absent
    pub fn validate(&self) -> Result<(), String>;  pub fn weakenings(&self) -> Vec<String>; // SHA-1 pins
    pub fn check(&self, file: &str, bytes: &[u8], signature: Option<&[u8]>,
                 verify: &SignatureCheck<'_>) -> PinCheck;
}
pub type SignatureCheck<'a> = dyn Fn(&PublisherKey, &[u8], &[u8]) -> bool + 'a; // (key, message, signature)
pub const PUBLISHER_MESSAGE_TAG: &[u8];                  // b"ai6-provider-library-v1\0"
pub fn publisher_message(file: &str, digest: &[u8; 32]) -> Vec<u8>; // tag || file || NUL || sha256
pub enum PinCheck { Accepted(String), Unpinned, Refused(String) }
pub fn library_pin(bytes: &[u8]) -> coqui_peer::PeerId;  // sha256:HEX
```

Only files that declare an AI6 provider kind are checked. Under `strict` each is read once,
checked against the pin set by its file name, and handed to Guatiao as a copy
`verified_dir/sha256-HEX.EXT`; anything else is skipped without reaching the loader. A
publisher entry needs `FILE.sig` beside the library: the 64-byte Ed25519 signature of
`publisher_message(FILE, sha256)`, so a signature is valid only under the file name it was
made for. A pin covers the library file only: a non-system library it imports is found by
the operating system's loader (on Windows also beside the executable, in the current
directory and on `PATH`). Under `report` libraries load from where they were found and each
unaccepted one is a `weakened` line; `off` checks nothing. The Station's check-in carries
`plugins_weakened` when the list is not empty.

## Command line

- `ai6 serve [--listen 127.0.0.1:7081] [--station ID] [--role control|execution]... [--label KEY=VALUE]... [--terminal ID] [--config-dir PATH] [--plugin-path PATH]... [--plugin-pins PATH] [--plugin-profile strict|report|off]`
  (feature `cli`): run the Station and load Operation and Agent configuration. The pin set
  defaults to `plugins.toml` in the configuration directory; verified copies go to the state
  directory's `plugins/verified/`.
  `--terminal` pins the terminal provider (refused at Mission start for a JSON harness when
  it gives a terminal); without it an available one that suits the handler is used: `pipe`
  preferred for a JSON harness, `pty` otherwise. It accepts
  `--max-concurrent-missions 4`, `--stall-timeout 300`, `--loop-timeout 300`,
  `--loop-min-lines 50`, `--blocked-after 10`, optional `--max-runtime`, `--history 4096`
  and `--normalize-digits`. Station roles default to control and execution; repeated `--role`
  values replace that default. `--label` adds a placement label. `--resources-every 60`
  sets the seconds between resource samples of each remote Theater (`0` leaves only the
  300-second probe; `StationConfig::with_resources_every`, `DEFAULT_RESOURCES_EVERY`).
  `--config-poll 5` sets the seconds between checks of the roster source and
  `loadouts.toml` (`0` off; `StationConfig::with_config_poll`, `DEFAULT_CONFIG_POLL`).
  `--safe-house-cache NAME` (repeatable, replacing `DEFAULT_CACHE_DIRS`) and
  `--safe-house-cache-age 604800` set `StationConfig::with_safe_house_hygiene`; an invalid
  name exits 1. Timeout values are
  non-negative seconds and may be fractional. `AI6_CONFIG_DIR` overrides the default
  config directory.
- `ai6 runner serve [--state-dir PATH] [--listen ADDR] [--plugin-path PATH]... [--plugin-pins PATH] [--plugin-profile P] [--terminal ID] [--detach]`:
  run the field runner on this host until Ctrl-C or SIGTERM (provider libraries checked as
  the Station's, verified copies under its state directory); `--detach` starts it in the
  background and prints its `runner.json`. `ai6 runner call [--state-dir PATH] METHOD PATH`
  sends one request with a body from standard input and prints `{"status", "body"}`; exit 3
  means no runner answered.
- `ai6 green-ink gate`: Claude Code's `PreToolUse` hook for shell commands (see Harness
  gate); the Claude handler passes it through `--settings`. Always exits 0.
- `ai6 plugins pin PATH...` prints the `plugins.toml` entries that pin AI6 provider
  libraries by SHA-256; anything else is refused with a non-zero exit. `--plugin-pins PATH`
  on any command must name an existing file; only the default `plugins.toml` may be absent.
- `ai6 harness pin DIR` prints a working directory's harness manifest (`# sha256:..  PATH`
  per file) and the `pin = "sha256:..."` line for `[operation.ID.harness]`.
- `ai6 theater provision` also takes `--plugin-pins` and `--plugin-profile`.
- `ai6 identity` creates or prints this Station's certificate fingerprint.
- `ai6 peer waiting` lists rejected peer certificates; `ai6 peer accept <ID>` pins one.
- `ai6 sleeper [--config-dir PATH] list|authorize <OPERATION> <NAME>|revoke <OPERATION> <NAME>`:
  show and approve an Operation's standing orders. It reads the Registry from
  `AI6_REGISTRY` or the platform state directory, like the Station.
- `ai6 quarantine list|add <AGENT> <REASON> [--operation ID | --all]|release <AGENT> (--operation ID | --all)`:
  stop an Agent, or lift that, through the Registry; a Station applies the change within a
  scheduler tick.
- User-facing management goes through Universal Exports and the separate `ai6ctl` command.
