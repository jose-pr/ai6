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
| `acp` | The Agent Client Protocol wire (`section_00::acp`): `AcpSession`, `AcpFacade`; part of `daemon`. Adds `agent-client-protocol`, `coqui-acp` and `futures`; no async runtime. | No |
| `gate` | The green ink gate's client of a Station (`section_00::gate`), for agent providers; part of `daemon`. | No |
| `station-tls` | Persistent Station certificate identities and peer/CA mutual TLS (`coqui-peer-tls`). | No |
| `sqlite` | `SqlRegistry` and `SqlProvider` in a SQLite file. | No |
| `postgres` | `SqlRegistry` and `SqlProvider` in PostgreSQL, shared by the Stations of an HA deployment. | No |
| `cli` | The `ai6` command with all of the above and Ctrl-C handling. | No |

MSRV: Rust 1.89. The crate depends on Guatiao, `coqui-access`, `coqui-roles`, `coqui-peer` (digest text for pins), serde, toml, fs2, sha2, lru, zeroize, and `rustls-pki-types` and `rustls-webpki` (no crypto provider) to read a `ca_file`.
`daemon` adds Axum, Tokio, Guatiao provider loading, `ureq`, `subtle`, `futures-util` and `ring` (Ed25519 publisher signatures),
`publisher` adds `url` and `webpki-root-certs`, and `station-tls` adds `coqui-peer-tls` and rustls.
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
node that runs them: `station::server` (daemon service and `serve*`), `station::limits` (the
`station` record's tables), `station::plugins`
(provider host), `station::safehouse` (opens a Mission's safe house through a provider),
`station::runner` (the field runner), `station::publisher` (`publisher` feature) and
`station::identity` (`station-tls` feature). The command exposes `ai6 serve`,
`ai6 runner serve|connect`, `ai6 identity`, `ai6 peer waiting|accept` and `ai6 config path`.
Each reads the bootstrap file `ai6.cfg` (`ai6::bootstrap`, below; `--config PATH`). `ai6
serve --provider LOCATION` and `--registry LOCATION` take a directory, `sqlite:PATH` or a
`postgres://` URL over its `[provider]` and `[registry] location`; `[registry]
headquarters_lease` (default 60) sets `StationConfig::with_headquarters_lease` and `[provider]
poll` (default 5, 0 off) `StationConfig::with_config_poll`.

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
pub const SCOPE_KINDS: [&str; 5] = ["station", "operation", "theater", "config", "compartment"];
                              // "config:roster" is the roster every Station reads,
                              // "config:assistant" Moneypenny's repositories,
                              // CONFIG_ACCESS ("config:access") the global access policy,
                              // CONFIG_ACCOUNTS ("config:accounts") the accounts
pub const ACTIONS: &[&str];   // sorted: access.manage, access.read, account.manage,
                              // agent.configure, agent.read, approval.class,
                              // approval.decide, assistant.memory.admin, audit.read,
                              // cable.read, cable.send, compartment.cover, compartment.manage,
                              // compartment.read, mission.control, mission.create, mission.read, mission.review,
                              // mission.status, mission.verify, pool.manage, pool.read, product.read, roster.manage,
                              // roster.read, session.input, session.read,
                              // station.configure, station.read; cable.* on operation:<id>
                              // bound Moneypenny's Cables for the person;
                              // assistant.memory.admin on config:assistant reads another
                              // person's memory; access.read / access.manage read and change
                              // a policy at config:access (global) or operation:<id>
pub const STATION_ACTIONS: &[&str];     // those that mean something at station:<id>: audit.read,
                              // station.configure, station.read
pub const OPERATION_ACTIONS: &[&str];   // the ACTIONS that mean something on an Operation:
                              // agent.*, approval.*, cable.*, mission.*, pool.*, session.*,
                              // audit.read, access.read, access.manage, compartment.cover,
                              // product.read. A role bound at an
                              // `operation:` scope grants only these there
pub fn role_authorizer() -> RoleAuthorizerBuilder;   // builder limited to SCOPE_KINDS
pub fn access_from_toml(source: &str, max_bytes: Limit) -> Result<EffectiveAccess, AccessPolicyError>;
pub fn load_access(path: impl AsRef<Path>, max_bytes: Limit) -> Result<EffectiveAccess, AccessPolicyError>;
pub fn load_policy(path: impl AsRef<Path>, max_bytes: Limit) -> Result<AccessPolicy, AccessPolicyError>;
#[non_exhaustive] pub struct AccessPolicyError;   // Display, Error
```

### The access policy

`AccessPolicy` is the record `access`: `roles: BTreeMap<String, RoleDef { actions,
description? }>`, `rules: Vec<RuleDef { subject, scope, role, propagate, editor?,
changed_unix? }>` (order is meaning), `delegable: Option<Vec<String>>` (`None` is
`default_delegable()`: every Operation action but `access.manage`, `approval.class` and
`compartment.cover`; a compartment grant never counts towards what an editor may hand out),
`chief: Vec<String>` (subjects) and `limits`. `from_toml`,
`from_json`, `to_json`; the file form is `[role.<name>]` / `[[rule]]` plus top-level `chief`
and `delegable`. `seeded_policy()` is what a first start writes: `controller`, `operator`
and `auditor` (the table is pinned by a test), ordinary data, never written again.

```rust
pub fn compile(policy: &AccessPolicy, chief_from_host: &[String])
    -> Result<EffectiveAccess, AccessPolicyError>;
impl Authorizer for EffectiveAccess;   // allowed when any tier allows
impl EffectiveAccess {
    pub fn explain(&self, request: &AuthorizationRequest) -> Result<Explanation, ProviderError>;
    pub fn holds(&self, person: &Person, action: &str, scope: &str, lineage: &Lineage) -> bool;
    pub fn is_chief(&self, person: &Person) -> bool;
    pub fn host_chiefs(&self) -> &[String];
}
pub struct Explanation { allowed, tier: Option<TierKind>, rule_index, role }
pub enum TierKind { Chief, Global, Operation(String) }
pub fn check_change(before: &AccessPolicy, after: &AccessPolicy, editor: &Editor,
    effective: &EffectiveAccess, lineage: &Lineage) -> Result<(), Refusal>;
pub struct Editor { person, chief }   // Editor::new(person, &effective)
pub struct Refusal { pub reason: String }
pub struct Lineage;   // each Operation's ancestors, nearest first (from_roster, new)
```

- Tiers: the chief list and the global policy compile into one first-match tier with the chief
  rules first; each Operation's own policy is a tier of its own. A request is allowed when
  any tier allows, so an Operation policy only adds and no stored rule shadows a chief.
- `chief` is a reserved role of every action in `ACTIONS` (computed when compiled, so a later
  action is held), bound only at `*` through the chief list (`AccessPolicy::chief` plus the
  host's subjects). A stored role or rule naming `chief`, and a role name with `@`, are
  refused. A role bound at an `operation:` scope is compiled to a copy holding only its
  `OPERATION_ACTIONS`; a copy with none keeps its place in the order, so the first match still
  decides.
- `check_change` refuses: an editor without `access.manage` at `config:access`; a changed
  `chief` list by a non-chief; a result with no chief (stored plus host); an action added to
  a role or by a rule the editor does not hold at that rule's scope and, when the rule propagates,
  at every Operation under it (at `*` for a role no rule
  binds; chiefs hold everything); a `delegable` entry that is not
  an Operation action or that the editor does not hold at `*`. Removals and reorders that add
  nothing pass.

`access_from_toml` reads the file format into an `EffectiveAccess` with no host chief. A
subject is `account:<name>`, `person:<id>` or `group:<name>`; a scope is `*` or one of
`SCOPE_KINDS` followed by `:<id>`. No matching rule is deny, and the first matching rule is
decisive even when its role lacks the action. A request names the resource's ancestors
(`AuthorizationRequest::ancestors`, nearest first: Universal Exports fills them for an
Operation from the Stations' `operation_lineage`), and a rule whose scope is among them
matches too unless it sets `propagate = false`; a child Operation is narrowed by a rule placed
before its parent's. `[limits] ancestors` (a number or `"none"`, default 64) bounds how many
a request names; more is an error, never a denial. The file is read up to `max_bytes`
(`[provider] access_policy_bytes`, 1 MiB by default). The Universal Exports gateway loads and
enforces this policy for user requests.

### Operation policies

`OperationAccess { roles: BTreeMap<String, RoleDef>, rules: Vec<RuleDef> }` is the record
`operation_access` of one Operation (a keyed family, global only, live in the gateway; the
file `operation_access` holds `[operation_access.<id>]` tables). Its tier only adds to the
global one: a request on `operation:Y` is allowed when the global tier, Y's tier or any
ancestor's allows it, whatever order the rules are in elsewhere.

```rust
pub struct AccessSet<'a> { global: &'a AccessPolicy, operations: &'a BTreeMap<String, OperationAccess>,
    lineage: &'a Lineage }
pub fn validate_operation_access(operation: &str, record: &OperationAccess, set: &AccessSet)
    -> Result<(), Refusal>;
pub fn check_operation_change(operation: &str, before: &OperationAccess, after: &OperationAccess,
    editor: &Editor, effective: &EffectiveAccess, set: &AccessSet) -> Result<(), Refusal>;
pub fn compile_with_operations(policy: &AccessPolicy, chief_from_host: &[String],
    operations: &BTreeMap<String, OperationAccess>, lineage: &Lineage)
    -> Result<EffectiveAccess, AccessPolicyError>;
impl EffectiveAccess { pub fn orphaned(&self) -> &BTreeMap<String, Vec<Orphan>>; }
pub struct Orphan { pub item: String, pub reason: String }   // "rule 2", "role x", "record"
pub const RESERVED_ROLES: [&str; 4];   // chief, controller, operator, auditor
```

- A record's roles hold only Operation actions within the global cap (`delegable`, default
  `default_delegable()`), and may not be named `chief`, `controller`, `operator` or `auditor`.
  A rule's scope is `operation:<id>` for the Operation or one under it (never `*`, `station:`
  or `config:`). A rule's role resolves nearest first: the record's own roles, then each
  ancestor's, then the global roles through their Operation actions, which must all be within
  the cap: the seeded `controller` (it holds `access.manage` and `approval.class`) cannot be
  bound from an Operation record under the default cap.
- `compile_with_operations` skips what no longer validates (the tree moved, a role or the cap
  changed) and lists it in `orphaned`; the rest of the record stays in force. A record for an
  Operation the `Lineage` does not name is ignored and listed as `record`.
- `check_operation_change` needs `access.manage` at `operation:<id>` through the chief and
  access tiers, a valid `after`, every action added to a role held at that Operation, and every
  grant a rule adds (here, or through a role of this record in the records of Operations under
  it) held at its scope and, when it propagates, at every Operation under it. Removals and
  reorders that add nothing pass.

`station::plugins::Plugins` scans the plugin paths for libraries that declare an AI6 kind or
any kind beginning with `KIND_PREFIX`, and lists and constructs `Authenticator`,
`Directory`, `Authorizer` and `Accounts` providers by id (`authenticators()`,
`account_managers()`, `authenticator(id, config)`, `accounts(id, config)` and the
rest); the caller composes authentication, directory lookup and authorization.
`cached_directory(id, config, cache)` and `cached_authorizer(id, config, cache)` return the
provider in an `Arc<coqui_access_kit::Cached<dyn ..>>` keeping its answers as the
`CacheOptions` say (default 5 s, 1 s for "not found" or a denial, 10 000 entries); a credential
is never kept. AI6
registers no access provider of its own. The providers are the loadable libraries of
`coqui-access`, built from <https://github.com/Boriken-Dev/coqui-access>:

| Id | Kinds | Config (a Guatiao value; every key optional) |
| --- | --- | --- |
| `coqui_access_password` | Authenticator, Directory, Accounts | `source`, `max_accounts`, `argon2`, `max_password_bytes`, `record_limits` |
| `coqui_access_key` | Authenticator, Directory, Accounts | `source`, `prefix`, `key_hash` (tagged by `algorithm`), `max_keys`, `record_limits` |
| `coqui_access_mtls` | Authenticator, Directory, Accounts | `source`, `waiting`, `max_records`, `waiting_per_source`, `waiting_total`, `waiting_rerecord_secs`, `trust`, `ca_file`, `subject`, `san`, `ip_in_dns`, `account`, `max_chain`, `record_limits` |
| `coqui_access_ldap` | Authenticator, Directory | `url`, `start_tls`, `ca_file`, `base`, `bind`, `bind_password`, `user_filter`, `name_attribute`, `group_attribute`, `group_filter`, `group_base`, `nested_groups`, `timeout_seconds` |
| `coqui_access_proxy` | Authenticator | `trusted`, `user_header`, `certificate_header`, `certificate_encoding`, `subject_header`, `account`, `max_certificate_bytes` |
| `coqui_roles` | Authorizer | `scope_kinds`, `roles`, `rules`, `limits` |

`source` is `{ "file": { "path": "<file>", "format": "toml" | "json" } }`; with none, records
live in memory. `coqui_access_ldap` also takes `cache` (on by default). The host's own tests build the plugin libraries as dev-dependencies and load them from
beside the test binary; after removing or renaming a plugin, clean `target/`, because the host
loads every library it finds.

## Review (`ai6::directorates::m::review`)

Who decides a Mission's plan or output is good enough. Pure: no I/O, so what a reviewer or a
Mission's output says reaches a decision only as a `Vote`. The `review` family is global and applies
live; a seat or a choice naming something the global roster lacks makes the configuration invalid.

```rust
pub const NO_REVIEW: &str /* "none" */; pub const BRIEF_REVIEW_KEY: &str /* "review" */;
pub const PLAN_REASON: &str /* "review-plan" */; pub const OUTPUT_REASON: &str /* "review-output" */;
pub const REDO_REASON: &str /* "review-redo" */; pub const DEFAULT_REVIEWER_TIMEOUT_SECS: u64 /* 3600 */;
#[non_exhaustive] pub enum ReviewGate { Plan, Output } // as_str, reason
pub fn is_review_reason(reason: &str) -> bool;
#[non_exhaustive] pub struct ReviewChoice { pub plan: Option<String>, pub output: Option<String> } // at, named
#[non_exhaustive] pub struct PanelSeat { pub agent: Option<String>, pub role: Option<String>, pub weight: u32 /* 1 */, pub veto: bool }
                  // PanelSeat::agent, PanelSeat::role, weighing, with_veto, label
#[non_exhaustive] pub enum OnNoAgreement { Person /* default */, Refuse }
#[non_exhaustive] pub struct ReviewPolicy { pub panel: Vec<PanelSeat>, pub pass_at: f64 /* 1.0 */, pub distinct_models: u32 /* 1 */,
    pub rounds: u32 /* 1 */, pub redo_limit: u32 /* 1 */, pub on_no_agreement: OnNoAgreement, pub max_turns: Option<u64>,
    pub max_tokens: Option<u64>, pub reviewer_timeout_secs: u64 /* 3600; 0 no limit */ }
                  // new, validate -> Result<(), String>, weakenings, at_least_as_strict_as
pub fn policies_from_records(records) -> Result<BTreeMap<String, ReviewPolicy>, String>;
#[non_exhaustive] pub enum BriefAuthority { Person, ZeroZero(String), Outside }
#[non_exhaustive] pub struct BriefNaming { pub choice: ReviewChoice, pub authority: BriefAuthority } // new
pub fn choice_in_data(data: &BTreeMap<String, Value>) -> Option<ReviewChoice>;
pub fn choice_in_labels(labels: &[String]) -> Option<ReviewChoice>; // review:<name>, review-plan:<name>
#[non_exhaustive] pub struct PolicyLookup<'a> { pub gate, pub brief: Option<&BriefNaming>, pub role: Option<&ReviewChoice>,
    pub operation: Option<&ReviewChoice>, pub author: Option<&str>, pub policies: &BTreeMap<..> } // new
#[non_exhaustive] pub struct AppliedPolicy { pub name: Option<String>, pub from: PolicySource, pub weakens: Option<String>, pub ignored: Option<IgnoredNaming> }
pub fn applicable(lookup: &PolicyLookup<'_>) -> Result<AppliedPolicy, ReviewRefusal>;
#[non_exhaustive] pub struct PanelCandidate { pub id, pub roles, pub models: BTreeSet<String>, pub unavailable: Option<String> } // of, new
pub fn models_of(agent: &Agent) -> BTreeSet<String>; // a route or Agent naming no model is handler:<id>
#[non_exhaustive] pub struct Reviewer { pub seat: usize, pub agent, pub role, pub weight: u32, pub veto: bool, pub model }
pub fn choose_panel(policy, author: &PanelCandidate, candidates: &[PanelCandidate]) -> Result<Vec<Reviewer>, PanelRefusal>;
#[non_exhaustive] pub enum Vote { Pass, Redo, Abstain, Absent } // Absent keeps its weight in the share
#[non_exhaustive] pub enum Decision { Waiting, Pass, Redo, Vetoed, NextRound, NoAgreement }
pub fn decide(policy, seats: usize, ballots: &[Ballot], round: u32) -> Decision;
#[non_exhaustive] pub enum Step { Wait, Pass, SendBack, AskAgain, Person(&'static str), Refuse }
pub fn next_step(policy, gate, decision, sent_back: u32) -> Step;
pub fn no_agreement(policy, gate) -> Step;
pub fn out_of_budget(policy, turns: u64, tokens: u64) -> bool;
pub fn read_vote(text: &str, tail: Limit, reasons: Limit, reason_bytes: Limit) -> VoteAnswer;
```

A reviewer's turn is a brief AI6 writes (`PANEL_SOURCE` `"ai6-review"`, data key `PANEL_DATA_KEY`
`"mission_review"`, a `review` provenance whose reference starts `mission-review:`):
`PanelTarget { mission, gate, iteration, round, seat }` (`new`, `review_id`, `entry`),
`is_panel_review(&Brief)`, `panel_target(&Brief)`, `panel_brief(&MissionReview, &Reviewer, &PanelTarget,
&ReviewLimits, (received_by, received_at)) -> Brief` (everything quoted between markers longer than
any `=` run inside), `redo_message(gate, reasons, sent_back, limit) -> String`, `one_line`.

`ai6::directorates::registry::reviews`: `MissionReview { mission_id, operation, gate, revision, policy_name,
policy, author, status: ReviewStatus, why, iteration, sent_back, round, panel, asked, votes, material:
ReviewMaterial, tokens, overrule }` (`open`, `vote_of`, `asked_of`, `record_vote` (one a seat a round),
`ballots`, `turns`, `others_reasons`, `redo_reasons`); `ReviewStatus { Reviewing, Waiting, Person,
SendingBack, Redoing, Passed, Refused }`; `MissionReviewStore` on `RegistryBackend`: `mission_review`,
`mission_reviews`, `put_mission_review` (written only over the revision read; `None` when another
wrote first); `tally(&[MissionReview]) -> BTreeMap<String, ReviewTally { put_up, sent_back, votes,
disagreed, absent }>` for an Agent's scorecard; `update_mission_review(store, (mission, gate), tries, change)`. SQL table
`ai6_mission_reviews`, made at the next open. `Mission.review_naming: Option<BriefNaming>`,
`StationLimits.review: ReviewLimits` (`[review]`), and a safe house's change: `SafeHouseDiff { text,
cut }`, the kind method `SafeHouseSession::diff(bytes)` (unsupported by default; `git` gives the working
tree against the base and untracked names) and `SafeHouse::diff(Limit)`.

The `AgentHandler` kind method `session_record(SessionRecordQuery { session_id, working_directory,
environment }) -> Result<Option<SessionRecordFile { path, format }>, ProviderError>` (default: none)
names where the harness keeps its own record of a Session on this host: `claude` (`claude-code-jsonl`),
`codex` (`codex-rollout-jsonl`) and `antigravity` (`antigravity-jsonl`) answer when the file is there.

The `AgentHandler` kind method `program() -> Result<Option<String>, ProviderError>` (default: `None`)
names the program the harness is started as, a bare name or a path: `claude`, `codex` and
`antigravity` answer their `executable`, `acp` its `command`. Before a Station starts a harness on its
own host it looks the program up with `section_00::programs::missing_program` on the Session's own
environment; a refusal fails the Mission at once (`Mission.failure`), and its brief with that reason.
A handler that answers `None` or an error is not looked up.

The plan gate: `ai6::READ_CLEARANCE` (`"read"`, set only by AI6) and `clearance_reads_only(&str)`; the
`AgentHandler` kind method `read_only_refusal() -> Result<Option<String>, ProviderError>` (default:
refuses; `claude` and `codex` hold a Session to reading and take up the Mission's tools on a resume of
the same Session). A Mission under a `plan` policy starts and is sent back at `read`, its only Cable
tool `submit_plan`, kept in `Mission.plan` until it is put up; it does not start on a handler that
refuses, nor in remote-hands mode, and waits with the reason in `Mission.waiting`.

`AgentHandler::image_input() -> Result<Option<String>, ProviderError>` (default: "this handler
gives its harness no image"; `claude` takes them, as stream-json image blocks): `None` when the
handler reads a message in the content form (`MessageContent`). The assistant desk refuses a job
with images on a handler that says why not, and sends a message whose own text begins with
`CONTENT_PREFIX` in the form to one that reads it, so it stays text. Adding it moved the kind's floor
hash (pinned in a test).

The crate root re-exports these, `decide` as `decide_review`, `applicable` as `applicable_review`,
`next_step` as `review_next_step`, `out_of_budget` as `review_out_of_budget`, `Decision` as
`ReviewDecision` and `Step` as `ReviewStep`. `RosterBuilder::review(name, policy)` adds a policy and
`Roster::reviews()` lists them; `AgentRole::with_review` and `Operation.review` choose one per gate.

## Compartments (`ai6::directorates::m::compartments`)

Need-to-know access across Operations; clearance stays the autonomy level. A compartment carries
no policy authority. The `compartment` family is global and applies live.

```rust
#[non_exhaustive] pub enum CompartmentAction { MissionStatus, MissionRead /* implies status */,
    ProductRead, CableSend, CableRead /* people */, MissionPropose /* Agents */,
    MemoryRead /* an Operation's memory, audited as a crossing */ }
    // written mission.status ...; ALL, as_str(), parse(name) -> Option; anything else is Invalid
#[non_exhaustive] pub enum CoverTarget { Operation(String), Mission(String) } // Display, parse(text)
#[non_exhaustive] pub struct CompartmentCover { pub target: CoverTarget, pub propagate: bool } // new, with_propagate
#[non_exhaustive] pub enum ReadInSubject { Person(String), Group(String), Account(String),
    Operation(String), Mission(String), Agent(String) }      // Display, parse(text), is_people()
#[non_exhaustive] pub struct ReadIn { pub subject: ReadInSubject, pub propagate: bool } // Operation only
#[non_exhaustive] pub struct CompartmentLimits { pub max_fanout: Limit /* 100 */, pub rate: Rate /* 10.0 */,
    pub burst: Limit /* 50 */, pub sender_rate: Rate /* off */, pub sender_burst: Limit /* off */,
    pub max_product_bytes: Limit /* 65 536 */ }
#[non_exhaustive] pub struct Compartment { pub id, pub description, pub actions: BTreeSet<CompartmentAction>,
    pub covers: Vec<CompartmentCover>, pub read_in: Vec<ReadIn>, pub min_clearance: Option<String>,
    pub limits: CompartmentLimits }
impl Compartment { pub fn new(id) -> Self; /* with_description, with_actions, with_cover, with_read_in,
    with_min_clearance, with_limits */ pub fn validate(&self) -> Result<(), RosterError>;
    pub fn weakenings(&self) -> Vec<String>; pub fn to_definition(&self) -> Value;
    pub fn from_definition(id, &Value) -> Result<Self, RosterError>; }
pub struct CompartmentSettings { pub description, pub actions, pub limits, .. } // the form: from_json, apply(id, base)
pub struct Compartments;   // of(iter), from_records(iter), get, iter, is_empty, weakenings,
                           // compile(&Roster) -> Result<CompiledCompartments, RosterError>
pub struct CompiledCompartments; // Default grants nothing
impl CompiledCompartments {
    pub fn field_allows(&self, &FieldPrincipal, CompartmentAction, &CompartmentResource) -> Option<&str>; // the compartment
    pub fn field_allows_in(&self, id, &FieldPrincipal, CompartmentAction, &CompartmentResource) -> bool;
    pub fn person_allows(&self, &Person, CompartmentAction, &CompartmentResource) -> Option<&str>;
    pub fn person_compartments(&self, &Person, operation) -> Vec<&str>;
    pub fn person_read_in(&self, id, &Person) -> bool;     // named in read_in, whatever it covers or grants
    pub fn field_read_in(&self, id, &FieldPrincipal) -> bool; // agent:, mission:, operation: (propagating up
                                                              // the tree), at its clearance floor
    pub fn reachable(&self, &FieldPrincipal, CompartmentAction) -> Vec<CompartmentResource>;
    pub fn person_reachable(&self, &Person, CompartmentAction) -> Vec<CompartmentResource>;
    pub fn held(&self, &FieldPrincipal) -> Vec<HeldCompartment>;
    pub fn get(&self, id) -> Option<&Compartment>; pub fn iter(&self); pub fn refused(&self) -> &BTreeMap<String, String>;
    pub fn lineage(&self) -> &Lineage; }
#[non_exhaustive] pub enum CompartmentResource { Operation(String), Mission { id, operation } }
#[non_exhaustive] pub struct FieldPrincipal { pub agent, pub operation, pub mission: Option<String>, pub clearance } // new
pub fn current(provider: &dyn ConfigProvider) -> Result<CompiledCompartments, RosterError>;
```

A Session's tools (feature `cables-mcp`), listed only while it holds a compartment and working
before the harness reports its Session ID: `compartments`, `missions_in`, `mission_brief`,
`mission_products`; `propose_mission` takes `operation`. A brief or a product marks the Session
`product:<operation>` first; a proposal into another Operation is `brief:compartment` there. Cable
scope `compartment` (`CableTarget`, `CableSendRequest::with_operation` narrows it) needs `cable.send`
through it and no receive grant; `CableConfig::cross_operation()` is `Compartments` by default, so any
Cable crossing an Operation also needs `cable.send` there, or `StationGrants`, a logged weakening.
`CableRelay::set_compartments(Arc<dyn CableCompartments>)` gives the relay its answers;
`with_durable_audit` takes the log of crossings and of Cables sent for a person, and refuses
without one. A `compartment` Cable grant in the `cables` record names only `release`.

**Ask `current` for every decision and keep nothing.** It reads the records from the provider, so a
removed grant holds from the next call; a record that cannot be read grants nothing and is named in
`refused()`; a provider that cannot be read is an error, and the caller refuses. `compile` is the
stricter check a provider's `put` and a snapshot go through: an Operation a compartment names must
exist, past `RosterSettings::max_compartment_rules` (10 000 pairs per compartment; `none` for no
bound) is refused. A cover or read-in of an Operation reaches down the tree unless `propagate =
false`; nothing reaches upward. Union across compartments; first match inside one is any match.

`directorates::registry::products`: `mission_status(&dyn RegistryBackend, &FiledBrief) -> Value`, the fields
any holder of `mission.status` sees and nothing else, and `mission_product(.., limit)`, with the outcome's
`state`, `reason`, `branch` and `summary`, the CX report and the refs, each text cut with `CUT_MARKER`. An
outcome's other keys (Session IDs, who recovered it) are not part of a product.

## Settings (`ai6::settings`)

```rust
pub fn schema(section: &str) -> Option<serde_json::Value>;   // a dotted path: provider, registry.limits.sessions, record.station.scheduler
pub fn sections() -> Vec<(String, String)>;                  // every table's path and title
pub mod reference {
    pub const DOCS: [&str; 4];                               // the docs holding generated tables
    pub const OO_DART: &str;                                 // Outer Office's copy of [oo]
    pub fn table(section: &str) -> Result<String, String>;   // one section's Markdown table
    pub fn rewritten(text: &str) -> Result<String, String>;  // a doc with each marked table written anew
    pub fn write(root: &Path) -> Result<(), String>;         // every generated file under a checkout
    pub fn stale(root: &Path) -> Result<Vec<String>, String>; // the ones that differ
}
```

`[oo]` is declared by `bootstrap::oo::OoSettings` (`OoChat`, `OoRefresh`, ..), whose schema and form
are `directorates::m::records::oo_settings_form()`; `Bootstrap::oo` stays the table as written, since
no Rust program reads it. A setting is declared once, on its struct: a doc comment, `#[schema(title = "..")]`, a unit
(`format = "bytes"`; `Seconds` and `TimeLimit` carry theirs), and a default (`#[map(default = ..)]`
reading the type's `Default`, or for an `Option` a closing `Default: ..` in the doc comment). A test
refuses a field with no title or no stated default. Everything else reads it: `ai6 config schema`,
the record forms, and the tables between `<!-- schema:SECTION:begin -->` and its end, which
`cargo run -p ai6 --features cli --example settings_reference` writes and `-- --check` verifies.

## Station runtime limits (`ai6::station::limits`)

The `station` record's tables, kept by a Station as it starts (they apply at restart). Each
table derives `ToValue`, `FromValue` and `Schema` with `deny_unknown_fields`; its keys, titles,
defaults and units are its declaration, which `ai6 config schema record.station.<table>` prints and
`docs/configuration.md` renders. Counts and sizes are `Limit`, ceilings on durations `TimeLimit`,
other durations `Seconds`; a queue, page or attempt count is a plain integer. Not re-exported at
the crate root: several table names repeat the Registry's (`AccountLimits`, `ApprovalLimits`,
`AuditLimits`, `SourceLimits`, `StationLimits`).

```rust
pub struct StationLimits { /* one field per table, and */ pub providers: BTreeMap<String, serde_json::Value> }
impl StationLimits { pub fn validate(&self) -> Result<(), String>; }   // zero queues/pages/attempts, a non-table provider
```

`ProbeLimits` (`[probes]`, the timeouts of a verifier's reads of a Mission's deliverables: `git_timeout` 30 s,
`http_timeout` 20 s, `tracker_timeout` 30 s, `file_timeout` 5 s, and `comments` 100, the latest comments a
tracker probe reads). `[registry.limits.sessions]` also holds `amendment_bytes` (8 KiB), `deliverables` (32 per
attempt), `evidence_entries` (64, the oldest go first) and `evidence_detail_chars` (300).

`OverlayLimits` (`[overlays]`: `manifest_bytes` 1 MiB, `manifest_depth` 64, `files` 4096,
`bytes` 64 MiB, `file_bytes` 16 MiB, `unpinned` false, `context_bytes` 24 KiB, `fetch_timeout` 300 s,
`deps_timeout` 600 s, `output_bytes` 1 MiB, `dotagents_pyz` and `dotagents_pin` none) has `tree() ->
TreeLimits`, `manifest() -> ReadLimits` and `weakenings()` (looser limits, and `unpinned = true`).

A Station hands `&StationLimits` (or one table) to whatever does the work; there is no
global. Free functions that bounded with a constant take the table or the value:
`reattach_delay(failures, &RecoveryLimits)`, `parked_until(.., grace_secs)`,
`classify(command, repo, nesting)`, `ActionScope::check_size(&ApprovalLimits)`,
`HarnessPolicy::decide(root, &HarnessLimits)`, `probe(.., &ProvisionLimits)`,
`strip_controls(bytes, csi_bytes)`, `JsonLines::new(&TerminalOutputLimits)`,
`read_review(text, &VettingLimits)`, `Vetting::within(&VettingLimits)`.
`Plugins::with_provider_configs` takes `[providers.<id>]` as Guatiao values
(`plugins::provider_configs` converts them).

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
    pub fn with_partial_line_bytes(self, bytes: Limit) -> Self;   // default 4,096; station partial_line_bytes
    pub fn with_csi_bytes(self, bytes: Limit) -> Self;            // default 64; station [terminal_output] csi_bytes
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
pub struct Exposure { pub source: String /* brief:source | brief:registry_drop | brief:compartment | cable:X | product:OPERATION | tool:X | review */, pub at: f64 }
pub enum Restriction { Proposals, Approvals, Cables, Secrets }
pub struct ExposurePolicy;   // default() = strict; ::preset(Strict | Observe); .restricts(r); .exposed_by_tool(name)
pub struct ExposureDefinition;   // [operation.ID.exposure]: preset, restrict = [..], tools = [..]
pub struct ExposureGate;     // clone-shared per Session: new(policy, inherited, &SessionLimits),
                             // mark(source, at) -> bool (sources cut to exposure_source_bytes,
                             // at most `exposures` kept), exposures(), is_exposed(), restricts(r),
                             // take_changes()
pub fn cable_source(sender: &str, limit: Limit) -> String;   // "cable:SENDER" cut to limit bytes
pub fn memory_source(id: &str, limit: Limit) -> String; // "memory:<id>": read what an exposed Session wrote
pub fn tool_source(tool: &str, limit: Limit) -> String;      // "tool:NAME" cut to limit bytes

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

## Records a person writes

Operations, Agents, Agent roles, cover policies, Moneypenny's settings and the roster's settings
are declared once, on their types: `#[derive(ToValue, FromValue, Schema, Form)]` holds each field's
title, description (its doc comment), default, choices, bounds and section. A stored definition or
a file becomes a `guatiao::Value` at the format boundary and the type comes from that, so a refusal
names the key and never the value.

```rust
impl ConfigRecord { pub fn form(self) -> Result<Option<RecordForm>, RecordFormError>; }  // None: no form yet
pub struct RecordForm { pub schema: serde_json::Value, pub form: serde_json::Value, pub key: RecordKey }
pub struct RecordKey { pub title: String, pub description: String, pub fixed: Option<String> }
pub fn read_definition<T: guatiao::FromValue>(definition: &serde_json::Value) -> Result<T, String>;
```

`Limit`, `Seconds` and `TimeLimit` implement `ToValue`, `FromValue` and `Schema`: a `Limit` is a
whole number or `"none"`, and `null` reads as `"none"`. `WatchdogOverrides` holds `Option<Seconds>`.
`Sleeper`, `HarnessPolicy`, `CoverIdentity` and the record enums keep serde as well, for the Registry
and check-ins they also cross.

## Operations and Agent roster (`ai6::directorates::m::roster`)

```rust
#[non_exhaustive]
pub struct Agent {
    pub id: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub handler: Option<String>,
    pub handler_settings: serde_json::Map<String, serde_json::Value>, // `handler_settings` in the roster; {} = provider defaults
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
    pub routes: Vec<AgentRoute>,                  // `[[agent.ID.route]]`, in place of `handler`; empty for one handler
    pub balance: RouteBalance,                    // Headroom (default) | RoundRobin | Priority
    pub retire_on: Option<String>,                // UTC date YYYY-MM-DD the model reaches end of life
    pub on_retire: OnRetire,                      // Desk (default) | Retired
    pub retire_warning_days: u32,                 // DEFAULT_RETIRE_WARNING_DAYS = 14; 0 = no warning
    pub wake_on_cable: bool,                      // true; false: a Cable waits for the Agent's own read_cables
    pub wakes_per_hour: u32,                      // DEFAULT_WAKES_PER_HOUR = 60; 0 = no wake
}
#[non_exhaustive] pub struct AgentRoute {        // one way to its model; ToValue, FromValue, Schema, Form
    pub handler: String,                          // required
    pub model: Option<String>, pub effort: Option<String>,
    pub handler_settings: serde_json::Map<String, serde_json::Value>,
    pub pool: Option<String>,
    pub env: SessionEnv,                          // no credential's or AI6's variable (refused)
    pub secrets: Vec<String>,                     // `[secret.NAME]`s, issued whatever the Session read
    pub priority: i64,
}
impl AgentRoute { pub fn new(handler: impl Into<String>) -> Self; pub fn reference(&self) -> RouteRef; }
/// Which route ran; serde; Display `handler model (pool name)`.
#[non_exhaustive] pub struct RouteRef { pub handler: String, pub model: Option<String>, pub pool: Option<String> }
impl RouteRef { pub fn new(handler: impl Into<String>, model: Option<String>, pool: Option<String>) -> Self; }
#[non_exhaustive] pub enum RouteBalance { Headroom, RoundRobin, Priority }
impl RouteBalance { pub fn strategy(self) -> PoolStrategy; }
impl Agent {
    pub fn is_routed(&self) -> bool;
    /// Complete: what a route leaves out from the Agent. One `handler` is one route; neither, none.
    pub fn route_list(&self) -> Vec<AgentRoute>;
    pub fn runs_on(&self, handlers: &[String]) -> bool;
    /// As a Session on `route` works: its handler, model, effort, settings and pool; its env over the Agent's.
    #[must_use] pub fn on_route(&self, route: &AgentRoute) -> Self;
    pub fn route_named(&self, reference: &RouteRef) -> Option<AgentRoute>;
    /// Its model, or for routes every model they name: what a clearance raise is signed for.
    pub fn occupant_model(&self) -> Option<String>;
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
    pub require_approval_before_done: bool,       // a done with no green ink decided in that attempt is held
    pub deliverables: Vec<String>,                // each `kind` or `kind:target` every Mission of the role owes; validated
}
impl AgentRole {
    pub fn with_deliverables(self, Vec<String>) -> Self;
    pub fn new(id: impl Into<String>, purpose: impl Into<String>, policy: impl Into<String>) -> Self;
    pub fn requiring_approval_before_done(self, bool) -> Self;
    pub fn validate(&self) -> Result<(), RosterError>;
}
#[non_exhaustive]
pub struct Operation {
    pub id: String,
    pub parent: Option<String>,                   // the parent Operation's codename; None: a root
    pub inherit: bool,                            // layered over its ancestors; default true
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
    pub overlays: Vec<String>,                    // [overlay.<name>] names its Sessions get after the loadout's; each once
    pub harness: HarnessPolicy,                   // [operation.ID.harness]; ignore unless configured
    pub promotion: PromotionPolicy,               // [operation.ID.promotion]; see Promotion below
    pub scorecard: ScorecardPolicy,               // [operation.ID.scorecard]; see Scorecard below
}
impl Operation {
    /// The defaults a file gives `[operation.ID]`: clearance "field", cover "covert",
    /// on_blown "flag", strict exposure and green ink, `Workspace::default()`.
    pub fn new(id: impl Into<String>) -> Self;
    #[must_use] pub fn with_parent(self, parent: impl Into<String>) -> Self;
    /// Each key its definition was resolved with, dotted ("green_ink.kinds"), and the
    /// codename of the Operation that wrote it; defaults have no entry, a record built in code
    /// has none.
    pub fn origins(&self) -> &BTreeMap<String, String>;
    /// The `[operation.<id>]` keys it was resolved from, ancestors' layered in, before presets
    /// and defaults; empty for a record built in code.
    pub fn definition(&self) -> &serde_json::Map<String, serde_json::Value>;
    /// No `path`, `theater` or `workspace.source`: it only groups others, and intake fails
    /// its briefs with "Operation \"P\" has nowhere to work".
    pub fn has_nowhere_to_work(&self) -> bool;
    /// The checks the file loader runs after resolving an Operation; whether `parent` is
    /// registered is checked with the roster.
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
    ReadOnly,      // the ConfigProvider takes no put/remove
    Conflict,      // put_if: the stored record is not the one the writer read
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
    pub fn settings(&self) -> &RosterSettings;                // the `roster` record
    // The tree of Operations; `key` is a codename or project name, unknown ones give nothing.
    pub fn parent(&self, key: &str) -> Option<&str>;
    pub fn children(&self, key: &str) -> Vec<&str>;           // codename order
    pub fn lineage(&self, key: &str) -> Vec<&str>;            // ancestors, nearest first
    pub fn subtree(&self, key: &str) -> Vec<&str>;            // key first, then depth first
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
    pub fn watch(self, watch: Watch) -> Self;      // build checks its Agent and Operations
    pub fn settings(self, settings: RosterSettings) -> Self;
    pub fn max_operation_depth(self, depth: Limit) -> Self;   // Limit::UNBOUNDED: no bound
    pub fn max_compartment_rules(self, rules: Limit) -> Self; // pairs per compartment
    pub fn build(self) -> Result<Roster, RosterError>;   // Roster::from_parts; settings validated
}
/// The `roster` record: a `[roster]` table in the global `operations` file (serde; unknown
/// keys refused). `max_operation_depth` reads a number, or "none" (JSON null too).
#[non_exhaustive] #[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RosterSettings {
    pub max_operation_depth: Limit,   // levels, a root the first; default 64; 0 refused
    pub max_compartment_rules: Limit, // read-in and cover pairs per compartment; default 10 000
    pub reports: ReportSettings,      // [roster.reports]: the Report lists (Reports, below)
}
impl RosterSettings {   // Default: the defaults above
    pub fn new() -> Self;
    #[must_use] pub fn with_max_operation_depth(self, depth: Limit) -> Self;
    #[must_use] pub fn with_reports(self, reports: ReportSettings) -> Self;
    pub fn validate(&self) -> Result<(), RosterError>;
    pub fn weakenings(&self) -> Vec<String>;  // "roster.max_operation_depth none is looser than the default 64"
}
pub const DEFAULT_MAX_OPERATION_DEPTH: usize = 64;
pub fn config_dir() -> Result<PathBuf, RosterError>;

// ai6::directorates::m::assistant, re-exported at the crate root
#[derive(Default, Serialize, Deserialize)] // serde: unknown keys refused
pub struct AssistantSettings;   // keys: personal_repo, shared.<group> {repo, write}, org {repo, write}, allow_elevated,
                                // interpreters (["python3"]), network ("limited"), allowed_hosts ([]),
                                // denied_networks ([]: nothing denied, under open too),
                                // unsandboxed_scripts (false), skills.<name> {path, pin}, skills_unpinned,
                                // skills_allow_tools, group.<group> and person.<id> narrowing tables,
                                // views (AssistantViewLimits), limits (AssistantLimits),
                                // desk (AssistantDeskLimits), gateway (AssistantGatewayLimits);
                                // a table at its default is not serialized
// [assistant.views], what Universal Exports holds her views to: Option<u64> each, absent =
// its default; validate() refuses 0 as "assistant.views.<key> must be at least 1"
#[non_exhaustive] #[derive(Default, Serialize, Deserialize)]
pub struct AssistantViewLimits { pub blocks, view_bytes, text_bytes, short_bytes, columns,
    rows, items, series, points, pie_slices, diff_bytes, fields, schedule_rows, graph_nodes,
    graph_edges, svg_bytes, svg_elements, svg_depth, svg_id_bytes, navigate_label_bytes: Option<u64> }
impl AssistantViewLimits {
    pub fn defaults() -> Self;                         // each bound when the record sets none
    pub fn set(&self) -> Vec<(&'static str, u64)>;     // the ones given
}
// [assistant.limits], [assistant.desk], [assistant.gateway]: #[non_exhaustive], Default = the
// documented defaults (docs/configuration.md "Moneypenny's limits"), serde(default,
// deny_unknown_fields); Limit / TimeLimit fields take a number, null or "none".
pub struct AssistantLimits { pub cable_grants, message_bytes, context_bytes, event_bytes,
    events, change_bytes, change_files, commit_message_bytes, shared_bases, skills,
    personal_walk: Limit }   // contract checks; both sides
pub struct AssistantDeskLimits { pub idle: TimeLimit, pub live, job_bytes, report_queue,
    known, clone_bytes, save_bytes: Limit, pub report_attempts: u32,
    pub token_margin, job_wait, call_margin, job_retry, report_retry, close_wait, remove_retry,
    unlocked_stale, clone_timeout: Seconds }   // a Station's desk; read per job / start / use
pub struct AssistantGatewayLimits { pub conversations_per_person, conversations,
    conversation_entries, conversation_bytes, person_bytes, live_per_person,
    tool_calls_per_minute, proposals,
    message_bytes, memory_context_bytes, change_message_chars, recap_bytes, ready_entries,
    ready_bytes, socket_message_bytes, route_bytes, result_chars, author_chars,
    audit_target_bytes, proposal_reads: Limit, pub conversation_age: TimeLimit, pub event_buffer: u64,
    pub token_ttl, token_reuse_margin, answer_timeout, answer_deadline,
    proposal_ttl, hello_timeout, max_wait, sweep_every, shared_base_wait: Seconds }
pub struct AssistantPageLimits { pub max_file_bytes, max_page_bytes, max_page_files, max_pages,
    max_person_bytes, max_total_bytes, inline_bytes, title_chars, max_feeds, feed_reads_per_minute,
    staged_uploads, handoffs: Limit, pub default_ttl, open_ttl, handoff_ttl, upload_wait, stage_ttl, sweep_every: Seconds,
    pub scripts, feeds, frame, sharing: bool, pub network: PageNetwork }   // [assistant.pages]; open by
    // default; validate() refuses a zero; never a weakening (a quota moves either way)
#[non_exhaustive] pub enum PageNetwork { Open, Own, None }   // serde lowercase; as_str(), tighter(other)
impl AssistantSettings { pub fn pages(&self) -> &AssistantPageLimits;
                         pub fn with_pages(self, pages: AssistantPageLimits) -> Self; }
// AssistantLimits also images (4) and image_bytes (5 MiB): what one message carries.
// AssistantLimits also page_file_bytes (8 MiB), page_files (200), page_bytes (16 MiB): what a
// Station reads of a page. AssistantPageFile { path, bytes, sha256 } is the manifest
// entry of an upload; page_call_digest(path) -> String matches an upload to its publish_page call;
// ASSISTANT_PUBLISH_PAGE_TOOL = "publish_page".
// each: validate() -> Result<(), String> refuses only what cannot work ("assistant.<table>.<key>:
// at least 1"); weakenings() -> Vec<String>, one line per bound looser than its default
// (bounds::looser; pace keys left out)
impl AssistantSettings {
    pub fn new() -> Self;   // keeps nothing, python3 in the sandbox, no host, no skill, refuses elevated Stations
    pub fn with_personal_repo(self, template: impl Into<String>) -> Self; // {person} | {person_digest}
    pub fn with_shared(self, group: impl Into<String>, repo: SharedRepo) -> Self;
    pub fn with_org(self, repo: SharedRepo) -> Self;       // checked out as "org"
    pub fn with_allow_elevated(self, allow: bool) -> Self; // a weakening
    pub fn with_interpreters(self, interpreters: Vec<String>) -> Self;  // empty: no scripts
    pub fn with_network(self, policy: NetworkPolicy, allowed_hosts: Vec<String>) -> Self; // Open: a weakening
    pub fn with_unsandboxed_scripts(self, allow: bool) -> Self;         // a weakening
    pub fn with_denied_networks(self, denied: Vec<String>) -> Self;     // DenyEntry strings
    pub fn with_skill(self, name: impl Into<String>, skill: SkillEntry) -> Self;
    pub fn with_skills_unpinned(self, allow: bool) -> Self;     // a weakening
    pub fn with_skills_allow_tools(self, allow: bool) -> Self;  // a weakening
    pub fn with_group(self, group: impl Into<String>, narrowing: AssistantNarrowing) -> Self;
    pub fn with_person(self, person: impl Into<String>, narrowing: AssistantNarrowing) -> Self;
    pub fn with_views(self, views: AssistantViewLimits) -> Self;
    pub fn views(&self) -> &AssistantViewLimits;
    pub fn with_limits(self, limits: AssistantLimits) -> Self;
    pub fn limits(&self) -> &AssistantLimits;
    pub fn with_desk(self, desk: AssistantDeskLimits) -> Self;
    pub fn desk(&self) -> &AssistantDeskLimits;
    pub fn with_gateway(self, gateway: AssistantGatewayLimits) -> Self;
    pub fn gateway(&self) -> &AssistantGatewayLimits;
    pub fn personal_repo(&self) -> Option<&str>;
    pub fn shared(&self) -> &BTreeMap<String, SharedRepo>;
    pub fn org(&self) -> Option<&SharedRepo>;
    pub fn allow_elevated(&self) -> bool;
    pub fn interpreters(&self) -> &[String];
    pub fn network(&self) -> NetworkPolicy;
    pub fn allowed_hosts(&self) -> &[String];
    pub fn denied_networks(&self) -> &[String];
    pub fn unsandboxed_scripts(&self) -> bool;
    pub fn deployment_reach(&self) -> AssistantReach;      // before any table
    pub fn has_group_narrowing(&self) -> bool;             // any [assistant.group.<g>] table
    /// The deployment's reach capped by every group table of `groups` and the person's own.
    pub fn reach_for(&self, person: &str, groups: &[String]) -> AssistantReach;
    pub fn skills(&self) -> &BTreeMap<String, SkillEntry>;
    pub fn skills_unpinned(&self) -> bool;
    pub fn skills_allow_tools(&self) -> bool;
    pub fn skills_for(&self, person: &str, groups: &[String]) -> Vec<String>; // catalog less what tables leave out
    pub fn validate(&self) -> Result<(), String>;          // "assistant.<key>: …"; reads each SKILL.md; a table
                                                           // that widens fails naming "assistant.group.<g>.<key>"
    pub fn personal_repo_for(&self, person: &str) -> Result<Option<String>, String>;
    pub fn shared_for(&self, groups: &[String]) -> impl Iterator<Item = (&str, &SharedRepo)>;
    pub fn shared_named(&self, name: &str, groups: &[String]) -> Option<&SharedRepo>;
    /// allow_elevated, each write = true, network = "open", unsandboxed_scripts = true,
    /// skills_unpinned, skills_allow_tools, each SHA-1 skill pin, then each looser bound of
    /// limits, desk and gateway
    pub fn weakenings(&self) -> Vec<String>;
    /// What replacing `current` loosens or redirects: weakenings `current` lacks, interpreters,
    /// hosts or a looser network, a changed personal/shared/org repository, a skill added or
    /// with another path or pin, a group or person table removed or a key it set changed
    pub fn changes_from(&self, current: &AssistantSettings) -> Vec<String>;
}
#[non_exhaustive] #[derive(Default, PartialOrd, Ord)] // serde lowercase; Denied < Limited < Open
pub enum NetworkPolicy { Denied, #[default] Limited, Open }
impl NetworkPolicy { pub fn as_str(self) -> &'static str; }
#[derive(Default, Serialize, Deserialize)] // serde: unknown keys refused; every key optional
pub struct AssistantNarrowing;  // interpreters, network, allowed_hosts, denied_networks,
                                // unsandboxed_scripts, skills
impl AssistantNarrowing {
    pub fn new() -> Self;                                     // narrows nothing
    pub fn with_interpreters(self, interpreters: Vec<String>) -> Self; // a subset of the deployment's
    pub fn with_network(self, network: NetworkPolicy) -> Self;        // no looser than the deployment's
    pub fn with_allowed_hosts(self, hosts: Vec<String>) -> Self;     // under limited, a subset
    pub fn with_denied_networks(self, denied: Vec<String>) -> Self;  // added to the deployment's
    pub fn with_unsandboxed_scripts(self, allow: bool) -> Self;       // true only if the deployment allows
    pub fn with_skills(self, skills: Vec<String>) -> Self;            // catalog skills only
    pub fn skills(&self) -> Option<&[String]>;
}
pub const DEFAULT_INTERPRETER: &str = "python3";
pub struct SharedRepo;  // SharedRepo::new(repo).writable(bool); repo(), write()
pub struct SkillEntry;  // SkillEntry::new(path).pinned(PeerId); path(), pin() -> Option<PeerId>
pub const SKILLS_PLUGIN: &str = "ai6";             // Claude Code calls each installed skill `ai6:<name>`
pub fn skill_name(name: &str) -> bool;             // 1-64 of a-z, 0-9, -, starting with a letter or digit
// SKILL_INERT_KEYS and check_skill are in ai6::directorates::q::overlays: every SKILL.md AI6
// delivers, hers or an overlay's, passes the one check.
#[non_exhaustive] pub enum RepoKind { Local, Remote }
impl RepoKind {
    pub fn of(repo: &str) -> Result<Self, String>;
    pub fn local_path(repo: &str) -> Option<PathBuf>; // a Local repository's directory (file:// stripped)
}
pub const PERSON_PLACEHOLDER: &str = "{person}";
pub const PERSON_DIGEST_PLACEHOLDER: &str = "{person_digest}";
pub const ORG_SHARED: &str = "org";
pub fn person_digest(person: &str) -> String;  // 16 hex digits of SHA-256
pub fn file_safe(name: &str) -> bool;          // 1-64 of letters, digits, . - _, not starting with .
```

`[assistant]` lives in the global `agents.toml` only (an Operation's `.ai6/agents.toml` with
one fails to resolve its Agents); a SQL roster keeps it as the one row of
`ai6_config_assistant`, keyed `settings` (`ConfigRecord::Assistant`), and has the default
settings without it. `RepoKind::of` refuses empty values, control characters, Git
remote helpers (`::`) and schemes other than `file`, `ssh`, `git`, `http`, `https`; a local
path or `file://` is `Local` (made bare on first use), the rest and `user@host:path` are
`Remote` (must exist). `personal_repo_for` fills `{person_digest}` always and `{person}` only
for an identifier of 1-64 lowercase letters, digits, `.`, `_`, `@`, `-` not starting with `.`
(so identifiers differing only in case never share a repository); others are
refused, naming `{person_digest}`. `validate` refuses other placeholders, a template that does
not hold exactly one `{person}` or `{person_digest}` inside its path (after `scheme://host` or
`user@host:`), a template some person's identifier would fill in as a shared or the `org`
repository (compared without case), group names that are not `file_safe` or are `org`, and
repositories `RepoKind::of` refuses. `shared_for` yields the
groups' repositories the person is a member of, then `org`.

The skills catalog: `validate` refuses a name `skill_name` refuses, a `path` that is not
absolute (`Roster::load` first makes `agents.toml`'s relative paths absolute under the
configuration directory; a SQL roster or a builder must give absolute ones), a skill without
a `pin` unless `skills_unpinned`, a skill `check_skill` refuses, and a group or person table
naming a skill the catalog lacks. `check_skill(dir, name, allow_tools, tools_setting, file_bytes)`
(`directorates::q::overlays`) reads `<dir>/SKILL.md` (a regular file of at most `file_bytes`,
`[harness] manifest_file_bytes` here, never through a link; the settings check uses the
defaults; a refusal names `tools_setting`, `assistant.skills_allow_tools` here): front matter between two `---` lines
must parse as a YAML mapping (`serde-saphyr`; duplicate keys refused, merge keys expanded) with
`name` equal to the catalog's name and a non-empty `description`; unless `allow_tools`, it
may hold only `SKILL_INERT_KEYS` and no injected command (any `` !` ``, or a fence of three or
more backticks or tildes followed by `!`). So the skill directories must be readable wherever
the roster is loaded, Stations and Universal Exports alike. `skills_for` intersects the
catalog with each table of the person's groups and the person's own; a key a table leaves
out narrows nothing.

`load` reads `operations.toml` and `agents.toml`; missing files produce an empty roster and
invalid files fail closed. `from_parts` (and `builder().build()`) is the way in without
files: each Operation goes through `validate`, each role through `validate`, each Agent's
codename and clearance are checked at once, and a key given twice is refused. A code-built
Agent is complete as given (it inherits nothing); an Operation checkout's
`.ai6/agents.toml` may still extend or replace it. Operations are global: an Operation is not owned by a Station,
and its `station_labels` lists the Station labels a Station must carry to run its Missions.
`stations = ["id", ..]` optionally lists the Stations allowed to run them; a Mission's brief
may narrow that list but never widen it. `theater` binds the Operation to one Theater by ID.

Operations form a tree: `parent` names another Operation's codename, and an Operation
without one is a root. Every roster source (`load`, `from_parts`, `builder`, each
`ConfigProvider`) refuses a `parent` that is not registered and a cycle (the message names
the chain, `A -> C -> B -> A`), whatever the settings, and a tree deeper than
`RosterSettings::max_operation_depth`; so a provider refuses removing an Operation that still
has children. The Station logs `RosterSettings::weakenings` at start and on reload.

A definition (files, a `ConfigProvider`) is layered over its ancestors' before it is read:
as JSON tables, root first and the Operation last, tables merging key by key at any depth
(`station_labels`, `green_ink`, `budget`, `vetting`, `workspace`, ...) while scalars, arrays
and `cover_identity` replace (`roster`, `stations`, `pools`, `clearance`, `cover`,
`on_limit`). Presets and defaults then apply once, so a child's `green_ink.kinds` alone keeps
its parent's `preset`. Never inherited: `ai6::directorates::m::inherit::NOT_INHERITED`
(`name`, `description`, `path`, `theater`, `config`, `workspace.source`, `sources`,
`sleepers`, `parent`, `inherit`), so a child neither reads its parent's checkout
`.ai6/agents.toml` nor runs its Sleepers. `inherit = false` takes nothing from above it, and
its own children take nothing from above it either; it stays in the tree for access, pools
and Cables. A child cannot return a parent's key to unset except that way. Counters
(`max_concurrent`, budgets, vetting rates) stay per Operation. A record built in code takes
nothing from its parent. Weakening reports read the resolved records.

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
them waiting). An invalid date fails resolving the Agent. `operations` yields stable codename order.
`Agent.limits` is layered over the daemon-wide watchdog settings when a Mission starts or
resumes.

### Harness project settings (`ai6::directorates::m::harness`)

```rust
pub const PROJECT_DIRECTORIES: [&str; 4];      // ".claude", ".codex", ".agents/plugins", ".agent/plugins": every regular file, any depth
pub const PROJECT_FILES: [&str; 5];            // ".mcp.json", ".agents/hooks.json", ".agent/hooks.json", ".agents/mcp_config.json", ".agent/mcp_config.json"
#[non_exhaustive] pub enum ProjectSettings { Ignore /* default */, Pinned, Load } // lowercase
#[non_exhaustive] pub struct HarnessPolicy {
    pub project_settings: ProjectSettings,
    pub pin: Option<coqui_peer::PeerId>,        // the manifest digest `pinned` needs
}
impl HarnessPolicy {
    pub fn pinned(pin: PeerId) -> Self;  pub fn with(project_settings: ProjectSettings) -> Self;
    pub fn validate(&self) -> Result<(), String>;    // pinned needs a pin
    pub fn weakenings(&self) -> Vec<String>;         // load; a SHA-1 pin
    pub fn decide(&self, root: &Path, limits: &HarnessLimits) -> ProjectDecision; // reads the manifest under pinned (blocking)
}
#[non_exhaustive] pub struct ProjectDecision { pub load: bool, pub mcp: bool,
    pub manifest: Option<PeerId>, pub refused: Option<String> }
#[non_exhaustive] pub struct ProjectManifest { pub files: BTreeMap<String, PeerId> } // path -> sha256
impl ProjectManifest {
    pub fn read(root: &Path) -> Result<Self, String>;      // within HarnessLimits::default()
    pub fn read_within(root: &Path, limits: &HarnessLimits) -> Result<Self, String>;
    pub fn read_tree(root: &Path) -> Result<Self, String>; // a directory as the AOS tree digest reads it
    pub fn read_tree_within(root: &Path, limits: &HarnessLimits) -> Result<Self, String>;
    pub fn digest(&self) -> PeerId;  pub fn digest_with(&self, algorithm: coqui_peer::Digest) -> PeerId;
}
```

`read_tree` reads a whole directory (a skill: `ai6 assistant pin-skill DIR`) through
`directorates::q::overlays::tree_digest` within the same limits, so `digest()` is the AOS tree
digest an overlay's pin is: excluded names (`.git`, `node_modules`, `__pycache__`, ...) and
`.pyc` files are no members, and a link, special file, name that is no AOS relative path or two
paths equal under ASCII case is an error. Paths are relative to it; `root` itself must be a
directory reached without a link.

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
`decision.mcp` under `pinned` is whether the manifest holds `.mcp.json`, `.agents/mcp_config.json` or `.agent/mcp_config.json`. `ai6 harness pin DIR`
prints the manifest and its pin. Weakenings are printed at start.

## Bootstrap files (`ai6::bootstrap`, no feature)

One file, `ai6.cfg`, holds only what a process needs before it reaches its configuration
provider, for every program: `[provider]` and `[registry]` are shared, `[station]` is a Station's
and the `ai6` command's, `[unex]` is Universal Exports' and its clients', and `[oo]` is Outer
Office's, which no Rust program reads. Found by `SearchPath`: `--config` (must exist), else
`AI6_CONFIG` (must exist), else the file in `platform_config_dir()`, else in
`system_config_dir()`; the first found is the only one read, and none means every default. The working directory is never searched. The format follows
the extension (`toml`, `json`, `yaml`/`yml`); a `.cfg` names it on its first line (`#!json`,
`#!yaml`, `#!toml`, the line's last word; a document starting `%YAML` is YAML), else it is TOML.
A document becomes one `guatiao::Value`, and a refusal names the key or position and never the
value. Unknown keys are refused by every program, naming the key; each program checks at start
what it needs. A relative path in a file is relative to the file's directory.

```rust
pub const FILE_NAME: &str = "ai6.cfg";       pub const CONFIG_ENV: &str = "AI6_CONFIG";
pub const DEFAULT_LISTEN: SocketAddr;        // 127.0.0.1:7081
pub const DEFAULT_UNEX_LISTEN: SocketAddr;   // 127.0.0.1:7082
pub const DEFAULT_POLL: Duration;            // 5 s

#[non_exhaustive] #[derive(Debug, Clone, PartialEq, Eq)]   // Debug shows field names, never values
pub enum SecretRef { Provider(SecretSource), Named(String) }
// the one shape of every secret setting: `{ provider = "NAME", key = "KEY", ..fields }` (fields flat
// beside them, none named provider/key/secret/env/green_ink/kinds/targets) or `{ secret = "NAME" }`;
// a bare string, `{ file = .. }` and `{ env = .. }` are refused. Built in: `{ provider = "file", key =
// "PATH" }`, `{ provider = "env", key = "VAR" }`, `{ provider = "inline", value = ".." }`.
// FromValue, Schema; ToValue refuses inline. Bootstrap::read refuses Named (no [secret] is read yet).
impl SecretRef {
    pub fn file(key: impl Into<String>) -> Self;    pub fn env(name: impl Into<String>) -> Self;
    pub fn weakening(&self) -> Option<String>;      // Some for env and inline
    pub fn relative_to(self, dir: &Path) -> Self;   // a relative `file` key joined to dir
    pub fn read(&self) -> io::Result<Zeroizing<String>>; // built-ins only (else Unsupported); a file's one
                                                    // trailing newline dropped; empty is InvalidData
    pub fn describe(&self) -> String;               // the file, variable or provider, never the value
}

// every field Option or defaulted; #[non_exhaustive], ToValue, FromValue, Schema, deny_unknown_fields;
// each field has a title and a default in its schema (Bootstrap::schema()); Seconds, Limit: crate::bounds
pub struct ProviderSettings { location: Option<String>, password: Option<SecretRef>, poll: Option<Seconds>,
    loadout_bytes: Limit, loadout_value_chars: Limit, access_policy_bytes: Limit }   // ConfigLimits defaults
pub struct RegistrySettings { location: Option<String>, password: Option<SecretRef>, headquarters_lease: Option<Seconds>,
    sqlite_busy_timeout: Option<Seconds>, limits: RegistryLimits }   // [registry.limits.*]
pub struct IdentitySettings { dir: Option<PathBuf> }
pub struct PluginSettings { paths: Vec<PathBuf>, profile: Option<PluginProfile>, keep_verified: Option<KeepVerified> }
impl PluginSettings { pub fn keep_verified(&self) -> KeepVerified; } // its default: the newest 8
pub struct SecretSettings { dir: Option<PathBuf> }
pub struct StationSettings { id: Option<String>, state_dir: Option<PathBuf>, listen: Option<SocketAddr>,
    plugins: PluginSettings, identity: IdentitySettings, secrets: SecretSettings }
pub struct AccessSettings { chief: Vec<String> }   // [unex.access] chief: always-chief subjects
pub struct PagesSettings { dir: Option<PathBuf>, origin: Option<String>, listen: Option<SocketAddr> }
pub struct UnexTlsSettings { certificate: Option<PathBuf>, key: Option<PathBuf> }   // [unex.tls]: PEM files; both or neither
pub struct LiaisonSettings { listen: Option<SocketAddr>, identity: Option<PathBuf>, poll: Option<f64>,
    max_links: Option<u64>, plain_beyond_loopback: Option<bool>, peer: BTreeMap<String, LiaisonPeerSettings> }  // [unex.liaison]; defaults off, 1 s, 64, false; plain_beyond_loopback is a weakening
pub struct LiaisonPeerSettings { id: Option<String>, principal: Option<String>, addressees: Vec<String>,
    a2a: Option<bool>, url: Option<String> }   // [unex.liaison.peer.<name>]; universal_exports::liaison::LiaisonConfig::from_settings checks them
pub struct UnexSettings { url: Option<String>, ca_file: Option<PathBuf>, station_token: Option<SecretRef>,
    listen: Option<SocketAddr>, public_origin: Option<String>, proxy_secret: Option<SecretRef>,
    proxy_secret_min_bytes: Option<u64>, state_dir: Option<PathBuf>, plugins: PluginSettings,
    access: AccessSettings, pages: PagesSettings, tls: UnexTlsSettings, liaison: LiaisonSettings }  // a Station publishes with url and station_token; paths resolved beside the file
pub struct Bootstrap { provider: ProviderSettings, registry: RegistrySettings, unex: UnexSettings,
    station: StationSettings, oo: serde_json::Map<String, serde_json::Value> }  // oo: unread here
pub const DEFAULT_PAGES_LISTEN: SocketAddr;                              // 127.0.0.1:7083
impl ProviderSettings { pub fn poll(&self) -> Duration;                    // DEFAULT_POLL; 0 turns reload off
                        pub fn limits(&self) -> ConfigLimits; }
impl RegistrySettings { pub fn headquarters_lease(&self) -> Duration;      // DEFAULT_LEASE
                        pub fn sqlite_busy_timeout(&self) -> Duration; }   // 30 s

pub struct Located<T> { pub value: T, pub path: Option<PathBuf> }      // None: defaults
impl<T> Located<T> { pub fn dir(&self) -> Option<&Path>; }
impl Bootstrap {
    pub fn find(explicit: Option<&Path>) -> Result<Located<Self>, BootstrapError>;
    pub fn find_in(search: &SearchPath) -> Result<Located<Self>, BootstrapError>;
    pub fn read(path: &Path) -> Result<Self, BootstrapError>;
}
impl Located<Bootstrap> {
    pub fn state_dir(&self) -> Result<PathBuf, BootstrapError>;          // [station] state_dir, else platform_state_dir()
    pub fn provider_location(&self) -> Result<String, BootstrapError>;   // [provider] location, else the file's dir, else platform_config_dir()
    pub fn registry_location(&self) -> Result<String, BootstrapError>;   // [registry] location, else a SQL provider, else <state_dir>/registry
    pub fn registry_password(&self) -> Option<&SecretRef>;               // the provider's when the Registry is its database
    pub fn identity_dir(&self) -> Result<PathBuf, BootstrapError>;       // [station.identity] dir, else state_dir
    pub fn secrets_dir(&self) -> Result<PathBuf, BootstrapError>;        // [station.secrets] dir, else secrets/ beside the file
    pub fn listen(&self) -> SocketAddr;                                  // [station] listen
    pub fn weakenings(&self) -> Vec<String>;                             // each Env or Inline secret, a set [unex] ca_file
    // The gateway's own:
    pub fn unex_state_dir(&self) -> Result<PathBuf, BootstrapError>;     // [unex] state_dir, else platform_state_dir()
    pub fn unex_registry_location(&self) -> Result<String, BootstrapError>; // as registry_location, in unex_state_dir
    pub fn unex_listen(&self) -> SocketAddr;                             // [unex] listen
    pub fn proxy_secret_min_bytes(&self) -> u64;                         // [unex] proxy_secret_min_bytes, else 32
    pub fn pages_dir(&self) -> Result<PathBuf, BootstrapError>;          // [unex.pages] dir, else pages/ in unex_state_dir()
    pub fn pages_listen(&self) -> SocketAddr;                            // [unex.pages] listen
    pub fn liaison_identity(&self) -> Result<PathBuf, BootstrapError>;   // [unex.liaison] identity, else liaison/identity in unex_state_dir()
    pub fn unex_weakenings(&self) -> Vec<String>;
}
pub struct SearchPath { pub explicit: Option<PathBuf>, pub variable: Option<(String, PathBuf)>, pub candidates: Vec<PathBuf> }
impl SearchPath { pub fn new(name: &str, variable: &str, explicit: Option<&Path>) -> Self;
    pub fn locate(&self) -> Result<Option<PathBuf>, BootstrapError>; }   // NotFound for a named file that is missing
pub fn read_file<T: FromValue>(path: &Path) -> Result<T, BootstrapError>;
pub fn read_ca_file(path: &Path) -> Result<Vec<Vec<u8>>, BootstrapError>;  // the DER of each PEM certificate; refuses, naming path: unreadable, none in it, broken PEM, unparsable certificate
pub fn is_sql_location(location: &str) -> bool;
pub fn platform_config_dir() -> Result<PathBuf, BootstrapError>;  // XDG_CONFIG_HOME/ai6, %APPDATA%\ai6, ~/.config/ai6
pub fn system_config_dir() -> Option<PathBuf>;                     // /etc/ai6, %ProgramData%\ai6
pub fn platform_state_dir() -> Result<PathBuf, BootstrapError>;   // %LOCALAPPDATA%\AI6, XDG_STATE_HOME/AI6, ~/.local/state/AI6
#[non_exhaustive] pub struct BootstrapError;  // kind(): NotFound, InvalidData, ...; Into<io::Error>
// format: EXTENSIONS, Format { Json, Toml, Yaml } (name, for_path, for_extension),
//   parse(path, text) -> guatiao::Value, read::<T: FromValue>(path, text),
//         from_str<T>(Format, &str), to_string<T>(Format, &T), FormatError
// public: public_file(&Bootstrap) -> Result<PublicFile { text, left_out }, PublicError>
```

`StationConfig::with_state_dir`, `with_protected_path` (a directory an assistant's
interpreters may never read; the state and secrets directories always are, sent as
`AssistantMode::station_paths`) and `with_bootstrap_weakenings` (sent at check-in as
`bootstrap_weakened`), and `ServeConfig::with_secrets_dir`, carry the file into a Station.

## Configuration providers (`ai6::directorates::m::config`)

Where a Station's configuration comes from: records of each family (`ConfigRecord`) at a
scope (`ConfigScope`), global or one Station's. A Station's view is its own records over the
global ones, key by key (a singleton's one record replaces the global one whole); the
provider hands it out checked and built. "Provider" here is a configuration provider, not a
Guatiao provider.

```rust
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConfigRevision;   // opaque; new(u64), get()
#[non_exhaustive] #[derive(Clone, Default)]   // the non-roster families, for one Station
pub struct Settings {
    pub theaters: Theaters,
    pub loadouts: LoadoutCatalog,            // loadout, sandbox, mcp, overlay, secret, pool records
    pub cables: CableConfig,                 // default denies every action
    pub pins: Option<PinSet>,                // plugin records; None when there are none
    pub station: StationRuntime,             // station record
    pub tls: Option<serde_json::Value>,      // tls record, a coqui-peer-tls Policy document
    pub access: Option<serde_json::Value>,   // access record, an AccessPolicy document
    pub operation_access: BTreeMap<String, OperationAccess>,   // operation_access records, by Operation ID
    pub unex: UnexRecord,                    // unex record
    pub limits: ConfigLimits,                // what the provider read these within
}
// [provider] limits of the bootstrap file; also at the crate root. Default: 1 MiB, 4,096, 1 MiB.
#[non_exhaustive] #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigLimits { pub loadout_bytes: Limit, pub loadout_value_chars: Limit, pub access_policy_bytes: Limit }
impl ConfigLimits { pub fn weakenings(&self) -> Vec<String>; }   // "provider.KEY VALUE is looser than the default D"
impl Settings {
    pub fn access_policy(&self) -> Result<Option<AccessPolicy>, String>;   // none: no record
    #[cfg(feature = "station-tls")]
    pub fn tls_policy(&self) -> Result<coqui_peer_tls::Policy, String>;  // over Policy::default()
}
// Deserialize + Serialize, deny_unknown_fields, every key optional:
pub struct StationRuntime { max_concurrent_missions, stall_timeout, loop_timeout, loop_min_lines,
    blocked_after, max_runtime, history, normalize_digits, roles, labels, terminal,
    resources_every, safe_house_caches, safe_house_cache_age,   // seconds as f64
    partial_line_bytes: Option<Limit>,
    // the tables, each #[serde(default)]: see "Station runtime limits"
    scheduler, publisher, sources, transports, recovery, vetting, accounts, approvals, harness,
    provision, secrets, terminal_output, green_ink, detached, runner, api, audit, theaters,
    theater_tools, head_of_station, probes,
    providers: BTreeMap<String, serde_json::Value> }   // [providers.<id>], each a table
impl StationRuntime { pub fn validate(&self) -> Result<(), String>;   // includes StationLimits::validate
    pub fn limits(&self) -> StationLimits;                             // the tables and providers
    pub fn duration(seconds: Option<f64>) -> Option<Duration>; }
pub struct UnexRecord { authenticator, authenticator_config, directory, directory_config,
    authorizer, authorizer_config, proxy_user_header, proxy_lookup_by /* "name" */,
    page_authenticator, page_authenticator_config /* the pages' request-taking authenticator */,
    login_tries_per_address, login_tries_per_name, client_address_header, access_cache_ttl,
    access_cache_negative_ttl, access_cache_entries, assistant /* true */,
    assistant_agent /* "moneypenny" */, route_text_bytes /* None: 128 */,
    provider_config_depth: Limit /* 64 */,
    gateway: UnexGatewayLimits, controls: ControlLimits, timeline: UnexTimelineLimits,
    login: LoginTable, text: TextLimits }          // [gateway] [controls] [timeline] [login] [text]
impl UnexRecord { pub fn weakenings(&self) -> Vec<String>;   // inline credentials, and each looser limit
    pub fn limits(&self) -> UnexLimits; }                   // the five tables together
pub struct UnexLimits { pub gateway, pub controls, pub timeline, pub login, pub text }
impl UnexLimits { pub fn validate(&self) -> Result<(), String>;   // zeros where nothing could pass
    pub fn weakenings(&self) -> Vec<String>; }
// bootstrap: UnexSettings.proxy_secret_min_bytes (32, DEFAULT_PROXY_SECRET_MIN_BYTES), smaller is a weakening
#[non_exhaustive] #[derive(Clone)]
pub struct ConfigSnapshot { pub revision: ConfigRevision, pub roster: Roster, pub settings: Settings }
impl ConfigSnapshot {
    pub fn new(revision: ConfigRevision, roster: Roster, settings: Settings) -> Self;
    /// Families whose records differ; snapshots built with `new` carry none and differ in none.
    pub fn changed_families(&self, other: &Self) -> Vec<ConfigRecord>;
}
pub trait ConfigProvider: Send + Sync {
    /// `station`'s view; "" is the global records only.
    fn load(&self, station: &str) -> Result<ConfigSnapshot, RosterError>;
    fn changes_since(&self, station: &str, since: ConfigRevision) -> Result<Option<ConfigSnapshot>, RosterError>;
    fn revision(&self, station: &str) -> Result<ConfigRevision, RosterError>;
    /// One family's records at one scope as JSON objects, for an administrator; default ReadOnly.
    fn definitions(&self, record: ConfigRecord, scope: &ConfigScope) -> Result<ConfigDefinitions, RosterError>;
    /// Store / remove one record, checking every view it touches first; default ReadOnly.
    fn put(&self, record: ConfigRecord, scope: &ConfigScope, key: &str, definition: &serde_json::Value)
        -> Result<ConfigRevision, RosterError>;
    /// Store only when the stored record is `expected` (None: none); Conflict otherwise. SQL
    /// checks and stores in one transaction; the default reads then puts.
    fn put_if(&self, record: ConfigRecord, scope: &ConfigScope, key: &str,
        expected: Option<&serde_json::Value>, definition: &serde_json::Value)
        -> Result<ConfigRevision, RosterError>;
    fn remove_on_host(&self, record: ConfigRecord, scope: &ConfigScope, key: &str)
        -> Result<Option<ConfigRevision>, RosterError>;   // for someone on the host: `remove` by default; FileProvider edits its files, Universal Exports never does
    fn remove(&self, record: ConfigRecord, scope: &ConfigScope, key: &str)
        -> Result<Option<ConfigRevision>, RosterError>;   // None: there was none
    /// Several records as one change: all or none (default: put one by one).
    fn put_all(&self, scope: &ConfigScope, records: &[(ConfigRecord, String, serde_json::Value)])
        -> Result<Option<ConfigRevision>, RosterError>;
    /// Global and each Station scope that holds records (default: Global).
    fn scopes(&self) -> Result<Vec<ConfigScope>, RosterError>;
}
pub const IMPORT_ORDER: [ConfigRecord; 24];   // each family after those it refers to
pub fn family_document(record: ConfigRecord, records) -> Result<String, RosterError>;   // TOML
pub fn export(provider: &dyn ConfigProvider, dir: &Path) -> Result<usize, RosterError>;
#[non_exhaustive] #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Display)]
pub enum ConfigRecord { Operation, Agent, Role, Assistant, Cover, Theater, Loadout, Sandbox,
    Mcp, Overlay, Secret, Pool, Cables, Plugin, Station, Tls, Access, OperationAccess, Unex,
    Roster, Compartment, Review, Watch }        // Watch: `[watch.<scope>]`, file `watches`, global
impl ConfigRecord {
    pub const ALL: [Self; 24];                  // Roster first
    pub const SINGLETON_KEY: &'static str;      // "settings": a singleton family's one record
    pub const fn name(self) -> &'static str;    // "operation", "agent", ... "unex", "roster"
    pub fn from_name(name: &str) -> Option<Self>;
    pub const fn file(self) -> &'static str;    // operations (Operation, Roster) | agents | covers
                                                // | theaters | loadouts | cables | plugins | station
                                                // | tls | access | unex
    pub const fn is_singleton(self) -> bool;    // Assistant, Cables, Station, Tls, Access, Unex, Roster
    pub const fn is_whole_file(self) -> bool;   // the singletons but Assistant and Roster
    pub const fn station_scoped(self) -> bool;  // Theater, the loadout families, Cables, Plugin, Station, Tls
    pub const fn applies_live(self) -> bool;    // all but Theater, Cables, Plugin, Station, Tls, Unex
}
#[non_exhaustive] #[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Display)]
pub enum ConfigScope { Global, Station(String) }
impl ConfigScope {
    pub fn as_text(&self) -> String;              // "" | "station:<id>"
    pub fn parse(text: &str) -> Option<Self>;    // None for an id that is not one plain component
    pub fn is_plain(id: &str) -> bool;            // not empty, ., .., nor holding / \ :
    pub fn of_station(station: &str) -> Self;     // Global for ""
}
#[non_exhaustive] #[derive(Clone, PartialEq)]
pub struct ConfigDefinitions {
    pub revision: ConfigRevision,
    pub records: Vec<(String, serde_json::Value)>,   // by key
    pub writable: bool,                              // the provider takes put/remove
}   // new(revision, records, writable), for a provider built outside this crate
/// Check-in key `config` of every Station (serde; empty fields are omitted).
#[non_exhaustive] #[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigStatus {
    pub revision: Option<String>,    // decimal: a digest can pass 2^53
    pub refused: Vec<String>,        // why the newest configuration is refused
    pub needs_restart: Vec<String>,  // restart-only family names ("theater", "cables")
}
pub struct FileProvider;     // ConfigProvider (+ definitions, read-only); new(root), root(),
                             // with_limits(ConfigLimits)
pub struct MemoryProvider;   // ConfigProvider (read-only); new(roster), with_settings(Settings),
                             // with_limits(ConfigLimits), replace(&self, roster),
                             // replace_all(&self, roster, settings)
/// `sqlite:PATH` / `sqlite://PATH` / `postgres://…` → SqlProvider; anything else →
/// FileProvider. Unsupported for a SQL location without its feature.
pub fn open(location: &str) -> io::Result<Arc<dyn ConfigProvider>>;
/// As `open`, a SQL provider connected with `options` (features `sqlite` or `postgres`), and
/// either provider reading documents within `limits`.
pub fn open_with(location: &str, options: &SqlOptions, limits: ConfigLimits) -> io::Result<Arc<dyn ConfigProvider>>;

// Features `sqlite` or `postgres`; also at the crate root.
pub struct SqlProvider;   // ConfigProvider with definitions/put/remove + Debug
impl SqlProvider {
    pub fn open(location: &str) -> io::Result<Self>;   // as SqlRegistry::open
    pub fn open_with(location: &str, options: &SqlOptions) -> io::Result<Self>;
    pub fn with_limits(self, limits: ConfigLimits) -> Self;
}
```

A provider hands out only checked configuration: an unacceptable one is an error
(`RosterErrorKind::Invalid`; a database or file that cannot be read is `Unavailable`), and
every Agent is resolved (`Roster::check_agents`). A record of a global-only family at a
Station scope, or a singleton under another key, is `Invalid`.

`FileProvider` reads one file per family in its directory, `<stem>.<ext>` with `ext` one of
`cfg`, `toml`, `json`, `yaml`, `yml` (`ai6::bootstrap::format`), and a Station's own records
from `stations/<id>/` beside them. Each top-level table of a file names a family that file
holds: `operations` (`[operation.<id>]`), `agents` (`[agent.<codename>]`, `[role.<id>]`,
`[assistant]`), `covers` (`[cover.<name>]`), `theaters` (`[theater.<id>]`), `loadouts`
(`[loadout.*]`, `[sandbox.*]`, `[mcp.*]`, `[overlay.*]`, `[secret.*]`, `[pool.*]`); `plugins` is `[[library]]`
entries keyed by `file`; `cables`, `station`, `tls`, `access` and `unex` are each one record,
the whole file; `operation_access` (`[operation_access.<id>]`) is one record per Operation. Anything else is `Invalid`, naming the table, and two files for one family
are `Invalid`, naming both. `ai6.cfg` is never a family file; no family has its stem. A
relative skill path in `[assistant]` is relative to the directory. Its revision is a digest of
every family file of the view (global and the Station's own) with their relative paths, taken
before they are parsed, so `changes_since` reads them but parses nothing while they are
unchanged. `definitions` lists one scope's records (`Assistant` as one record keyed
`settings`); `put`/`remove` are `ReadOnly`: the files are edited by hand or version control.
`MemoryProvider` gives every Station the same view; it starts at revision 0 and moves to the
next on each `replace`. Any other store builds its roster with `Roster::from_parts`.

The daemon loads its own view at start (`ServeConfig::with_config_provider`; default
`FileProvider` over the configuration directory) and polls `revision(station)` every
`DEFAULT_CONFIG_POLL` (5 s; `StationConfig::with_config_poll`, `ai6.cfg [provider] poll`,
0 off): a new revision is loaded and its live families applied to requests, the scheduler
and new Sessions (running Sessions keep theirs); a changed family that only a restart applies
(`theater`) is logged and listed in `config.needs_restart` by name. A revision that fails with
`Invalid` is refused once per revision, logged, audited (`config.refused`, actor and resource
`station:<id>`) and reported at check-in (`config.refused`) while the last good configuration
keeps running. Loadouts apply live (unless they were set in code), and the Station then
declares its pool accounts again; `cables`, `plugin`, `station` and `tls` apply at restart like
`theater`. The bootstrap file and `ServeConfig` settings apply only at start.

`SqlProvider` keeps one table per family, `ai6_config_<name>` (`scope`, `key`, `definition`,
`revision`; primary key `(scope, key)`): a row is a record's scope text, key and a JSON object
with the keys of its table in the family file (`deny_unknown_fields` as there). `load` parses
each with the file loader's types and checks, resolves Agents' `extends`, adds the cover rows
over the built-in covers and builds the roster with `Roster::from_parts`; an Operation's
trusted `.ai6/agents.toml` still applies on top. The `assistant` row is keyed `settings`;
removing it restores the default settings. The revision is the `config` row of `ai6_clock`,
read before the rows; `changes_since` reads only it. `put` and `remove` change one record and
check, in the same transaction, the views the change touches (the global view and each
Station's with records of its own for a global change; that Station's for one of its own),
refusing and rolling back a change that leaves one unacceptable; each change advances the
revision. A writer that edits the tables directly must advance that row in the same
transaction, or Stations do not see the change. The database may be the Registry's.

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
    pub exit_grace_ms: Option<u64>,          // 1000: output awaited after a command exited
    pub keepalive_secs: Option<u64>,         // 30; 0 sends none
    pub keepalive_max: Option<usize>,        // 3 unanswered probes
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
pub enum CableReach { Required, Optional }  // default Required; TOML `cables`: "required" | "optional"
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
    pub connect_timeout_ms: Option<u64>,     // default 30000
    pub initialize_timeout_ms: Option<u64>,  // default 20000
    pub request_timeout_ms: Option<u64>,     // default 30000
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
    pub cables: CableReach,                  // Optional: a wrapped harness with no `forward` starts
                                             // without Cables, a weakening; omitted when Required
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
    pub fn with_cables(self, cables: CableReach) -> Self;
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
    pub cables: CableReach,                  // omitted when Required
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
    pub fn new(name: impl Into<String>) -> Result<Self, String>;           // [A-Za-z0-9._+-], not leading - or .
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
/// A program name longer than `limits.name_bytes` fails the probe.
pub fn probe(client: &TransportClient, capabilities: &Capabilities, requires: Option<&Requirements>,
             dir: Option<&str>, limits: &ProvisionLimits) -> ProbeReport;
/// CPUs (nproc), 1-minute load, MemTotal/MemAvailable; under cgroup v2 the container's
/// cpu.max quota and memory.max (less memory.current) narrow them; macOS via sysctl (no
/// available memory). Never cpu_percent. Err: no `run`, a Windows host, nothing recognised.
pub fn probe_resources(client: &TransportClient, capabilities: &Capabilities, limits: &ProvisionLimits)
    -> Result<Resources, String>;
/// Steps in order: os, disk, tools, harnesses, runner, plugins. `check` writes nothing.
pub fn provision(client: &TransportClient, theater: &Theater, check: bool, limits: &ProvisionLimits)
    -> Vec<ProvisionStep>;
```

Both block on the transport and need a POSIX `sh` there (`command -v`, `df -Pk`, `sha256sum`
or `shasum -a 256`, `chmod`, `mv`). The probe names ambient credentials (git credential store
or helper, SSH agent or private keys, GitHub CLI, Claude Code and Codex logins, provider keys
in the login environment) and never reads their values. A copied file is written to
`<dest>.ai6-partial` and moved over `<dest>`, so a running runner binary can be replaced; it
is copied only when its SHA-256 differs. `runner.command` must be a path (`~/` = the
transport's default directory); without `provision.runner` the Station's own executable is
used only when the Theater reports the Station's `os` and `arch`. The `ai6 theater
provision THEATER [--check] [--config PATH] [--plugin-path P]...` command prints one line
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
pub fn run_process(command: &mut Command, input: &ProcessInput<'_>, reader_grace: Duration)
    -> Result<Output, ProviderError>;
pub const DEFAULT_READER_GRACE_MS: u64 = 1_000;   // a transport's `reader_grace_ms` default
pub fn io_error(error: &io::Error, what: &str) -> ProviderError;   // NotFound -> GUATIAO_ERR_NOT_FOUND
pub trait Shell { fn sh(&self, script: &str, stdin: Option<&[u8]>, max_output: u64) -> Result<Output, ProviderError>; }
pub fn read_file(shell: &dyn Shell, spec: &ReadSpec) -> Result<FileData, ProviderError>;
pub fn write_file(shell: &dyn Shell, spec: &WriteSpec) -> Result<(), ProviderError>;
pub fn list_dir(shell: &dyn Shell, path: &str) -> Result<DirListing, ProviderError>;
pub const EXIT_NOT_FOUND: i64 = 44;
```

`run_process` kills a process at its timeout (not its descendants), keeps at most
`max_output` bytes of each stream and gives the output readers `reader_grace` to finish once the
program has ended. The file functions are POSIX `sh` scripts over `Shell`
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
pub struct AgentFacts { pub id: String, pub handler: Option<String>, pub model: Option<String>, pub priority: i64, pub roles: Vec<String>, pub tags: Vec<String> }
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
#[non_exhaustive] pub enum OnLimit { Rotate /* default */, Wait, Backup }   // serde: "rotate" | "wait" | "backup"
impl OnLimit { pub fn as_str(self) -> &'static str }
pub fn parked_until(now: i64, resets_at: Option<i64>, fallback_secs: u64, grace_secs: i64) -> i64
```

`parked_until` honours a reset time in the future plus `grace_secs` (a Station passes
`[accounts] reset_grace`, 15 s by default); a missing or
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
pub trait BriefStore;        // proposals, queued briefs, claims, leases, outcomes (store.rs);
                             // filed_brief(key), filed_briefs(operation, limit) -> FiledBrief { brief, state, attempts, outcome }
                             // request_cancel(key, at) -> io::Result<bool>: keeps the first time a cancel of the
                             //   claimed brief was asked (false: not claimed); finishing forgets it, a lapse back to
                             //   the queue keeps it. cancel_requests() -> io::Result<BTreeMap<key, at>>: claimed ones.
                             //   File: cancels/KEY.json; SQL: ai6_briefs.cancel_requested_at (step briefs.cancel_requested_at)
pub trait StationStore;      // check-ins, Station settings, Headquarters election (store.rs)
pub trait AccessTokenStore;  // Universal Exports access tokens (store.rs)
pub trait CableStore;        // Cables, their delivery state and send quotas (store.rs)
pub trait AuditStore;        // audit records (store.rs)
pub trait AgentRoleStore;    // the per-Operation Agent-role overlay (store.rs)
pub trait AgentRecordStore;  // Agent track records (agent_records.rs)
pub trait ClearanceStore;    // clearance grants per Agent and Operation (clearance.rs)
pub trait ApprovalStore;     // green ink and class authorisations (approvals.rs)
pub trait AssistantLogStore; // Moneypenny's conversation log (assistant_log.rs)
pub trait LogbookStore;      // each Station's Head of Station reports (logbook.rs)
pub trait QuarantineStore;   // quarantines and incidents (quarantine.rs)
pub trait SessionStore;      // Session records and fenced leases (sessions.rs)
pub trait SessionContextStore; // each Session's harness context (contexts.rs)
pub trait SleeperStore;      // Sleeper authorizations and fired windows (sleepers.rs)
pub trait SourceStore;       // Source cursors, ingested items, reports owed (sources.rs)
pub trait TimelineStore;     // the activity timeline (timeline.rs)
pub trait UsageStore;        // usage ledgers and daily spend (usage.rs)
pub trait AccountStore;      // pool account claims, quota and daily tokens (accounts.rs)
pub trait VettingStore;      // held briefs and vetting clearances (vetting.rs)
pub trait WatchStore;        // each watch's record, lease and logbook (watches.rs)
pub trait Limited { fn limits(&self) -> &RegistryLimits; }   // the bounds it was opened with
pub trait RegistryBackend: Limited + AccountStore + AgentRecordStore + BriefStore + StationStore + AccessTokenStore + CableStore
    + AuditStore + AgentRoleStore + ApprovalStore + AssistantLogStore + ClearanceStore + LogbookStore
    + QuarantineStore + ReportStore + SessionContextStore + SessionStore + SleeperStore + SourceStore + TimelineStore
    + UsageStore + VettingStore + WatchStore + Send + Sync {}
impl<T: /* every family */ + Send + Sync> RegistryBackend for T {}

/// `sqlite:PATH` / `sqlite://PATH` / `postgres://…` / `postgresql://…` → SqlRegistry;
/// anything else → FileRegistry (a directory, not touched until used).
pub fn open(location: &str) -> io::Result<Arc<dyn RegistryBackend>>;
/// As `open`, either store applying `options.limits()`, a SQL Registry connected with
/// `options` (features `sqlite` or `postgres`).
pub fn open_with(location: &str, options: &SqlOptions) -> io::Result<Arc<dyn RegistryBackend>>;
```

## Registry limits (`ai6::directorates::registry::limits`, also at the crate root)

```rust
// [registry.limits] of ai6.cfg: one table per family, each #[non_exhaustive],
// ToValue, FromValue, Schema, deny_unknown_fields, Default = the documented defaults
// (docs/configuration.md "Limits"). Counts and sizes are Limit, ages TimeLimit, days u32.
pub struct RegistryLimits { pub accounts: AccountLimits, pub agents: AgentLimits, pub agent_roles: AgentRoleLimits,
    pub approvals: ApprovalLimits, pub assistant_log: AssistantLogLimits, pub audit: AuditLimits,
    pub briefs: BriefLimits, pub cables: CableLimits, pub clearance: ClearanceLimits,
    pub contexts: ContextLimits, pub files: FileLimits, pub held: HeldLimits, pub logbook: LogbookLimits,
    pub proposals: ProposalLimits, pub quarantine: QuarantineLimits, pub reports: ReportLimits,
    pub sessions: SessionLimits,
    pub sleepers: SleeperLimits, pub sources: SourceLimits, pub stations: StationLimits,
    pub timeline: TimelineLimits, pub tokens: TokenLimits, pub usage: UsageLimits,
    pub watches: WatchLimits }
impl RegistryLimits {
    pub fn weakenings(&self) -> Vec<String>;   // "registry.limits.T.KEY VALUE is looser than the default D"
    pub fn held_record_bytes(&self) -> Limit;  // briefs.record_bytes + 64 KiB
}
impl LogbookLimits { pub fn record_bytes(&self) -> Limit; }       // a whole logbook, from its four keys
impl QuarantineLimits { pub fn history_bytes(&self) -> Limit; }   // 16 × record_bytes
// StationLimits::gone_dark_after is Seconds (45): pass `registry.limits().stations.gone_dark_after.0`
// where a Station or Universal Exports judges a check-in dark.
```

## Bounds (`ai6::bounds`, also at the crate root)

```rust
pub struct Limit;       // u64 or no bound; reads a number, null or "none"; writes a number or null
impl Limit { pub const UNBOUNDED: Self; pub const fn at(n: u64) -> Self; pub const fn get(self) -> Option<u64>;
    pub fn allows<N: TryInto<u64>>(self, n: N) -> bool; pub fn or_max(self) -> usize; }   // Display: n or "none"
pub struct Rate;        // a number per second or no bound, same null / "none"; at(f64), get(), UNBOUNDED, Display
pub struct Seconds(pub Duration);   // fractional seconds, finite and not negative; Serialize + Deserialize
pub struct TimeLimit;   // Seconds or no bound, same null / "none" rule
impl TimeLimit { pub const UNBOUNDED: Self; pub const fn at(d: Duration) -> Self; pub const fn secs(s: u64) -> Self;
    pub const fn get(self) -> Option<Duration>; pub fn allows(self, d: Duration) -> bool;
    pub fn allows_secs(self, s: f64) -> bool; pub fn as_secs_f64(self) -> f64; }   // INFINITY when unbounded
/// One line per setting in `current` looser than in `default` (both serialized): larger, or
/// null; a key in `smaller_is_looser` (by name or path) the other way round.
pub fn looser(prefix: &str, current: &Value, default: &Value, smaller_is_looser: &[&str]) -> Vec<String>;
```

Every trait is re-exported at the crate root. The methods are synchronous; async callers run
them on a blocking thread. Calls through `dyn RegistryBackend` need no import; calling them
on a `FileRegistry` or `SqlRegistry` value needs the family trait in scope
(`use ai6::directorates::registry::backend::*`). The sections below give each family's
methods under the module that defines it. `FileRegistry` keeps them in one directory every
Station shares; `SqlRegistry` (features `sqlite`, `postgres`) in a database. A SQL location
without its feature is `Unsupported`. The daemon takes `Arc<dyn RegistryBackend>` through
`ServeConfig::with_registry` (default: the `FileRegistry` in `registry` under its state
directory), Universal Exports through `Gateway::with_registry`, and `CableRelay::new` takes
one too. The `ai6` and `universal-exports` commands open the location their bootstrap file
names (`Located::registry_location`, below).

## SQL Registry (`ai6::directorates::registry::sql`, features `sqlite` or `postgres`)

```rust
#[derive(Debug)]
pub struct SqlRegistry;   // every Registry family; also at the crate root
impl SqlRegistry {
    /// `sqlite:PATH` or `sqlite://PATH` (a SQLite file), or a `postgres://` /
    /// `postgresql://` URL whose parameters pass to the server (such as
    /// `options=-c%20search_path%3Dai6` for a schema), besides `sslmode` and `sslrootcert`,
    /// read here. Creates or upgrades the tables.
    pub fn open(location: &str) -> io::Result<Self>;   // open_with(location, &SqlOptions::default())
    pub fn open_with(location: &str, options: &SqlOptions) -> io::Result<Self>;
}

#[derive(Clone, Debug /* password shown as *** */)]
pub struct SqlOptions;   // also at the crate root; Default = for_lease(DEFAULT_LEASE)
impl SqlOptions {
    pub fn for_lease(lease: Duration) -> Self;   // timeouts derived from the HQS lease
    pub fn with_password(self, password: impl Into<String>) -> Self;
    pub fn with_limits(self, limits: RegistryLimits) -> Self;    // what the opened Registry applies
    pub fn with_busy_timeout(self, timeout: Duration) -> Self;   // SQLite lock wait; default 30 s
    pub fn limits(&self) -> &RegistryLimits;
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
connects at once. It makes what the database lacks and runs no DDL when nothing is, so a role with
no `CREATE` right starts on a whole database; `PermissionDenied`, naming the role, the schema and
what is missing, when that role cannot make it. Writes also refuse, `Unsupported`, while the database's schema version is
newer than this build's.

TLS: `sslmode=verify-full` is the default: TLS (rustls, ring), with the server's certificate
checked against the platform's roots (`rustls-native-certs`), or, as libpq does, only
against `sslrootcert=PATH` (PEM) when given (`sslrootcert=system` keeps the platform's),
and its name or IP address checked. `sslmode=require` encrypts but accepts any certificate;
`sslmode=disable` sends everything in clear. Both are accepted and named by `weakenings`;
`ai6 serve` and `universal-exports` print them at start. A password comes from the location,
or from `SqlOptions::with_password` when the location has none (the commands pass their
bootstrap file's `[provider]` and `[registry] password`); a password in the location is
visible in the process list. Messages from the command show locations through `redact`.

Timeouts (`SqlOptions::for_lease(L)`, no floor; a sub-millisecond server timeout is sent as
1 ms, since PostgreSQL reads 0 as none): each session runs
`statement_timeout`, `lock_timeout` and `idle_in_transaction_session_timeout` of `L/4`, so a
Station that stalls inside a transaction (a paused VM, a half-open connection) loses its
session, and the clock row it held, well before a standby must take over; the connection
uses `connect_timeout` and `keepalives_idle` of `L/6` and `tcp_user_timeout` of `L/4`. A
location that sets one itself (the parameter, or `-c name=…` in `options`) keeps its own.
`ai6 serve` derives them from its `[registry] headquarters_lease`. A lock or statement timeout maps
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
READ ONLY`, whatever the server's default, and serializes each open's check of one schema with an
advisory lock. Brief, Headquarters and Session leases and check-in times
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
pub fn record_digest(brief: &Brief) -> String;  // "sha256:HEX" of the canonical JSON record without provenance, mission_id or handle
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
    pub fn try_new(station: impl Into<String>, max_concurrent_missions: usize) -> io::Result<Self>; // >= 1; default ResourceLimits
    pub fn with_resource_limits(self, limits: ResourceLimits) -> io::Result<Self>; // max_cpu_percent > 100 refused
    pub fn resource_limits(&self) -> ResourceLimits;
    pub fn station(&self) -> &str;
    pub fn max_concurrent_missions(&self) -> usize;
}
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
impl MissionProposal {
    pub fn try_new(operation_id, parent_mission_id: MissionId, proposer_agent_id, proposer_session_id: SessionId,
        idempotency_key: &str, brief: Brief, limits: &ProposalLimits) -> io::Result<Self>;   // key within limits
    pub fn with_parent_role(self, Option<String>) -> Self;   // set by the Station, never the Agent
}
BriefStore::submit_mission_proposal(&MissionProposal) -> io::Result<()>   // a retry keeps the stored vetting
BriefStore::list_mission_proposals(operation: &str, limit: usize) -> io::Result<Vec<MissionProposal>>
BriefStore::promote_mission_proposal(operation: &str, proposal_id: &str, received_by: &str) -> io::Result<Brief>
BriefStore::reject_mission_proposal(operation: &str, proposal_id: &str) -> io::Result<MissionProposal>
BriefStore::set_proposal_vetting(operation: &str, proposal_id: &str, vetting: &Vetting) -> io::Result<MissionProposal>  // NotFound unless pending
```

`FileRegistry::open(root)` opens the file-backed Registry without creating or changing
files. `list_new()` returns valid
version-1 `Brief` values ordered by priority descending, creation time ascending, then
key. It skips malformed briefs, rejects unknown fields and
newer versions, and bounds scans and individual briefs by `[registry.limits.briefs]`
(`records`, 1,000; `record_bytes`, 1 MiB). `FileRegistry::with_limits(RegistryLimits)` applies
other limits; `open` keeps the defaults. Listing
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
allot_mission_id(entry: &MissionEntry) -> io::Result<MissionHandle>
entered_mission_id(entry: &MissionEntry) -> io::Result<Option<String>>  // the UID
resolve_mission(id: &str) -> io::Result<Option<MissionHandle>>  // UID, dropped key or any handle ever given, without case
rename_mission(mission_id: &str, handle: &str) -> io::Result<MissionHandle>  // NotFound / InvalidInput / AlreadyExists
enter_brief(brief: &Brief, entry: &MissionEntry) -> io::Result<Brief>  // AlreadyExists when the entry's brief is in any state
reenter_brief(brief: &Brief, entry: &MissionEntry, again: &dyn Fn(&Brief) -> bool) -> io::Result<Reentry>
    // the Mission `entry` names, finished and accepted by `again`, queued again as `brief`: same UID
    // and handle; the claim count starts again, so `recover`'s `max_attempts` bounds this attempt's
    // lapses alone; its outcome, report marks and spent provenance dropped; provenance is the brief's own
#[non_exhaustive] pub enum Reentry { Queued(Brief), Unchanged(FiledBrief) /* in flight, or refused */, Unknown }
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

A Mission's ID is a UID (UUIDv7, lower-case text) minted where a brief enters and kept in
`Brief::mission_id`; `Brief::key()` is that UID, or `{source}--{id}` for a brief that has none
(one dropped into the queue by hand). Every relation uses the UID. What people see is
`Brief::handle`, given by the rule in `registry::mission_ids` and changed only by
`rename_mission`; `Brief::shown_handle()` is it or the key, and `Brief::mission_handle()` both.
No brief digest (provenance, clearance, review) covers the Mission ID or the handle, so a rename
or a new key leaves them valid.
`Brief::id` stays what the brief is called where it came from. `Brief::name` is what to show
beside the handle when the title is not (an issue's title); `Brief::display_name(bytes)` gives
it, or the title, on one line. A Mission keeps it as `Mission::name` (and the handle as
`Mission::handle`, as the Station last learned it) and a published brief as `name`; it is never
part of `Brief::prompt`.

```rust
pub struct MissionHandle { pub mission_id: String, pub handle: String }  // UID and current handle
pub fn new_uid() -> String;                        // UUIDv7, lower-case
pub fn is_uid(text: &str) -> bool;                 // hyphenated lower-case UUID text
pub fn valid_handle(handle: &str) -> bool;         // [A-Za-z0-9_-], no '-' at either end, <= 240, not a UID
pub fn run_handle(mission_handle: &str, attempt: u32) -> String;  // `<handle>.<attempt>`
pub struct MissionEntry;  // where a brief enters; all of these are the only way a handle is made
impl MissionEntry {
    pub fn person(operation: Option<&str>) -> Self;              // <operation>-<n>; `mission` with none
    pub fn proposal(operation: &str, proposal_id: &str) -> Self; // <operation>-<n>, once per proposal
    pub fn review(operation: &str, review_id: &str) -> Self;     // <operation>-<n>, once per review
    pub fn sleeper(operation: &str, name: &str, window: &str, station: Option<&str>) -> Self; // <operation>-<name>-<n>
    pub fn standing(operation: &str, name: &str, station: Option<&str>) -> Self; // <operation>-<name>-<n>, one for every window
    pub fn follow_up(operation: &str, instance: &str, item: &str) -> Self;
        // the item's Mission's handle now and its next follow-up number from 2 (`G-gh-31-2`); a new
        // Mission each time; NotFound when the item has no Mission
    pub fn source(operation: &str, instance: &str, item: &str, code: Option<&str>, tag: Option<&str>) -> Self;
        // <operation>-<tag>-<code>, <operation>-<code> with no tag, <operation>-<instance>-<item> with no code
}
```

`allot_mission_id` returns the Mission an entry names, with its current handle, or mints a UID
and gives a handle: atomically per Operation across processes (the file Registry's lock; the SQL
Registry's write transaction), numbers never reused, however a brief later moves or is deleted. A
person's entry is new each time; every other entry keeps its Mission. Only ASCII letters, digits,
`_` and `-` stay in a handle. No two handles differ only by case, none is given twice, a number
or a code whose handle is taken is skipped or takes a short hash, and none reads as a UID. A
brief's `mission_id` must be a UID, and its `handle` needs one and the handle character set, or
the brief is invalid. `rename_mission(uid, handle)` refuses a malformed handle
(`[registry.limits.briefs] handle_bytes`), one another Mission was ever given, and a brief with
no handle; it rewrites the handle on every copy of the brief (each state, the held record) and
keeps the old one in the book naming the Mission. Stored under `mission-ids/` (file: `counters/`,
`issued/<folded handle>.json` = `{mission_id, handle, entry?}`, `entries/<digest>.json` the same
for the entry's current handle), `ai6_mission_counters` and `ai6_mission_ids` (SQL: `folded`,
`handle`, `mission_id`, `entry` on the current handle's row, `revision`).

`enter_brief` allots and queues in one step, which is how every entry point in AI6 but the
gateway's queues a brief (the gateway allots first, to audit the ID before it writes).
`submit_brief` validates and atomically writes a unique Mission brief to `new/` under the key it
already has; it refuses a key already present in `new/`, `claimed/`, `done/` or `failed/`. Its
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
#[non_exhaustive]
pub struct TimelineEvent {
    pub id: String, pub at: f64,            // fresh UUID; Unix seconds
    pub kind: String,                       // "lifecycle" | "control" | "cable" | "sleeper"
    pub action: String,                     // a state, "<action>.<outcome>", "sent", "authorized", "revoked"
    pub operation: String,                  // whose timeline shows it
    pub station: Option<String>, pub mission: Option<String>,
    pub session: Option<String>, pub actor: Option<String>,
    pub detail: BTreeMap<String, String>,   // at most detail_entries (8) safe names and IDs
}
impl TimelineEvent {
    pub fn new(kind, action, operation, at: f64) -> Self;
    pub fn station(self, ..) -> Self; pub fn mission(self, ..) -> Self;
    pub fn session(self, ..) -> Self; pub fn actor(self, ..) -> Self;
    pub fn detail(self, key, value) -> Self;
}
```

`record_event` refuses (`InvalidInput`) a kind, action or detail key that is not a bounded
lowercase name (`a-z0-9._-`, `name_bytes`), an identity or detail value that is blank, over
`text_bytes` or holds control characters, more than `detail_entries` details, or a
non-positive time (`[registry.limits.timeline]`: 64, 256, 8); it stores the event under
`timeline/` named by time, and past `records` (10,000) removes the oldest.
`timeline_events(operation)` returns one Operation's events oldest first, skipping unreadable
files. `submit_cable` records a `cable`/`sent` event for a new Cable under the sender's
Operation (sender Station and Agent, Mission and Session, details `cable`, `scope`,
`recipients`; never the body or the target name), and `authorize_sleeper` /
`revoke_sleeper` record `sleeper`/`authorized` and `revoked` (detail `sleeper`); see Sleepers for the
rest of that kind. Universal
Exports records lifecycle transitions and control outcomes. A failed timeline write never
fails the Cable or authorization it describes. Events never hold prompts, messages, Cable
bodies, terminal bytes or secrets.

`StationSettings::try_new(station, capacity)` accepts a Station ID and a maximum
concurrent Mission count of at least 1; `set_station_settings` refuses one above
`[registry.limits.stations] mission_capacity` (1,024). `StationStore::station_settings(station)`
reads the validated current value; `set_station_settings(station, settings)` persists it
under a collision-resistant Station-specific file and the shared Registry lock. A daemon
uses the live value for new Mission admission and scheduling; absent settings retain its
startup default, and unreadable settings fail closed. Existing provider Sessions are not
stopped when the limit is lowered. Every backend stores all four fields; a record missing one
is invalid.

`put_agent_role(operation, role)`, `list_agent_roles(operation)`, `agent_role(operation,
key)` and `remove_agent_role(operation, role_id)` manage validated, bounded role
overrides in `agent-roles/<operation>/`. These use the shared Registry lock for writes,
isolate every Operation, reject path-like identifiers and cap each Operation at
`[registry.limits.agent_roles] records` (256) roles.
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
Records are bounded by `[registry.limits.stations] record_bytes` (1 MiB) and scans by its
`records` (1,000). `elect` returns `true` when it
acquires or renews the lease and `false` when another Station still holds it. An expired
record moves to `headquarters/lease.lapsed.<station>` before the new lease is written.
`headquarters` returns the current record even when its lease has expired.
`HeadquartersLease::term() -> u64` is the election term: it grows by one whenever a
different Station takes the lease and is unchanged by the holder renewing it. Headquarters
leases are bounded like Station records and use wall-clock Unix seconds, so participating Stations
need synchronized clocks. The daemon publishes `roles`, `labels`, `capacity`, `running`,
the host's `resources` (sysinfo: CPU count, CPU percent from the second sample on, the
one-minute load average except on Windows, total and available memory; in this crate's own
tests every check-in repeats the process's first sample), its `resource_limits` (Registry
settings, else the defaults), each Theater's running `sessions`,
local Operation paths, available `handlers`, reachable `theaters` (with their capabilities) and each Operation's
placement policy (`operation_policies`, omitted with a log line when over 512 KiB), every
roster Operation's ancestors nearest first (`operation_lineage`: `{"G": ["P", "ROOT"],
"ROOT": []}`, omitted with a log line when over 512 KiB), the
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
Moneypenny's desk runs by the roster's `[assistant.desk]` (`AssistantDeskLimits`, read per
job and harness start): a clone of her repositories is given up and removed
past `clone_timeout` or `clone_bytes`, and a save of `personal/` past `save_bytes` saves nothing
and a notice says why;
`with_account_login(impl AccountLogin + 'static)` replaces `CommandLogin` for Codex pool
accounts, `with_account_homes(dir)` moves their per-Mission `CODEX_HOME`s from
`<state>/accounts` (cleanup removes only the directories it made, marked
`.ai6-account-homes`),
`with_headquarters_lease(Duration)` sets how long the Headquarters lease lasts between
renewals (default `DEFAULT_LEASE`, 60 s; zero makes startup fail): how long a standby
control Station waits before taking over, `with_config_poll(Duration)` sets how often it
checks its roster source and `loadouts.toml` (default `DEFAULT_CONFIG_POLL`, 5 s; zero turns
reloading off; loadouts set with `with_loadouts` are never reloaded), and `with_terminal` pins the terminal provider. Without a pin the Station uses an available
terminal provider that suits the Agent's handler (`choose_terminal` in the terminal API). A
pin that gives a JSON harness a terminal is refused at Mission start. Empty roles make daemon startup fail. The current Rust scheduler does not yet use election for
distributed placement. Station and Agent limits count assignments in the shared Registry.
The default Station ID is `<host>-<random>`, kept in the state directory; `ai6.cfg`
`station` and `--station` set an explicit identity.
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
    pub clearance: Option<String>,           // what the harness was launched at, which fixed its tools
    pub agent: Option<String>,               // the Agent codename it ran as; a scorecard groups by it
    pub overlays: Vec<IssuedDigest>,         // ai6::directorates::q::overlays: each overlay and its digest
}
impl SessionLaunch { pub fn new(handler, terminal, working_directory) -> Self;
    pub fn with_overlays(self, Vec<IssuedDigest>) -> Self;
    pub fn with_agent(self, Option<String>) -> Self;
    pub fn with_model(self, Option<String>, Option<String>) -> Self;
    pub fn with_mode(self, ExecutionMode) -> Self; pub fn with_runner(self, key) -> Self;
    pub fn with_clearance(self, Option<String>) -> Self; }
#[non_exhaustive] pub struct SessionRecord {
    pub session_id: String,                  // AI6's ID, the run's UID (not the provider's)
    pub mission_id: String,                  // the Mission's UID
    pub handle: Option<String>,              // `<mission handle>.<attempt>` when it opened; a rename leaves it
    pub operation: Option<String>,           // the Mission's Operation, as the opening Station knew it
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
    pub opened_at: Option<f64>,              // Unix seconds
    pub closed_at: Option<f64>,              // set when it becomes Ended or Lost
    pub result: Option<SessionResult>,       // how its Mission stood at the holder's last write
    pub window: Option<String>,              // the standing Sleeper's window the run answers
    pub verdict: Option<MissionVerdict>,     // what a person found; needs no lease, the latest stands
    pub deliverables: Vec<Deliverable>,      // what the Mission was to deliver; set when the attempt goes verifying
    pub evidence: Vec<Evidence>,             // what probes found of them, oldest first, bounded (evidence_entries)
    pub probe_requested: Option<f64>,        // when a person asked for a probe, until the Station ran it
}
#[non_exhaustive] pub enum ProbeResult { Found, Missing, Unprobed, Failed }   // serde kebab-case; as_str()
    // Missing: the place was read and it is not there. Failed: the look failed or timed out, nothing is known.
#[non_exhaustive] pub struct Evidence { pub deliverable: Deliverable, pub result: ProbeResult,
    pub detail: String /* one line, data: may quote a tracker or server */, pub at: f64 }
    // new(deliverable, result, detail, at); detail within evidence_detail_chars
#[non_exhaustive] pub struct SessionResult { pub state: String /* the Mission state: working, blocked,
    stalled, done, failed, blown, abandoned, parked, quarantined, held, verifying */, pub reason: Option<String>
    /* a blocked Mission's reason (green-ink, issue, budget, interrupted, clearance-lowered), or
       SessionResult::HANDED_OFF */, pub denied_tools: u32 }
impl SessionResult { pub const HANDED_OFF: &'static str; pub fn new(state) -> Self;
    pub fn with_reason(self, Option<String>) -> Self; pub fn with_denied_tools(self, usize) -> Self }
impl SessionRecord { pub fn leased_at(&self, now: f64) -> bool;   // false once closed
    pub fn takes_verdict(&self) -> Result<(), SessionError> }   // Unsettled unless it ended done or held, or is verifying
#[non_exhaustive] pub enum OutcomeVerdict { Confirmed, Disputed, FailedVerification, Redispatch }   // serde kebab-case;
    // ALL, as_str(), from_name(), is_fault() (every verdict but Confirmed). Redispatch only settles a verifying Mission:
    // its next attempt, told the note and the amendment; the verified attempt counts against its Agent.
#[non_exhaustive] pub struct MissionVerdict { pub verdict: OutcomeVerdict, pub note: Option<String>,
    pub amendment: Option<String> /* a redispatch's, within amendment_bytes (8 KiB) */,
    pub by: String, pub at: f64 }   // new(verdict, by, at, note) .with_amendment(Option<String>); note within verdict_note_bytes (4 KiB)
#[non_exhaustive] pub struct FinalReport { pub session_id: String, pub mission_id: String, pub text: String,
    pub at: f64 }   // new(session_id, mission_id, text, at, limit): a longer text is cut, the cut marked
pub struct NewSession   // NewSession::new(session_id, mission_id, station); next_attempt() -> Option<Self>
                        //   (a new UID, the next attempt) .with_handle(mission_handle) .attempt()
                        //   .with_provider_session(id) .with_theater(id) .with_attempt(n, predecessor)
                        //   .with_operation(id)
                        //   .with_exposure(Vec<Exposure>) .with_launch(SessionLaunch) .with_account(AccountRef)
                        //   .with_window(label)
pub struct SessionUpdate  // default() .provider_session(id) .status(SessionStatus) .parked_until(f64)
                          //   .exposed(Vec<Exposure>) (adds sources not yet recorded)
                          //   .launch(SessionLaunch) .recent(Vec<String>) .checkpoint(SafeHouseCheckpoint, at)
                          //   .account(AccountRef) .result(SessionResult) (closing the status sets closed_at)
                          //   .deliverables(Vec<Deliverable>) (replaces) .evidence(Vec<Evidence>) (appends, bounded)
                          //   .probed() (answers a probe request)
#[non_exhaustive] pub enum SessionError { NotFound, Exists, Ended, Unsettled(String), HeldBy(String), Stale,
    Io(io::Error) }

pub trait SessionStore {
    fn open_session(&self, new: NewSession, ttl: Duration) -> Result<SessionRecord, SessionError>  // Exists: the ID, or the Mission's attempt, has a record
    fn session(&self, id: &str) -> io::Result<Option<SessionRecord>>
    fn sessions(&self) -> io::Result<Vec<SessionRecord>>          // stable ID order
    fn mission_exposure(&self, mission_id: &str) -> io::Result<Vec<Exposure>>  // every Session of the Mission, earliest first
    fn take_session(&self, id: &str, station: &str, ttl: Duration) -> Result<SessionRecord, SessionError>
    fn renew_session(&self, id: &str, station: &str, fence: u64, ttl: Duration) -> Result<SessionRecord, SessionError>
    fn update_session(&self, id: &str, station: &str, fence: u64, update: SessionUpdate) -> Result<SessionRecord, SessionError>
    fn lose_session(&self, id: &str, reason: &str) -> Result<SessionRecord, SessionError>
    fn latest_session(&self, mission_id: &str) -> io::Result<Option<SessionRecord>>  // highest attempt
    fn record_verdict(&self, operation, mission_id, &MissionVerdict) -> Result<SessionRecord, SessionError>
        // on the latest attempt; NotFound, or Unsettled when it has not ended done or held or is not verifying
    fn request_probe(&self, operation, mission_id, at: f64) -> Result<SessionRecord, SessionError>
        // asks the Station that holds a verifying latest attempt to probe its deliverables; no lease needed;
        // NotFound, or Unsettled when the attempt is not verifying
    fn file_final_report(&self, &FinalReport) -> io::Result<()>   // replaces the Session's; SQL: ai6_final_reports
    fn final_report(&self, session_id) -> io::Result<Option<FinalReport>>
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
/// first_reattach_delay doubled per failed try after the first, up to max_reattach_delay:
/// 5, 10, 20, 40, 60, 60, ... s by default
pub fn reattach_delay(failures: u32, limits: &RecoveryLimits) -> Duration
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

## Scorecard (`ai6::directorates::m::scorecard`)

How each Agent, route and model has done in an Operation over a window, computed on demand from
the Session records and the usage ledger; nothing is stored. `judge(&SessionRecord)` gives each
attempt one `Judgment` by whose doing the outcome was; only `done`, `failed`, `blown`, `stalled` and
`disputed` (a person's verdict, whatever the state) are judged, and a limit, a lost Station, a
hand-off, a person's stop or wait and a spending limit are listed but never held against the Agent. An attempt with no Session record (a failed checkout, a
brief with no Agent) is not an attempt.

```rust
#[non_exhaustive] pub enum Judgment { Done, Failed, Blown, Stalled, Disputed, WaitingOnPerson, BudgetStop,
    Parked, Lost, HandedOff, Withdrawn, Running, Unrecorded }   // serde snake_case
impl Judgment { pub const ALL: [Self; 13]; pub fn as_str(self) -> &'static str;
    pub fn is_judged(self) -> bool; pub fn is_fault(self) -> bool /* failed, blown, stalled, disputed */ }
pub fn judge(record: &SessionRecord) -> Judgment;
pub const DEFAULT_WINDOW_DAYS: u64 /* 30 */; pub const DEFAULT_SMALL_SAMPLE: u64 /* 5 */;
pub struct ScorecardDefinition;   // [operation.ID.scorecard]: window_days (0 = every attempt), small_sample
impl ScorecardDefinition { pub fn resolve(&self) -> ScorecardPolicy }
#[non_exhaustive] pub struct ScorecardPolicy { pub window_days: u64, pub small_sample: u64 }  // Operation.scorecard,
                                  // and OperationPolicy.scorecard in the check-in
#[non_exhaustive] pub struct AttemptRef { pub mission_id: String, pub session_id: String, pub attempt: u32 }
#[non_exhaustive] pub struct Standing {
    pub attempts: BTreeMap<Judgment, Vec<AttemptRef>>,   // the numbers are their lengths
    pub judged: u64, pub failure_rate: Option<f64>,      // faults / judged
    pub small_sample: bool,                              // judged < the policy's small_sample
    pub refused_tools: u64, pub attempts_with_refusals: u64,
    pub usage: UsageTotals, pub wall_secs: f64 }         // closed attempts; parked time included
impl Standing { pub fn count(&self, Judgment) -> u64 }
#[non_exhaustive] pub struct AgentCard { pub agent: String, pub standing: Standing, pub routes: Vec<RouteCard> }
#[non_exhaustive] pub struct RouteCard { pub route: RouteRef, pub standing: Standing }
#[non_exhaustive] pub struct ModelCard { pub model: Option<String>, pub standing: Standing }
#[non_exhaustive] pub struct Scorecard { pub operation: String, pub window_days: u64, pub from: Option<f64>,
    pub until: f64, pub small_sample: u64, pub agents: Vec<AgentCard>, pub models: Vec<ModelCard>,
    pub unattributed: u64 /* attempts whose launch names no Agent: counted toward their model only */ }
impl Scorecard { pub fn build(operation: &str, records: &[SessionRecord],
    usage: &BTreeMap<String /* AI6 Session ID */, UsageTotals>, policy: &ScorecardPolicy, now: f64) -> Self }
```

The Station writes `SessionLaunch.agent` and a `SessionResult` at every sync of the Mission's state
to its Session record; Universal Exports serves the scorecard (`GET .../agents/scorecard`).

### Stewardship and holds (`ai6::directorates::m::stewardship`, `ai6::directorates::registry::holds`)

What M files from the scorecard. `propose(card, agents, held, policy)` is pure: it returns the
`Proposal`s a round files, each with the numbers and Missions in its `reason`. Every rule is off
until `StewardshipPolicy` sets its threshold; a row with `small_sample` is never judged; an Agent
with no roles, a held Agent or route, and a model in `never_prefer` are never proposed.

```rust
pub const HOLD_KIND: &str = "hold"; pub const PREFER_KIND: &str = "prefer";   // green ink kinds
#[non_exhaustive] pub struct StewardshipPolicy { pub propose: bool /* true */,
    pub hold_above: Option<f64>, pub reorder_gap: Option<f64>, pub auto_hold: bool /* false */,
    pub never_prefer: Vec<String> }       // Operation.stewardship; [operation.ID.scorecard] keys
#[non_exhaustive] pub enum Step { Hold { agent: String, route: Option<RouteRef> },
                                  Prefer { agent: String, over: String } }   // exported as ProposedStep
#[non_exhaustive] pub struct Proposal { pub step: Step, pub target: String /* AGENT@OP,
    AGENT:HANDLER/MODEL/POOL@OP, AGENT>OVER@OP */, pub reason: String, pub judged: u64 }
pub fn propose(card: &Scorecard, agents: &[AgentFacts], held: &[RosterHold],
               policy: &StewardshipPolicy) -> Vec<Proposal>;
/// The roles a hold would leave with no Agent able to take them; empty allows the hold.
pub fn roles_left_empty(agents: &[AgentFacts], holding: &str, route: Option<&RouteRef>,
    unavailable: &[String], held_routes: &[RosterHold]) -> Vec<String>;

#[non_exhaustive] pub struct RosterHold { pub operation: String, pub agent: String,
    pub route: Option<RouteRef> /* None: the whole Agent */, pub reason: String,
    pub by: String /* a person, or AUTOMATIC_HOLD ("m") */, pub at: f64,
    pub approval: Option<String> /* the green ink request that signed it */ }
impl RosterHold { pub fn new(operation, agent, route, reason, by, at) -> Self;
    pub fn signed_through(self, approval) -> Self; pub fn holds_agent(&self) -> bool;
    pub fn covers(&self, route: &RouteRef) -> bool; pub fn validate(&self, &HoldLimits) -> io::Result<()> }
pub trait HoldStore {   // part of RegistryBackend; `holds/OPERATION/AGENT--ROUTE.json`, table ai6_roster_holds
    fn put_hold(&self, hold: &RosterHold) -> io::Result<()>;          // replaces the same Agent/route
    fn holds(&self, operation: &str) -> io::Result<Vec<RosterHold>>;
    fn lift_hold(&self, operation: &str, agent: &str, route: Option<&RouteRef>)
        -> io::Result<Option<RosterHold>>;
}
pub struct HoldLimits { pub record_bytes, pub holds, pub text_bytes }   // [registry.limits.holds]
```

### Memories

Rows of text with a scope, the same three ways on every Registry. `ai6::directorates::m::memories`
decides who may see and change one; every read goes through it. Moneypenny's tools and the routes
are in `universal-exports`; a Session's tools (`memory_search`, `memory_read`, `memory_write`,
`memory_forget`) are on its Cable endpoint, served by `station::server::memories`. Who holds what:
`docs/memories.md`.

```rust
// ai6::directorates::registry::memories; re-exported at the crate root.
pub enum MemoryScope { Person(String), Operation(String), Global } // "person:<id>", "operation:<id>", "global"
impl MemoryScope { pub fn parse(text: &str) -> Option<Self>; }     // Display and serde as that text
pub struct MemorySource { pub by: String /* person:ID, agent:CODENAME or moneypenny:PERSON */,
    pub session: Option<String>, pub exposures: Vec<String> /* empty: not exposed */ }
impl MemorySource { pub fn new(by, session) -> Self; pub fn exposed_by(self, exposures) -> Self;
    pub fn exposed(&self) -> bool; }
pub struct MemoryHeader { pub id: String /* a generated UUID, lower case */, pub scope: MemoryScope,
    pub name: Option<String>, pub summary: String, pub created_at: f64, pub updated_at: f64,
    pub updated_by: String, pub source: MemorySource, pub compartments: Vec<String>,
    pub revision: u64 /* counts its writes */, pub bytes: u64 /* name + summary + content */ }
pub struct MemoryEntry { pub header: MemoryHeader, pub content: String }
impl MemoryEntry {
    pub fn new(scope, name, summary, content, source, at) -> Self;  // a fresh ID, revision 1
    pub fn with_id(id, scope, name, summary, content, source, at) -> Self;
    pub fn behind(self, compartments) -> Self;                      // sorted, no repeats
    pub fn validate(&self, &MemoryLimits) -> Result<(), String>;    // a message for the writer
}
pub enum MemoryError { Invalid(String), Full(String) /* says nothing of what the scope holds */,
    Missing, Changed { revision: u64 }, Io(io::Error) }
pub trait MemoryStore {  // part of RegistryBackend; `memories/<scope digest>/<id>.json`, table ai6_memories
    fn write_memory(&self, entry: MemoryEntry, base: Option<u64>) -> Result<MemoryEntry, MemoryError>;
        // base None: new, an ID taken is Changed. base Some: replaces it at that revision in its
        // own scope (another scope is Missing), keeping the old as a prior version.
    fn memory(&self, id: &str) -> io::Result<Option<MemoryEntry>>;  // not an ID: None
    fn memory_headers(&self, scope: &MemoryScope) -> io::Result<Vec<MemoryHeader>>; // newest change first
    fn memory_candidates(&self, scopes: &[MemoryScope], terms: &[String]) -> io::Result<Vec<MemoryEntry>>;
        // at least what `matching` takes, whoever may see it: the caller filters, then ranks
    fn memory_versions(&self, id: &str) -> io::Result<Vec<MemoryEntry>>; // newest first
    fn forget_memory(&self, id: &str, base: Option<u64>) -> Result<MemoryEntry, MemoryError>;
        // and its prior versions
    fn memory_limits(&self) -> &MemoryLimits;
    fn propose_memory(&self, &MemoryProposal) -> Result<(), MemoryError>; // Full past `proposals` a scope
    fn memory_proposals(&self, scope: &MemoryScope) -> io::Result<Vec<MemoryProposal>>; // oldest first
    fn memory_proposal(&self, id: &str) -> io::Result<Option<MemoryProposal>>;
    fn drop_memory_proposal(&self, id: &str) -> io::Result<bool>;
}
pub struct MemoryProposal { pub id: String, pub scope: MemoryScope,
    pub target: Option<(String, u64)> /* the entry and the revision it was made from */,
    pub forget: bool, pub name: Option<String>, pub summary: String, pub content: String,
    pub compartments: Option<Vec<String>> /* None leaves them */, pub source: MemorySource /* who
    asked, with their mark */, pub at: f64 }
impl MemoryProposal { pub fn new(scope, target, name, summary, content, compartments, source, at)
    -> Self; pub fn validate(&self, &MemoryLimits) -> Result<(), String>; }
pub fn search_terms(query: &str) -> Vec<String>;   // distinct runs of letters and digits, lower case
pub fn matching(entries: Vec<MemoryEntry>, terms: &[String]) -> Vec<MemoryEntry>;
    // holds at least one term, ranked: how many it holds, how many in its name or summary, newest
    // change, ID. No terms: all of them, newest first.
pub fn is_memory_id(id: &str) -> bool;
pub struct MemoryLimits { pub content_bytes, entries, scope_bytes, versions, summary_bytes, name_bytes,
    compartments, index_bytes, search_results, query_bytes, proposals: Limit } // [registry.limits.memories]

// ai6::directorates::m::memories
pub const AUDIT_WRITE, AUDIT_FORGET, AUDIT_READ, AUDIT_COMPARTMENTS, AUDIT_MARK_CLEARED,
    AUDIT_PROPOSED, AUDIT_APPROVED, AUDIT_REJECTED, PERSON_RESOURCE, GLOBAL_RESOURCE: &str;
pub enum MemoryWriteRight { Write, Propose, Refused }
pub trait MemoryCaller {          // who is asking, read afresh for the call; what cannot be read is not held
    fn by(&self) -> String; fn session(&self) -> Option<String>;
    fn reads_scope(&self, &MemoryScope) -> bool;
    fn writes_scope(&self, &MemoryScope) -> MemoryWriteRight;
    fn read_in(&self, compartment: &str) -> bool; // unknown or unreadable: false
    fn sees_all_compartments(&self) -> bool;       // a chief
    fn acts_in_person(&self) -> bool;              // clears a mark, decides a proposal
    fn exposures(&self) -> Vec<String>;            // a write carries them
    fn saw_marked(&self, source: &str);            // shown what an exposed Session wrote: memory:<id>
    fn mark_bytes(&self) -> Limit;
    fn now(&self) -> f64;
    fn audit(&self, action: &str, scope: &MemoryScope, target: &str) -> Result<(), String>;
}
// visible(): the scope's read, and no compartments, a chief, or read into one of them.
pub enum MemoryRefusal { Missing /* one answer for not there and not visible */, Forbidden(String),
    Invalid(String), Full(String), Changed { revision: u64 }, Failed(String) } // Display: for the caller
pub fn read_entry(store, caller, id) -> Result<MemoryEntry, MemoryRefusal>;
pub fn prior_versions(store, caller, id) -> Result<Vec<MemoryEntry>, MemoryRefusal>;
pub fn list_entries(store, caller, scope) -> Result<Vec<MemoryHeader>, MemoryRefusal>;
pub fn search_entries(store, caller, scopes, query, limits) -> Result<MemorySearch, MemoryRefusal>;
    // filters, ranks, counts, then cuts to search_results; query past query_bytes is Invalid
pub struct MemorySearch { pub hits: Vec<MemoryHeader>, pub total: usize }
pub enum MemoryTarget { New(MemoryScope), Existing { id: String, revision: u64 } }
pub struct MemoryChange { pub target, pub name, pub summary, pub content,
    pub compartments: Option<Vec<String>> } // ::new(target, name, summary, content).with_compartments(..)
pub struct MemoryWritten { pub entry: MemoryEntry, pub before: Option<String> }
pub enum MemoryOutcome { Kept(MemoryWritten), Proposed(MemoryProposal) }
pub enum MemoryForgotten { Forgotten(MemoryEntry), Proposed(MemoryProposal) }
pub fn write_entry(store, caller, change) -> Result<MemoryOutcome, MemoryRefusal>;
    // validated, then audited, then kept or proposed. Adding or removing a compartment needs a read-in
    // (or a chief), and none may leave the entry hidden from its writer. A person's own edit clears
    // a mark (memory.mark_cleared); anyone else's keeps it and adds their exposures.
pub fn forget_entry(store, caller, id, revision) -> Result<MemoryForgotten, MemoryRefusal>;
pub fn list_proposals(store, caller, scope) -> Result<Vec<MemoryProposal>, MemoryRefusal>;
    // only for a person who writes the scope, only what they may see
pub fn decide_proposal(store, caller, id, approve: bool) -> Result<Option<MemoryEntry>, MemoryRefusal>;
    // approved: written with the proposer's source and mark; rejected: dropped; both audited
pub fn header_json(&MemoryHeader) -> Value; pub fn entry_json(&MemoryEntry) -> Value;
pub fn proposal_json(&MemoryProposal) -> Value;
pub fn fenced(tool: &str, &Value) -> String;       // <ai6-data tool="..">, `<` escaped inside
pub fn bounded(text: &str, Limit) -> String;       // one line within the bound, for an audit record
pub fn summaries_for_a_model(headers, bytes: Limit, fence: impl Fn(&Value) -> String) -> Option<String>;
```


The Station's round (`station::server::stewardship`, Headquarters, with promotion) files each
proposal as a green ink request under Session `m` and Mission `hold-AGENT`/`prefer-AGENT`,
idempotency key `TARGET#(judged / small_sample)`. A `hold` a person signed (not an expiry, not a
class authorisation) is spent and becomes a hold in the signer's name; a `prefer` only closes. With
`auto_hold` it holds by itself and audits `agent.hold.applied`. Selection (`agents::select`,
`reassign_if_unavailable`) passes held Agents over; `routes::standing_of` refuses a held route. Nothing
touches a running Session.

## Reports (`ai6::directorates::registry::reports`, `ai6::directorates::m::reports`)

One record of something observed, in a scope, never deleted before `keep_closed`. Filing creates no
Mission, brief or placement. Assessment and closing are not implemented: a Report is filed `open`.

```rust
// ai6::directorates::registry::reports
pub enum ReportScope { Operation(String), Station(String), M } // "operation:ID", "station:ID", "m"; Display, FromStr, serde
impl ReportScope { pub fn operation(&self) -> Option<&str>; }
#[non_exhaustive] pub enum ReportState { Open, Assessed, Closed }     // snake_case; as_str, FromStr
#[non_exhaustive] pub enum ReportFiler { Source { source, item }, Agent { agent, session /* provider
    Session, or the Mission's ID until known */, mission }, Person { person }, Probe { probe },
    Review { mission }, Watchdog }       // serde tag "type"; is_person(), session(), source_key()
#[non_exhaustive] pub enum ReportLinkKind { Mission, Report, Decision, Evidence } // as_str, FromStr
pub struct ReportLink { pub kind: ReportLinkKind, pub id: String } // new(kind, id), mission(id)
#[non_exhaustive] pub struct Tearline { pub text, pub classification: Option<String>, pub compartments: Vec<String> }
    // new(text), classified, in_compartments; never above its Report, compartments within the Report's
#[non_exhaustive] pub struct ReportAssessment { pub verdict, pub by, pub at: f64, pub comment: Option<String> }
#[non_exhaustive] pub struct ReportResolution { pub by, pub at: f64, pub note: Option<String> }
#[non_exhaustive] pub struct Report { pub id, pub scope: ReportScope, pub kind, pub severity: Severity,
    pub title, pub text, pub filer: ReportFiler, pub links: Vec<ReportLink>, pub sub_source: Option<String>,
    pub from: Option<f64>, pub credibility: Option<u8> /* 1..=6, unset until assessed */,
    pub classification: Option<String>, pub handling: Option<String>, pub compartments: Vec<String>,
    pub tearline: Option<Tearline>, pub priority: Option<String>, pub deadline: Option<f64>,
    pub state: ReportState, pub assessment, pub resolution, pub created: f64, pub updated: f64 }
impl Report { pub fn new(id, scope, kind, severity, title, text, filer, at) -> Self; // open
    /* linked, via, information_from, believed, classified, handled, in_compartments, with_tearline,
       prioritised, due */ pub fn untrusted(&self) -> bool /* all but a person's */;
    pub fn about_mission(&self, id) -> bool;
    pub fn validate(&self, &ReportLimits) -> io::Result<()>; } // InvalidInput: bounds, a tearline
    // compartment not the Report's, credibility 1 with no Report or Evidence link. The lists are
    // ReportSettings::check, not the Registry's
#[non_exhaustive] #[derive(Default)] pub struct ReportFilter { pub scope, pub state, pub kind, pub mission, pub limit,
    pub priorities: Vec<String> /* the default list */ } // all(), in_scope, about_mission, in_state, of_kind,
    // at_most, ordered_by(priorities): pass the deployment's ReportSettings::priorities
#[non_exhaustive] pub enum Reliability { A, B, C, D, E, F }   // Ord: A first; as_str, FromStr
#[non_exhaustive] pub struct ReportSource { pub key, pub reliability: Reliability, pub rated: f64 } // new(key): F
#[non_exhaustive] pub struct ReportClearance { pub subject /* person:ID, group:NAME, agent:CODENAME */,
    pub classification, pub by, pub at: f64 }  // new(subject, classification, by, at), validate(&ReportLimits)
pub type ReportChange<'a> = &'a mut dyn FnMut(&mut Report) -> io::Result<()>;
pub trait ReportStore {  // part of RegistryBackend; reports/SCOPE/ID.json, report-sources/; ai6_reports,
                         // ai6_report_links, ai6_report_sources
    fn file_report(&self, &Report) -> io::Result<()>;
        // AlreadyExists; QuotaExceeded past per_session / per_operation (Agent filers, per scope,
        // over window); InvalidData past `reports`; drops Reports closed longer than keep_closed
    fn report(&self, id) -> io::Result<Option<Report>>;
    fn reports(&self, &ReportFilter) -> io::Result<Vec<Report>>;
        // priority (the settings' order, unset last), deadline, credibility, the source's
        // reliability (no record: F), then oldest first; cut to `page` after ordering
    fn change_report(&self, id, ReportChange<'_>) -> io::Result<Report>; // atomic; keeps ID, scope,
        // filer and `created` (InvalidInput otherwise); NotFound
    fn report_source(&self, key) -> io::Result<ReportSource>; // F when nobody rated it
    fn put_report_source(&self, &ReportSource) -> io::Result<()>;
    fn report_clearances(&self) -> io::Result<Vec<ReportClearance>>; // by subject; report-clearances/,
                                                                    // ai6_report_clearances
    fn put_report_clearance(&self, &ReportClearance) -> io::Result<()>; // replaces the subject's
    fn revoke_report_clearance(&self, subject) -> io::Result<bool>;     // false: none held
}
pub struct ReportLimits { pub record_bytes, reports, text_bytes, name_bytes /* also the title */, links,
    page, per_session /* 20 */, per_operation /* 200 */: Limit, pub window: Seconds /* 3600 */,
    pub keep_closed: TimeLimit /* none */ }   // [registry.limits.reports]: sizes, counts and rates only

// ai6::directorates::m::reports
#[non_exhaustive] pub struct ReportSettings { pub classifications /* unclassified, restricted,
    confidential: lowest first */, pub handlings /* operation, compartments, peers */,
    pub priorities /* critical, high, normal, low */: Vec<String> } // ToValue, FromValue, Schema, Form;
    // the `roster` record's [roster.reports] (RosterSettings::reports, with_reports)
impl ReportSettings { pub fn rank(&self, Option<&str>) -> Option<usize> /* unset is 0 */;
    pub fn check(&self, &Report) -> Result<(), String>; } // lists, and a tearline no higher than its Report
pub fn settings(&dyn ConfigProvider) -> Result<ReportSettings, RosterError>; // now; defaults with no record
pub struct ReportRules<'a> { pub settings: &'a ReportSettings, pub clearances: &'a [ReportClearance] }
impl ReportRules<'_> { pub fn new(settings, clearances) -> Self; pub fn clearance(&self, subjects: &[String]) -> usize; }
    // the highest rank a clearance names any subject at, else 0
pub trait ReportReader {   // answered from what is granted now
    fn reads_scope(&self, &ReportScope) -> bool;  // mission.read on an Operation (a compartment's
                                                  // crossing audited first); station.read
    fn read_in(&self, compartment) -> bool;       // CompiledCompartments::person_read_in / field_read_in
    fn subjects(&self) -> Vec<String>;            // what ReportClearance::subject is matched against
}
#[non_exhaustive] pub enum ReportSight { Content, Tearline, Markings }
pub fn sight(&dyn ReportReader, &Report, &ReportRules<'_>) -> ReportSight;
    // Content: the scope's read, read in to EACH compartment, cleared for the classification.
    // Tearline: the scope's read and cleared for the tearline's marking. Else Markings, also for
    // anything that only links to it. Nobody is exempt, a chief included.
pub fn report_json(&Report, ReportSight, Option<&ReportSource>) -> Value;
    // always id, scope, classification, handling, compartments, sight; never title or text below Content
pub struct ReportMarkings { pub classification, pub handling, pub compartments, pub tearline: Option<TearlineFiling> }
pub fn remark(&dyn ReportReader, &mut Report, ReportMarkings, &ReportRules<'_>) -> Result<(), String>;
    // the reader sees Content before and after, so lowering needs someone read in
pub struct ReportFiling { pub title, pub text, pub kind, pub severity, pub priority, pub deadline,
    pub information_from, pub sub_source, pub classification, pub handling, pub compartments,
    pub tearline: Option<TearlineFiling>, pub links: Vec<LinkFiling> } // serde, unknown keys refused
impl ReportFiling { pub fn into_report(self, scope, filer, links, at, &ReportLimits, &ReportSettings)
    -> Result<Report, String>; }
pub fn new_report_id() -> String;   // "report-" + 32 hex
```

Every Session (feature `cables-mcp`) has `file_report` (against its Mission's Operation, linked to
its Mission; a Head of Station round's goes to the logbook, check-in and channel and is also a
`station` Report of its Station), `reports` and `read_report`, answering inside `<ai6-data>`; another
Operation's content marks the Session `product:<operation>`. Universal Exports serves `POST`, `GET
/v1/reports`, `GET /v1/reports/{id}` (`403` with the markings below Content), `PUT
/v1/reports/{id}/markings` and `GET /v1/missions/{id}/reports`; filing needs `mission.create`, or
`station.configure` on `station:ID` or `config:reports`. `GET /v1/reports/clearances` (`access.read`)
and `PUT`, `DELETE /v1/reports/clearances/{subject}` (`access.manage`, both on `config:reports`)
list, grant and revoke clearances, each audited (`report.clearance.grant`, `.revoke`) before it is
made. The Station and the gateway read the settings and the clearances at every call.

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
    pub fn validate(&self, limits: &ClearanceLimits) -> io::Result<()>;   // Registry-key agent and operation, known clearance
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
                   pub fn count(&mut self, report: &AgentOutcomeReport, limits: &AgentLimits) -> io::Result<bool> /* false: key seen */ }
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
from the current one; incidents do not. A record past `[registry.limits.agents]`
`operations` (128), `clearances` in one (8) or `record_bytes` (1 MiB) is refused
(`InvalidData`). Only the Station that holds a Mission's Session counts it:
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
pub struct AccountRef { pub pool: String, pub account: String }   // Display "pool/account"; serde
impl AccountRef { pub fn new(pool, account) -> Self }
#[non_exhaustive] pub struct AccountClaim { pub station: String, pub mission: String, pub since: f64, pub expires: f64,
    pub operation: Option<String> }   // serde default None
#[non_exhaustive] pub struct QuotaWindow { pub name: String, pub used_permille: Option<u16>,
    pub resets_at: Option<f64>, pub status: Option<String>, pub observed_at: f64 }
impl QuotaWindow { pub fn new(name, observed_at: f64) -> Self; .used_permille(u16) .resets_at(f64) .status(s);
    pub fn use_at(&self, now: f64, unknown: u16) -> u16 }   // 0 once reset; `unknown` when no figure
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
    pub fn is_limited(&self, now) -> bool; pub fn highest_use(&self, now, unknown: u16) -> u16; pub fn tokens_on_day(&self, now: i64) -> u64 }
pub struct AccountCandidate { pub account: String, pub priority: i64, pub max_concurrent: Option<usize> }
pub struct AccountClaimRequest;   // ::new(&AccountPool, station, mission, ttl) .prefer(account) .excluding(account) .only(account)
                                  //   .operation(Option<String>) .in_use() (the first candidate, room or not)
                                  //   .unknown_use(permille) (default 500, [accounts] unknown_use_permille)
impl AccountClaimRequest { pub fn pool(&self) -> &str; pub fn candidates(&self) -> &[AccountCandidate] }
pub fn choose_account<'a>(strategy: PoolStrategy, candidates: &'a [AccountCandidate],
    states: &BTreeMap<String, AccountState>, prefer: Option<&str>, now: f64, unknown_use: u16)
    -> Option<&'a AccountCandidate>;

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

The Station claims with a TTL of `[accounts] claim_ttl` (60 s), renews at each check-in (its held Missions
and those whose account is being handed over), releases with the Session's leases, records
quota from `AgentEvent::quota` and adds each usage delta's tokens. A limit is recorded until
the reset, at most the Station's horizon (`[accounts] limit_horizon`, 8 days).
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
#[non_exhaustive] pub struct ActionScope { pub kind: String /* a-z0-9-_ */, pub target: String /* no controls */ }
impl ActionScope {
    pub fn try_new(kind, target) -> io::Result<Self>;   // InvalidInput: empty, or other characters
    pub fn check_size(&self, limits: &ApprovalLimits) -> io::Result<()>;   // [approvals] kind_bytes, target_bytes
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
pub fn classify(command: &str, repo: &dyn Repository, nesting: usize)
    -> Result<Vec<ActionScope>, String>;   // the governed steps a shell command takes; nested shells
                                           // and git aliases followed `nesting` deep
```

`classify` reads a command line as POSIX shells, PowerShell and cmd split it (quotes, `;` `&&`
`||` `|`, newlines, parentheses). A redirection and its target (`2>&1`, `> log`, `&>log`, `<<<x`,
`2>$null`) are no argument, and the words after them still are; a single digit 0-6 joined to the
operator is its stream, and `*>` leaves `*` an argument, since a POSIX shell expands it. Commands in
`$(...)`, backticks, `<(...)` and `>(...)`, quoted or not, and in the lines of a here-document, are
read as well; a comment's and a here-document line's quotes end with the line. For each simple
command it drops shell keywords (`if then elif else
while until do { !`), assignments (`GIT_DIR`, `GIT_WORK_TREE`, `GH_REPO` are used) and
wrappers with their own options (`sudo`/`doas` incl. `-D`, `env` incl. `-C` and `-S LINE`,
`timeout` and its duration, `nice`, `ionice`, `stdbuf`, `xargs`, `command`, `exec`,
`nohup`, `setsid`, `time`); `command -v` runs nothing. It follows `cd`/`pushd`/`export` for
later commands and reads `sh -c`, `bash -lc`, `pwsh -Command` (any shortening; a bare argument of
`powershell`), `cmd /c` (`/c"..."` too), `eval`, `iex` and `Invoke-Expression`. `git.cmd`,
`C:\Git\cmd\git.exe` and `.\git.exe` are `git`. It names,
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
`git … push` or `gh … merge|release|api` (`find -exec`, `parallel`, `Start-Process`). So is a
word only the shell can fill in: in a push (`$X`, `$(x)`, a backtick, `%X%`, `{a,b}`, a backslash,
`^` in a destination), a `git` subcommand with anything but letters, digits, `-`, `_` and `.`, a
`gh` release tag, repository or changing `gh api` path, a program named by one word of that kind
before `push`, `merge`, `release` or `api`, and a shell whose whole script is one (`eval "$X"`).
So are a quote or substitution that never closes around `git` or `gh`, `pwsh -EncodedCommand`, and
a shell nested deeper than `nesting`. Not seen: a program named by a variable with no such word
after it, a script a command runs, `gh` aliases, other programs that push.

`strict` asks for every governed kind at every clearance and refuses requests from `desk`;
`licensed` also lets `00` push and merge unattended. A push needs green ink only to a
protected ref. Keys: `preset`, `kinds` (added to the built-ins), `requesters`
(clearances that may ask), `unattended = { CLEARANCE = [KIND, ..] }` (replaces the
preset's list per clearance named), `protected` (ref globs, replacing the default) and
`request_ttl_secs`. An invalid value fails the roster load naming the key; a Station logs
each weakening at start.

Records, under the Registry lock: `approvals/pending/OP/ID.json`,
`approvals/decided/OP/ID.json`, `approvals/missions/OP/MISSION.json` (a Mission's request
IDs, at most `[registry.limits.approvals] mission_requests`, 64), `class-authorizations/OP/ID.json`.

```rust
pub const EXPIRY_SIGNER: &str;   // "expiry"
#[non_exhaustive] pub struct ApprovalRequest { pub id: String /* green-ink-<sha256> */, pub operation: String,
    pub mission: MissionId, pub session: SessionId, pub agent: String, pub scope: ActionScope,
    pub reason: String /* 1 to reason_bytes (2048), newlines allowed; never audited */, pub requested_at: f64, pub expires_at: f64,
    pub proposed_role: Option<String> /* serde default: the role a Mission proposal asks for */,
    pub rearmed: Option<Rearming> /* who last re-opened it after it expired */,
    pub links: Vec<ApprovalLink> /* serde default: what it is about, at most `links` (8) */ }
impl ApprovalRequest { pub fn with_links(self, Vec<ApprovalLink>) -> Self; }
#[non_exhaustive] pub struct ApprovalLink { pub label: String, pub url: String }   // new(label, url);
    // https with a host and no credentials, whitespace, control characters or backslashes
#[non_exhaustive] pub struct Rearming { pub by: String, pub at: f64, pub note: Option<String>,
    pub told_at: Option<f64> /* when the Mission's Station took it up */ }   // new(by, at, note)
impl ApprovalRequest {
    pub fn try_new(operation, mission, session, agent, scope, reason, idempotency_key: &str,
        limits: &ApprovalLimits) -> io::Result<Self>;   // key and reason within limits
    pub fn expiring(self, requested_at: f64, ttl: Duration) -> Self;   // required before storing
    pub fn proposing_role(self, Option<String>) -> Self;
}
#[non_exhaustive] pub enum Decision { Approved, Rejected, Expired }   // serde snake_case; as_str()
pub struct Revocation { pub by: String, pub at: f64, pub note: Option<String> }   // new(by, at, note)
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
    pub fn validate(&self, limits: &ApprovalLimits) -> io::Result<()>;   // needs a kind, an Agent or role, a glob, an expiry after the grant
}
pub trait ApprovalStore {
    fn request_approval(&self, request: &ApprovalRequest) -> io::Result<ApprovalStatus>;   // same ID returns the record
    fn approval(&self, operation, id, now: f64) -> io::Result<Option<ApprovalStatus>>;
    fn pending_approvals(&self, operation, now: f64) -> io::Result<Vec<ApprovalRequest>>;   // oldest first
    fn mission_approvals(&self, operation, mission, now: f64) -> io::Result<Vec<ApprovalStatus>>;
    fn live_approval(&self, operation, mission, scope: &ActionScope, now: f64) -> io::Result<Option<Approval>>;   // one that covers scope
    fn decide_approval(&self, operation, id, ruling: &Ruling) -> io::Result<Approval>;
    fn revoke_approval(&self, operation, id, revocation: &Revocation) -> io::Result<Approval>;
    fn expired_approvals(&self, operation, since: f64, now: f64) -> io::Result<Vec<Approval>>;
        // expired undecided at since or later, the newest first, at most expired_listed (200)
    fn rearm_approval(&self, operation, id, rearming: &Rearming, expires_at: f64) -> io::Result<ApprovalRequest>;
        // only an expired, undecided request; AlreadyExists otherwise
    fn mark_rearming_told(&self, operation, id, at: f64) -> io::Result<()>;
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
pub const GATE_TIMEOUT_ENV: &str;   // "AI6_GREEN_INK_TIMEOUT": seconds per exchange; unset, 30
pub const GATE_NESTING_ENV: &str;   // "AI6_GREEN_INK_NESTING": classify depth; unset, 4
/// What a Station puts in a Session's environment from [green_ink] and [approvals].
pub fn settings_environment(green_ink: &GreenInkLimits, approvals: &ApprovalLimits) -> [(String, String); 2];
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
pub const WEB_GATE_ARGS: [&str; 2];   // ["assistant", "web-gate"]: Moneypenny's WebFetch hook
// feature `gate`:
pub fn web_fetch_gate(input: &str, allow: &[String], deny: &[String]) -> Option<String>; // None: goes on
pub type Resolver<'a> = dyn Fn(&str) -> std::io::Result<Vec<IpAddr>> + 'a;
pub fn web_fetch_gate_with(input: &str, allow: &[String], deny: &[String], resolve: &Resolver<'_>)
    -> Option<String>;   // web_fetch_gate with the system resolver
pub fn host_in(host: &str, patterns: &[String]) -> bool;   // exact, or a leading "*." wildcard
```

`web_fetch_gate` takes a WebFetch `PreToolUse` hook input (`tool_input.url`) and denies the
fetch when the URL's host (lowercased, no trailing dot), or an address it resolves to, is in
`deny` (entries as `denied::DenyEntry` reads them; a name is resolved only when `deny` holds an
address, a network or a category, and one that does not resolve is then denied), or, when
`allow` is not empty, the host is not one of it (`*.example.com` covers the names under
`example.com`, not `example.com` itself, as Claude Code's `WebFetch(domain:…)` rules do;
`host_in` is the match; an address is never in it). An empty `allow` and `deny` let every
fetch through, to an address too. A URL without a host, a deny list it cannot read and
unreadable input are denied. `ai6 assistant web-gate [--hosts H,…] [--deny E,…]` is that hook:
it reads standard input, prints the decision and always exits 0. It exists because
Claude Code 2.1.310 fetches its preapproved documentation domains (`docs.python.org`,
`developer.mozilla.org`) under `dontAsk` although no rule names them (measured against a
loopback stand-in for the API).

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
server-sent events, within `AI6_GREEN_INK_TIMEOUT` (30 s unset) per request; `gate` follows
nested shells as deep as the client's `AI6_GREEN_INK_NESTING` says. `pre_tool_use` takes the `PreToolUse` hook input
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
    pub code: Option<String>,  // the item's code as a person writes it ("AI6-64", "63"); in the Mission ID, made safe
    pub code_tag: Option<String>,  // names the tracker when `code` does not ("gh"); replaced by the instance's name when it is not its provider's only one in the Operation
}
pub struct SourcePoll { pub cursor: Option<String> }
pub struct SourceBatch { pub items: Vec<SourceItem>, pub cursor: Option<String> }
#[guatiao::kind(name = "ai6_source")]
pub trait Source: Send + Sync {
    fn poll(&self, request: SourcePoll) -> Result<SourceBatch, ProviderError>;
}
pub const SOURCE_BRIEF_PREFIX: &str = "source:";

pub enum Ingest { Queued, Updated, Unchanged, InFlight, Finished }
pub struct FinishedSourceBrief { pub brief: Brief, pub outcome: serde_json::Value, pub ok: bool }
pub fn source_brief(operation: &str, provider: &str, item: &SourceItem, limits: &SourceLimits) -> Brief;  // no provenance, no Mission ID; text cut to item_text_bytes, name to name_bytes
pub fn source_entry(operation: &str, instance: &str, item: &SourceItem) -> MissionEntry;
pub fn reopened_brief((operation, provider): (&str, &str), item: &SourceItem, comments: &[Reply],
    limits: &SourceLimits, received_by: &str) -> Brief;  // the item now and the comments since, fenced; Source provenance
pub fn settled_cursor(cursor: &str, settling_secs: u64) -> Option<String>;  // the ISO 8601 cursor that much earlier on its own clock; None when it does not read as a time
pub fn reportable(brief: &Brief, provider: &str) -> bool;  // a `source` provenance naming provider and item

SourceStore::source_cursor(operation: &str, provider: &str) -> io::Result<Option<String>>
SourceStore::set_source_cursor(operation: &str, provider: &str, cursor: &str) -> io::Result<()>
SourceStore::ingest_source_item(operation: &str, provider: &str, item: &SourceItem, received_by: &str) -> io::Result<Ingest>
SourceStore::unreported_source_briefs() -> io::Result<Vec<FinishedSourceBrief>>
SourceStore::mark_source_reported(finished: &FinishedSourceBrief, reported: bool) -> io::Result<()>
```

A provider receives its settings as `SourceConfig::settings_toml` and validates them itself;
the Station removes its own `provider`, `intake` and `interval_secs` keys first. `poll` blocks
on the network and is called off the async runtime. Writing to a tracker is the Tracker kind
below. A provider returns an error for a rate limit
or a refused credential; the Station polls again at its next interval.

`source_brief` sets the brief's `source` to `source:<provider>`, generates its title, and places
the item's title, author, labels and text between fenced markers introduced as untrusted data;
the fence is lengthened until the text cannot contain it. The item's title also goes in
`Brief::name`, one line and cut, to show beside the ID; it never reaches `Brief::prompt`
unfenced. `ingest_source_item` gives the item its Mission ID, queues the brief, updates a brief still in
`new/` when its fingerprint (title, text, labels, `updated`) changed, and leaves a claimed
(`InFlight`) or finished brief alone. Cursors are stored under `sources/`.
`unreported_source_briefs` lists finished source briefs without a report mark (at most
`[registry.limits.sources] report_attempts`, five, attempts each); `mark_source_reported`
records an attempt. A finished brief from anywhere else is marked as owing no report when it
finishes.

On the Station, the Headquarters holder runs a source desk: per Operation and instance
(`sources.<name>`; `provider` names the provider, default the key; `intake = false` keeps an
instance out of polling) it polls on `interval_secs` (default 60 s, the `[sources] min_poll`
floor), ingests the batch, then stores the cursor; every 15 s it posts a comment through the
Tracker for each unreported finished source brief and applies the instance's
`status_labels` / `status_transitions` entry for the outcome (`done`, else `failed`). The
`provider` argument of `SourceStore` and the brief's key and `source` hold the instance name.
The finished outcome carries `state`, `mission_id`, `session_ids`, `agent` and `branch`. Report
wording follows `Cover::disclosure()`: `Hidden` (covert), `Assisted` (partial) or `Full`
(overt).

## Trackers (`ai6::directorates::registry::tracker`)

A Tracker is the second Guatiao kind (`ai6_tracker`) the tracker providers (`github`, `gitlab`,
`jira`) serve, built from the same `SourceConfig` as their Source. Every method is blocking and
names an issue by its item ID (`owner/repo#12`, `group/project#12`, `GUA-12`). An item outside the instance's
repository or project fails `GUATIAO_ERR_BAD_VALUE` (`foreign item ...`); a call the
instance cannot make fails `GUATIAO_ERR_UNSUPPORTED` naming what it can do.

```rust
pub struct TrackerCapabilities {
    pub comment: bool, pub edit_comment: bool,
    pub labels: Vec<String>, pub free_labels: bool,   // the tracker's labels; free_labels: any is accepted
    pub transitions: Vec<String>,                     // per item (Jira)
    pub assign: bool, pub close: bool, pub create: bool,
    pub issue_types: Vec<String>, pub priorities: Vec<String>,
    pub children: String,        // "none" | "sub_issues" | "subtasks" | "parent"
    pub links: Vec<String>,      // "issue_links", "remote_links"; empty: only text links
    pub replies: bool,
    pub visibility: String,      // "public" | "private" | "unknown"
    pub markers: bool,
}
pub struct TrackerIssue { pub id, native_id, title, body, state /* open|closed */: String,
    pub labels: Vec<String>, pub assignees: Vec<String>, pub url: Option<String>,
    pub updated: String, pub parent: Option<String>, pub author_id: Option<String>,
    pub priority: Option<String>, pub code: Option<String>, pub code_tag: Option<String> }
    // code and code_tag as SourceItem's: what a Mission created with the issue is numbered by
pub struct ReplyPoll { pub items: Vec<String>, pub since: Option<String> }
pub struct Reply { pub id, item, author_id, author: String, pub permitted: bool,
    pub own: bool, pub body: String, pub created: String }   // body untrusted
pub struct ReplyBatch { pub replies: Vec<Reply>, pub cursor: Option<String> }
pub struct TrackerPost { pub item: String, pub body: String, pub edit: Option<String>, pub marker: Option<String> }
pub struct PostedComment { pub id: String, pub url: Option<String> }
pub struct TrackerUpdate { pub item: String, pub add_labels: Vec<String>, pub remove_labels: Vec<String>,
    pub transition: Option<String>, pub add_assignees: Vec<String>, pub remove_assignees: Vec<String>,
    pub state: Option<String> /* open|closed */ }
pub struct IssueSpec { pub title: String, pub body: String, pub labels: Vec<String>,
    pub issue_type: Option<String>, pub priority: Option<String>, pub parent: Option<String> }
pub struct TrackerLink { pub from: String, pub to: Option<String>, pub relation: String, pub url: Option<String> }
#[guatiao::kind(name = "ai6_tracker")]
pub trait Tracker: Send + Sync {
    fn capabilities(&self, item: Option<String>) -> Result<TrackerCapabilities, ProviderError>;
    fn issue(&self, item: String) -> Result<TrackerIssue, ProviderError>;
    fn replies(&self, request: ReplyPoll) -> Result<ReplyBatch, ProviderError>;
    fn post(&self, post: TrackerPost) -> Result<PostedComment, ProviderError>;
    fn update(&self, update: TrackerUpdate) -> Result<(), ProviderError>;
    fn create(&self, spec: IssueSpec) -> Result<TrackerIssue, ProviderError>;
    fn link(&self, link: TrackerLink) -> Result<(), ProviderError>;
}
Plugins::trackers() -> Vec<ProviderInfo>;  Plugins::tracker(provider, config: Option<&Value>) -> Result<Box<dyn Tracker>, PluginBuildError>
// A live read Universal Exports asks the Headquarters Station for (serde, deny_unknown_fields):
pub struct SourceRead { pub id, operation: String, pub tracker, item, query: Option<String>,
    pub comments: u32, pub items: u32 }   // no item: lists what the sources find
pub struct SourceReadItem { pub tracker, id, title: String, pub state: Option<String>,
    pub labels, assignees: Vec<String>, pub url: Option<String>, pub updated: String,
    pub author, body: Option<String> }   // text untrusted
pub struct SourceReadComment { pub author, role /* ai6|answers|other */, at, body: String }
impl SourceReadComment { pub fn of(reply: Reply) -> Self; }
pub struct SourceReadAnswer { pub item: Option<SourceReadItem>, pub comments: Vec<SourceReadComment>,
    pub items: Vec<SourceReadItem>, pub error: Option<String> }
```

`Reply::permitted` is the provider's judgement that the author may answer for the issue (its
author or reporter, a repository owner, member or collaborator, a Jira assignee, or someone the
instance's `reply_authors` names); `own` marks the identity AI6 posts as. Settings both kinds
read: the Source's, plus `status_labels` and `status_transitions` (`working`, `waiting`,
`paused`, `done`, `failed`), `agent_labels`, `agent_transitions`, `reply_authors`, `reply_associations` (GitHub),
`issue_type`, `subtask_type` and `priority_map`.

## Linked issues and the outbox (`ai6::directorates::registry::mission_issues`)

```rust
pub enum LinkRelation { Source, Related, Child, Parent, Mirror }   // snake_case in JSON
pub enum LinkState { Proposed, Active, Unlinked }
pub struct IssueLink { pub operation, mission_id, tracker, item: String, pub url: Option<String>,
    pub relation: LinkRelation, pub state: LinkState, pub linked_by: String /* m | agent:CODENAME | person:SUBJECT */,
    pub linked_at: f64, pub status_comment: Option<String>, pub phase: Option<String> }
pub enum OutboxKind { Status, Report, Comment, Question, Update, Create, Link }
pub enum OutboxState { Pending, Held, Sending, Posted, Refused, Failed }
pub struct OutboxEntry { pub id: String, operation, mission_id, tracker, item: String, kind: OutboxKind,
    actor: String, body: String, payload: serde_json::Value, coalesce: Option<String>, state: OutboxState,
    attempts: u32, result: Option<OutboxResult>, error: Option<String>, at: f64, not_before: f64,
    sent_at: Option<f64>, fence: Option<u64> }       // OutboxEntry::new(operation, mission, tracker, item, kind, actor, at)
pub enum QuestionState { Open, Answered, Expired, Withdrawn }
pub struct IssueQuestion { pub id: String /* q-… when empty */, mission_id, tracker, item, entry: String, asked_at: f64, expires_at: f64, state: QuestionState }
pub enum ReplyState { Pending, Delivered, Ignored }
pub struct IssueReply { pub id /* the tracker's comment ID */, mission_id, tracker, item, author_id, author, body /* untrusted */, created: String, state: ReplyState }
pub struct OutboxResult { pub comment_id: Option<String>, url: Option<String>, created_item: Option<String>,
    created_code: Option<(String, Option<String>)> /* the created issue's code and tag */, dropped: Vec<String>,
    attached_to: Option<String> }
// ProposedIssue::mission: Option<Brief>: a person's Mission created with the issue; once the issue
// exists the Headquarters Station enters it with MissionEntry::source (numbered by the issue) and
// links it as its source, audited `mission.create.entered`
pub enum OutboxSettle { Posted(OutboxResult), Refused(String), Retry { error: String, not_before: f64, max_attempts: u32 } }
pub trait MissionIssueStore {   // on FileRegistry and SqlRegistry; part of RegistryBackend
    fn link_issue(&self, link: &IssueLink) -> io::Result<IssueLink>;      // upsert by (Mission, tracker, item); at most [registry.limits.issues] links_per_mission live ones
    fn issue_links(&self, mission_id: &str) -> io::Result<Vec<IssueLink>>;
    fn active_issue_links(&self) -> io::Result<Vec<IssueLink>>;
    fn unlink_issue(&self, mission_id: &str, tracker: &str, item: &str) -> io::Result<bool>;   // refuses its pending entries ("unlinked")
    fn set_issue_status(&self, mission_id, tracker, item: &str, comment: Option<&str>, phase: Option<&str>) -> io::Result<()>;
    fn queue_outbox(&self, entry: &OutboxEntry) -> io::Result<OutboxEntry>;   // an entry naming an item needs an active link in its Operation; `coalesce` replaces a pending entry
    fn outbox_entry(&self, id: &str) -> io::Result<Option<OutboxEntry>>;
    fn outbox_entries(&self, mission_id: Option<&str>) -> io::Result<Vec<OutboxEntry>>;
    fn due_outbox(&self, now: f64, limit: usize) -> io::Result<Vec<OutboxEntry>>;
    fn begin_outbox_send(&self, id: &str, term: u64, now: f64) -> io::Result<Option<OutboxEntry>>;   // pending -> sending under the election term
    fn sending_outbox(&self) -> io::Result<Vec<OutboxEntry>>;
    fn adopt_outbox_send(&self, id: &str, term: u64) -> io::Result<Option<OutboxEntry>>;   // a new term takes a send the old one left
    fn settle_outbox(&self, id: &str, term: u64, settle: OutboxSettle) -> io::Result<OutboxEntry>;   // PermissionDenied unless sending under `term`
    fn resolve_held_outbox(&self, id: &str, approved: bool, reason: &str) -> io::Result<Option<OutboxEntry>>;   // held -> pending, or refused
    fn ask_issue_question(&self, question: &IssueQuestion) -> io::Result<IssueQuestion>;   // AlreadyExists while one on the item is open
    fn issue_questions(&self, mission_id: &str) -> io::Result<Vec<IssueQuestion>>;
    fn open_issue_questions(&self) -> io::Result<Vec<IssueQuestion>>;
    fn settle_issue_question(&self, mission_id: &str, id: &str, state: QuestionState) -> io::Result<bool>;
    fn store_issue_reply(&self, reply: &IssueReply) -> io::Result<bool>;   // false for one already stored; cut to reply_bytes
    fn issue_replies(&self, mission_id: &str) -> io::Result<Vec<IssueReply>>;
    fn pending_issue_replies(&self) -> io::Result<Vec<IssueReply>>;
    fn mark_issue_reply(&self, mission_id, tracker, item, id: &str, state: ReplyState) -> io::Result<bool>;
    fn issue_cursor(&self, operation: &str, tracker: &str) -> io::Result<Option<String>>;
    fn set_issue_cursor(&self, operation: &str, tracker: &str, cursor: &str) -> io::Result<()>;
}
```

Files: `mission-issues/<mission>.json` (the Mission's links) and `issue-outbox/<id>.json`; SQL
tables `ai6_mission_issues` and `ai6_issue_outbox`. `CableStore::take_cable_quotas` is now
`take_quotas(buckets, now)` (same signature); outbox sends take `issue:tracker:OPERATION:NAME`.
`[registry.limits.issues]`: `links_per_mission` 32, `outbox_records` 1000 (settled entries are
dropped oldest first), `entry_bytes` 128 KiB. Station `[sources]` gains `outbox_every` (5 s),
`outbox_batch` (20), `send_attempts` (5), `retry_backoff` (30 s, doubling).

`ai6::directorates::m::issues::IssuePolicy` is the Operation's `[operation.ID.issues]` table
(`Operation::issues`): `preset` (`strict`, `licensed`), `updates` (`outcome`, `milestones`,
`all`), `max_body_bytes` 8192, `posts_per_hour` 6, `post_burst` 3, `posts_per_mission` 40,
`operation_posts_per_hour` 60, `tracker_posts_per_hour` 300, `status_interval_secs` 60,
`reply_ttl_secs` 259200, `on_timeout` (`resume`, `fail`), `on_secret` (`refuse`, `redact`),
`links` (`tracker`, `any`), `create` (`propose`, `direct`, `off`), `max_created` 10,
`max_created_per_day` 50, `max_split_depth` 2; counts are `Limit`s. It is parsed as written,
the one declaration of these keys with their titles (`Schema`); `validate()` refuses what cannot
work, `weakenings()` lists each value looser than the default, printed at start and published at check-in as `issues_weakened`;
`source_pages(&toml::Table) -> Vec<SourcePage { provider, web, project }>` says where an Operation's
`sources` are read on the web. `compose_status(disclosure, phase)` words a status for the cover policy; `is_ready_label(operation,
label)` says whether a label would start work.

`IssueLink` also carries `snapshot: Option<IssueSnapshot { title, body, state, labels, priority }>`, what the issue said when last read (untrusted; `set_issue_snapshot`). `[registry.limits.issues]` adds
`replies_per_mission` 200 and `reply_bytes` 64 KiB; `[sources]` adds `reply_every` 60 s.
`OnTimeout` is `resume`, `fail` or `wait`. New green ink kinds `issue-close`, `issue-assign` and
`issue-comment` (`ISSUE_CLOSE`, `ISSUE_ASSIGN`, `ISSUE_COMMENT`; `licensed` adds `issue-close` to
`00`'s unattended kinds); new exposure source `issue:TRACKER` (`issue_source`) and restriction
`Restriction::Issues` (`"issues"`, in `strict`).

`ai6::directorates::q::outbound` is the check Agent text passes before it is queued:
`check(text, &SecretSet, &LinkRule, max_bytes: Limit, on_secret: OnSecret) -> Result<String, Refusal>`
refuses (in order) empty text, text past the limit, invisible characters (direction controls,
zero-width, tag characters, control characters but newline, tab and carriage return), a link or
image to a host the `LinkRule` does not name (`LinkRule::hosts`, `LinkRule::any`; any scheme but
`http(s)` and every `data:` address is refused), a held secret's exact value (`SecretSet`; values under
six bytes are not matched) and credential shapes (`ghp_`/`gho_`/`ghs_`, `github_pat_`, `ATATT`, `xox?-`,
`AKIA`, PEM private key headers, JWTs, `sk-ant-`); under `OnSecret::Redact` the last two are replaced with
`[redacted]` instead. `Refusal::rule()` names the rule, never a value. Mentions come back in backticks.
`check_person(text, max_bytes)` is what a person's own text passes: empty, too long, invisible
characters and credential shapes are refused; links and mentions are theirs.

On the Station, the issue tools live on the Session's Cable MCP endpoint (`issues`, `issue_read`,
`issue_comment`, `issue_request_info`, `issue_update`, `issue_link`; `station::server::issues`):
tools write outbox entries (comments and questions `agent:CODENAME`, `held` with the approval IDs
in `payload.approvals` until green ink is decided; `payload.retry_key` returns the first answer),
Mission and Operation post buckets `issue:mission:ID` / `issue:operation:ID` and `posts_per_mission`
bound them. The Headquarters Station reads comments (`poll_replies`, every `reply_every`), stores
permitted ones and records the rest as ignored without their text; the Station holding the
Mission delivers them (`deliver_replies`, each scheduler pass, beside green ink decisions),
resumes or fails on an expired question, and withdraws a question whose Mission stopped waiting.
`blocked_reason = "issue"` (`ISSUE_REASON`) is like green ink: a message or output does not end
the wait.

On the Station, a Mission state change queues one `status` entry per active link
(`coalesce = status:MISSION:TRACKER:ITEM`); the source's ending is the `report` entry the
source desk queues for each finished source brief. The Headquarters Station drains the outbox
every `outbox_every`: it builds the Tracker of the entry's instance, sends under its term, and
settles. The first status posts a comment and keeps its ID (`status_comment`); later ones edit
it (a new comment only for `waiting` where editing is unsupported); `status_labels` are
swapped and `status_transitions` made when the issue offers them.

An Agent asks for issues with `issue_create`, `issue_split` and `issue_mirror`: an `IssueProposal
{ id, operation, mission_id, proposer, specs: Vec<ProposedIssue>, exposure, findings, dropped, state:
pending|approved|rejected|created, decided_by, mark_ready, created, failed, retry_key, told, at }`,
kept by `propose_issues`, `issue_proposal`, `issue_proposals`, `decide_issue_proposal`,
`record_created_issues` and `mark_proposal_told`, table `ai6_issue_proposals`. Class authorisations
name kind `issue-create` (`ISSUE_CREATE`), not a governed step. The Headquarters Station queues
`create` entries for approved proposals and settles them in one idempotent pass: it links each new
issue (`child` or `mirror`, `linked_by` the proposer), adds a mirror's back-link comment and remote
link, adds `mark_ready`'s labels as `person:SUBJECT` (audit `issue.ready`), and lists children the
tracker could not attach on their parent. A person's creation is an approved proposal whose Mission
is the `mission` it names, or `operation-created` for none. A `link` entry with `payload.resolve`
reads the issue into the link's `snapshot`, and unlinks it when it cannot be read. An `IssueReply` with
`operator: true` is a person's comment through the gateway: it answers a question and exposes nothing.

`ai6::settings_from_table::<T>(toml::Table)` builds a provider's settings through a Guatiao value, so
its one declaration gives its parsing, defaults and schema. `source_setups` strips `provider`,
`intake`, `interval_secs` and the keys of `InstanceKeys` (`status_labels`, `status_transitions`,
`agent_labels`, `agent_transitions`, `priority_map`, `reopen`) before a provider sees the table.

`reopen` (`label` default, `comment`, `never`) decides what queues a settled item again as its
Mission's next attempt (`BriefStore::reenter_brief`, only over the item's own Source brief): a poll
that finds a label the item's Source link snapshot lacks, AI6's status and Agent labels aside;
with `comment`, also a permitted reply AI6 did not write on a settled Source link, read by the
reply round, which then marks the reply delivered. The snapshot is refreshed while the Mission is
in flight, on each poll of the settled item, and for the newest `[sources] reopen_watch` settled
items per instance every `reopen_watch_every`. Reopens are audited `mission.reopened` and on the
timeline as `source`/`reopened`; `comment` is a start-up weakening. A Station whose book holds the
ended Mission and claims another brief for it (`Mission::brief_digest` differs) queues
it with `MissionStore::reopen` (new prompt and name, the Session hold let go).

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
    pub standing: bool,          // one Mission, each window an attempt of it (default false)
    pub resume: bool,            // needs standing: resume the last attempt's provider Session
}   // standing and resume are left out of the serialized form when false, so the fingerprint
    // of a Sleeper that sets neither is what it was before they existed
impl Sleeper {
    pub fn validate(&self) -> Result<(), String>;   // also refuses a `stations` entry that is no Station ID
    pub fn fingerprint(&self) -> String;   // SHA-256 over every field; any edit changes it
    pub fn due_window(&self, now: i64, since: i64, limits: &SleeperLimits) -> Option<i64>;   // after since, within catch_up
    pub fn latest_window(&self, now: i64, limits: &SleeperLimits) -> Option<i64>;
    pub fn window_before(&self, window: i64, limits: &SleeperLimits) -> Option<i64>;
}
pub struct SleeperAuthorization { pub fingerprint: String, pub authorized_by: String, pub at: i64, pub mark: Option<SleeperMark> }
pub struct SleeperMark { pub since: i64, pub window: i64, pub settled: bool, pub why: Option<String> }
impl SleeperMark { pub fn new(since: i64, window: i64) -> Self; }   // settled when window <= since
pub enum Fire { Queued, AlreadyFired, InFlight /* a standing Sleeper's earlier attempt is still in flight */ }
pub fn window_of(brief: &Brief) -> Option<&str>;   // the window a Sleeper's brief was queued for
pub const SLEEPER_BRIEF_SOURCE: &str = "sleeper";

pub struct Cron;
impl Cron {
    pub fn parse(expression: &str) -> Result<Cron, CronError>;
    pub fn latest_at_or_before(&self, now: i64, lookback_days: u32) -> Option<i64>; // Unix seconds
}
pub fn window_label(window: i64) -> String; // "20261025T0300Z"
/// The brief of one window; with `station`, that Station's round, narrowed to it.
pub fn sleeper_brief(operation: &str, name: &str, sleeper: &Sleeper, window: i64, station: Option<&str>) -> Brief;

SleeperStore::authorize_sleeper(operation: &str, name: &str, sleeper: &Sleeper, authorized_by: &str, at: i64) -> io::Result<()>
SleeperStore::sleeper_authorization(operation: &str, name: &str) -> io::Result<Option<SleeperAuthorization>>
SleeperStore::revoke_sleeper(operation: &str, name: &str) -> io::Result<bool>   // and its mark
SleeperStore::mark_sleeper(operation: &str, name: &str, mark: &SleeperMark) -> io::Result<bool>   // false with no approval
SleeperStore::fire_sleeper(operation: &str, name: &str, sleeper: &Sleeper, round: (i64, Option<&str>), received_by: &str) -> io::Result<Fire>  // (window, station)
```

Cron fields take `*`, numbers, `a-b`, `,` lists and `/step`; day-of-week is 0-7 (0 and 7 are
Sunday); when both day fields are restricted a day matches if either does. A schedule that
never matches within a year (`0 0 30 2 *`) parses but never fires.

`fire_sleeper` enters a brief whose `id` is `<operation>.<name>.<window>` (with a Station,
`...<window>.<station>`, the ID made key-safe with a digest when it had to change) with the entry
`MissionEntry::sleeper` for that window and Station, and reports `AlreadyFired` when that entry's
brief exists in `new`, `claimed`, `done` or `failed`, which is what makes a window, or one
Station's round in it, fire once across Stations. A `standing` Sleeper enters its first window
with `MissionEntry::standing` and every later one through `reenter_brief`: `AlreadyFired` when the
brief is for that window, `InFlight` (logged once per window) when an earlier window's attempt is
queued, held or claimed. Its data's `sleeper` object also carries `standing` and `resume`; a
start whose claimed brief says `resume`, with the Registry's Sleeper provenance, resumes the
latest run's provider Session when that run had the same handler, and records `sleeper`/
`resumed` or `started` on the timeline. A brief's
`stations` is the sleeper's, or `[station]` for a round; its data's `sleeper` object names
`operation`, `name`, `window` and, for a round, `station`. For an `each_station` sleeper the
Headquarters holder fires one round for each Station whose last check-in placement accepts
for the Operation and the sleeper's Agent or role, within the sleeper's `stations`; load,
resources and quarantines are ignored there (they only delay a brief), and a Station dark
past `[registry.limits.stations] gone_dark_after` gets no round until it checks in again.
`authorize_sleeper` refuses a prompt over `[registry.limits.sleepers] prompt_bytes`.
Authorizations are stored under `sleepers/` (SQL: the record column of
`ai6_sleeper_authorizations`), each with what the Headquarters holder has reckoned of its windows
(`mark`), which an approval given again keeps and a revoke ends. The Headquarters holder fires only
sleepers whose stored fingerprint equals the current definition, using the latest window after
`mark.since` (the first approval's time) and within `[registry.limits.sleepers] catch_up` (`None`,
the window's own period; `TimeLimit::UNBOUNDED`, no limit). Each window that passes unfired is
recorded as `sleeper`/`skipped` (details `sleeper`, `window`, `why`) once a later window comes, or at
once when past the bound; `skipped_events` (64) are named one by one and any older ones are one
event. A late firing is `sleeper`/`late`. A Sleeper brief filed as failed records
`sleeper`/`attempt-failed` (details `sleeper`, `window`, `reason`; never a harness's words, which
are in the outcome with `reason_from_harness`). A Station holding a standing attempt that is stalled,
has no live Session or re-attach pending, or failed to start, when the next window is due, fails it,
audits `sleeper.superseded` (target `<mission> <window>`) and records `sleeper`/`superseded`.

## Station logbooks (`ai6::directorates::registry::logbook`)

Each Station's logbook keeps the newest `[registry.limits.logbook] entries` (20) reports its Head of Station
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
    pub fn validate(&self, limits: &LogbookLimits) -> io::Result<()>;   // InvalidInput: Station ID, summary, findings out of limits
}
LogbookStore::file_station_report(report: &StationReport) -> io::Result<()>   // drops the oldest past `entries`
LogbookStore::logbook(station: &str) -> io::Result<Vec<StationReport>>         // oldest first; empty when none
```

## Watches (`ai6::directorates::m::watches`, `ai6::directorates::registry::watches`)

A watch is one standing Session per scope (`m`, `operation:<codename>`, `station:<id>`), run by
the Station the scope names (M's on the Headquarters Station, an Operation's on an execution
Station it allows, a Station's on itself) and woken by its area's events, one turn per wake. It is
not a Mission: no Mission record, brief, placement, attempt or Session record names it. Its record
(`watches/<hex of scope>.json`, SQL table `ai6_watches`) holds its lease, fence, provider Session
and handler, logbook, last wake, the turns of the last hour, what it was told, and controls.
`docs/watches.md` has the behaviour. `Watch`, `WatchDefinition` and `WatchScope` are at the crate
root.

```rust
pub const M_SCOPE: &str = "m"; pub const OPERATION_PREFIX: &str = "operation:"; pub const STATION_PREFIX: &str = "station:";
#[non_exhaustive] pub enum WatchScope { M, Operation(String), Station(String) }  // FromStr, Display
impl WatchScope { pub fn operation(&self) -> Option<&str>; pub fn station(&self) -> Option<&str>; }
/// `[watch.<scope>]`; ToValue, FromValue, Schema, Form; deny_unknown_fields.
#[non_exhaustive] pub struct WatchDefinition {
    pub agent: String, pub operation: Option<String>,   // operation: M's/a Station's Cables; an Operation's may only repeat its own
    pub wake_every: TimeLimit,            // 3600; none: events only
    pub max_turns_per_hour: Limit,        // 30, counted on the record across Stations
    pub max_events_per_turn: Limit,       // 50
    pub max_waiting_events: Limit,        // 500; past it the oldest are dropped and logged
    pub turn_timeout: TimeLimit,          // 1800; the turn is interrupted
    pub lease: Seconds,                   // 60; a lapsed lease is taken over
}
impl WatchDefinition {
    pub const DEFAULTS: Self; pub fn new(agent) -> Self;
    pub fn wake_every(self, TimeLimit) -> Self; pub fn max_turns_per_hour(self, Limit) -> Self; pub fn lease(self, Duration) -> Self;
    pub fn cable_operation<'a>(&'a self, scope: &'a WatchScope) -> Option<&'a str>;
    pub fn validate(&self, scope: &WatchScope) -> Result<(), String>;
}
#[non_exhaustive] pub struct Watch { pub scope: WatchScope, pub settings: WatchDefinition }   // Watch::new(scope, settings)
pub fn watches_from_records(records) -> Result<BTreeMap<String, Watch>, String>;
Roster::watches(&self) -> &BTreeMap<String, Watch>;   // by scope; `watches.toml` in Roster::load

// ai6::directorates::registry::watches
pub const TURN_WINDOW_SECS: f64 = 3_600.0;
#[non_exhaustive] pub struct WatchLimits { pub records: Limit /*1024*/, pub record_bytes: Limit /*1 MiB*/,
    pub logbook: Limit /*100*/, pub entry_bytes: Limit /*2 KiB, longer cut*/, pub seen: Limit /*1024*/, pub controls: Limit /*32*/ }
#[non_exhaustive] pub struct WatchLogEntry { pub at: f64, pub kind: String, pub text: String }   // new(kind, text, at)
#[non_exhaustive] pub struct WatchControl { pub id: String, pub by: String, pub note: String, pub at: f64 }
#[non_exhaustive] pub struct WatchRecord { pub scope, pub agent, pub station, pub fence: u64, pub lease_expires: f64,
    pub opened_at: f64, pub provider_session_id: Option<String>, pub handler: Option<String>, pub last_wake: Option<f64>,
    pub turns: Vec<f64>, pub logbook: Vec<WatchLogEntry>, pub seen: Vec<String>, pub controls: Vec<WatchControl> }
impl WatchRecord { pub fn turns_since(&self, since: f64) -> usize; }
#[non_exhaustive] #[derive(Default)] pub struct WatchUpdate { pub session: Option<Option<(String, String)>>,
    pub last_wake, pub turn_at, pub log: Vec<WatchLogEntry>, pub seen: Option<Vec<String>>, pub taken_controls: Vec<String> }
impl WatchUpdate { session(handler, id), forget_session(), woke(at, turn: bool), log(entry), seen(keys), took_controls(ids) }
#[non_exhaustive] pub enum WatchError { NotFound, HeldBy(String), Stale, Io(io::Error) }
pub trait WatchStore {
    fn claim_watch(&self, scope, agent, station, ttl: Duration) -> Result<WatchRecord, WatchError>;
        // makes, renews (same Station, same fence) or takes over a lapsed lease (fence + 1, logged);
        // another Agent forgets the provider Session; HeldBy while another's lease is live
    fn update_watch(&self, scope, station, fence: u64, renew: Option<Duration>, update: WatchUpdate) -> Result<WatchRecord, WatchError>;  // Stale unless holder and fence
    fn request_watch_wake(&self, scope, by, note) -> Result<WatchControl, WatchError>;   // no lease; NotFound before a first claim
    fn watch(&self, scope: &str) -> io::Result<Option<WatchRecord>>;
    fn watches(&self) -> io::Result<Vec<WatchRecord>>;   // by scope
}
```

## Moneypenny's conversation log (`ai6::directorates::registry::assistant_log`, also at the crate root)

One record per entry of a conversation, kept by Universal Exports; its retention is the gateway's
(`[assistant.gateway] log_age`, `log_bytes`). An entry past `[registry.limits.assistant_log]
entry_bytes` (1 MiB) is kept without its data, its text cut to half the bound, with `cut_from`.
Files `assistant-log/<conversation>/<micros>-<id>.json`; SQL table `ai6_assistant_log` (`id`,
`conversation`, `person`, `at_micros`, `bytes`, `entry` JSON, `revision`), made at the next open.

```rust
#[non_exhaustive] pub struct AssistantTurn { pub station: Option<String>, pub session: Option<String>,
    pub agent: Option<String>, pub route: Option<RouteRef>, pub account: Option<AccountRef>,
    pub model: Option<String>, pub tokens: Option<u64> }               // Default
#[non_exhaustive] pub struct AssistantLogEntry { pub id: String /* UUIDv7 */, pub conversation: String,
    pub person: String, pub at: f64, pub kind: String /* [a-z_], at most 32 */, pub text: String,
    pub data: Option<Value>, pub turn: Option<AssistantTurn>, pub cut_from: Option<u64> }
impl AssistantLogEntry { pub fn new(conversation, person, kind, text, at: f64) -> Self;
    pub fn with_data(self, Value) -> Self; pub fn with_turn(self, AssistantTurn) -> Self; }
#[non_exhaustive] pub struct AssistantConversation { pub conversation: String, pub person: String,
    pub first_at: f64, pub last_at: f64, pub entries: u64, pub bytes: u64 }
AssistantLogStore::log_assistant_entry(&AssistantLogEntry) -> io::Result<()>     // InvalidInput: bad ID, identity, time or kind
AssistantLogStore::assistant_log(conversation: &str) -> io::Result<Vec<AssistantLogEntry>>  // oldest first
AssistantLogStore::assistant_conversations(person: Option<&str>) -> io::Result<Vec<AssistantConversation>> // newest first
AssistantLogStore::prune_assistant_log(before: Option<f64>, bytes: Limit) -> io::Result<u64> // by age, then the oldest past `bytes`
```

## Session contexts (`ai6::directorates::registry::contexts`, also at the crate root)

What a Session's harness kept of it, one record per provider Session, replaced each time it is
filed. `[registry.limits.contexts]`: `context_bytes` (8 MiB; the newest part is kept, from a line
start, with `cut_from` the size it had) and `records` (2,000; the oldest filed go). Files
`contexts/<session>.json` (the record) and `.log` (its content); SQL table `ai6_session_contexts`
(`session` key, `conversation` and `operation`, each `''` when it is not one, `filed_micros`,
`record` JSON, `content` text, `revision`), made at the next open.

```rust
pub const EVENTS_FORMAT: &str = "ai6-events-jsonl";   // the root's CONTEXT_EVENTS_FORMAT
#[non_exhaustive] pub enum ContextOwner { Mission { operation: String, mission: String /* UID */ },
    Conversation { conversation: String } }            // JSON tagged by "kind"
#[non_exhaustive] pub enum ContextSource { Harness, Events }   // "harness", "events"
#[non_exhaustive] pub struct SessionContext { pub session: String, pub owner: ContextOwner,
    pub station: String, pub handler: String, pub format: String, pub source: ContextSource,
    pub filed_at: f64, pub cut_from: Option<u64>, pub content: String /* not in its JSON */ }
impl SessionContext { pub fn new(session, owner, station, handler, format, source, filed_at, content) -> Self;
    pub fn cut_to(self, Limit) -> Self; pub fn conversation(&self) -> Option<&str>;
    pub fn operation(&self) -> Option<&str>; }
SessionContextStore::file_session_context(&SessionContext) -> io::Result<()>   // InvalidInput without its parts
SessionContextStore::session_context(session: &str) -> io::Result<Option<SessionContext>>
SessionContextStore::conversation_contexts(conversation: &str) -> io::Result<Vec<SessionContext>> // oldest filed first
```

The Station files a Mission Session's context when its event pump ends, and Moneypenny's after
each turn and when her harness closes: the file the handler's `session_record` names, else the
events it saw (`EVENTS_FORMAT`, one `{at, kind, text, tools, usage?, data?}` per line). A harness a
runner supervises on another host always gets its events. Helpers for a handler:
`ai6::directorates::section_00::records::{home, configured_dir, file_safe, find_file}`.

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
#[non_exhaustive] pub struct Finding { pub kind: FindingKind, pub detail: String /* never brief text */, pub related: Option<String> }
impl Finding { pub fn new(kind, detail) -> Self; pub fn related(self, mission_or_key) -> Self; }
#[non_exhaustive] pub enum VettingVerdict { Pass, Hold, Refuse }   // serde snake_case
#[non_exhaustive] pub struct Vetting { pub verdict: VettingVerdict, pub findings: Vec<Finding> }
impl Vetting {
    pub fn pass() -> Self;
    pub fn from_findings(findings: Vec<Finding>) -> Self;  // any Malformed → Refuse; any → Hold; none → Pass
    pub fn within(self, limits: &VettingLimits) -> Self;   // findings ≤ limits.findings, each detail ≤ detail_bytes
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
#[non_exhaustive] pub struct KnownWork { pub reference: String, pub title: String, pub url: Option<String>,
    pub sleeper: Option<(String, String)> }   // Operation and Sleeper; windows of one Sleeper are no duplicates
pub fn sleeper_of(brief: &Brief) -> Option<(String, String)>;   // data's Sleeper, with a Sleeper provenance naming it
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
    pub limits: VettingLimits,                 // the verdict is kept within; more phrases than
                                               // `phrases` (or longer than phrase_bytes) hold screened briefs
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
    pub fn review_entry(&self, operation: &str) -> MissionEntry;  // one review has one Mission ID
}
pub fn is_review(brief: &Brief) -> bool;             // REVIEW_SOURCE and an Origin::Review provenance
pub fn review_target(brief: &Brief) -> Option<ReviewTarget>;  // None unless is_review
/// role chief-of-staff, priority of the reviewed brief, data: target + exposure ["brief:source"],
/// provenance Origin::Review (received_by, received_at; reference = review_id);
/// the brief and up to `limits.review_work` other work titles quoted between markers neither
/// contains, each line cut to `limits.detail_bytes`.
pub fn review_brief(reviewed: &Brief, target: &ReviewTarget, (operation, purpose): (&str, &str),
                    (work, limits): (&[KnownWork], &VettingLimits), received: (&str, i64)) -> Brief;
/// The JSON objects shaped {"verdict", "findings"?} in the final message's last 16 KiB: any
/// "hold" → Review findings "KIND: detail" (duplicate|scope|manipulation); exactly one
/// "clear" with no findings and only whitespace after it → pass; anything else (none, several,
/// another verdict, text after) → hold with "review-error: ...". Never admits on a bad answer.
pub fn read_review(text: &str, limits: &VettingLimits) -> Vetting;
```

A review Mission is one with the role and an `Origin::Review` provenance. Its Station starts it only
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
// [registry.limits.held]: per_operation (1,000; past it a brief stays queued), records
// (10,000 read across Operations), results (500), reason_bytes (1,024), actor_bytes (256)
#[non_exhaustive] pub struct HeldBrief { pub operation: String, pub brief: Brief, pub vetting: Vetting, pub held_at: f64 }
impl HeldBrief { pub fn held_only_for_rate(&self) -> bool; }
#[non_exhaustive] pub struct Clearance { pub kinds: BTreeSet<FindingKind>, pub digest: String, pub by: String, pub at: f64 }
impl Clearance { pub fn kinds_for(&self, brief: &Brief) -> BTreeSet<FindingKind>; } // empty unless digest matches

VettingStore::hold_brief(operation, key, vetting: &Vetting, at: f64) -> io::Result<Option<HeldBrief>> // None: not queued
VettingStore::held_briefs(operation, limit /* 1..=results */) -> io::Result<Vec<HeldBrief>>     // oldest first
VettingStore::release_held(operation, key, by, at) -> io::Result<Brief>   // again: same brief; AlreadyExists once claimed/finished
VettingStore::refuse_held(operation, key, by, reason /* ≤ reason_bytes */, at) -> io::Result<Brief>  // idem
VettingStore::requeue_held(operation, key) -> io::Result<bool>          // back to new/ with no clearance
VettingStore::withdraw_brief(operation, key, by, note /* ≤ reason_bytes */, at) -> io::Result<Brief>
                                         // a queued or held brief of `operation` filed as failed;
                                         // again: same brief; NotFound: none queued/held; AlreadyExists: claimed/finished
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
`withdraw_brief` files a queued or held brief as failed with outcome `{"state": "withdrawn",
"mission_id": KEY, "reason": "withdrawn by BY[: NOTE]", "withdrawn_by", "withdrawn_at"}`.

The Headquarters Station files a claimed brief whose cancel was first asked for at least
`[registry.limits.briefs] cancel_settle_after` (`BriefLimits::cancel_settle_after: Seconds`, 300)
ago as failed with `{"state": "failed", "mission_id", "reason": "cancelled; the Station did not
confirm", "station", "session_id", "cancel_requested_at"}`, through `finish` with the lease's
Station; the Station holding the Mission stops it at check-in while its last Session is still
`session_id`.

## Agent Client Protocol wire (`ai6::directorates::section_00::acp`, feature `acp`)

The Station is an ACP client (agentclientprotocol.com, schema v1, newline-delimited JSON-RPC)
for every Agent Session. One connection per Session; each connection runs on threads of its
own (a `futures` executor), so it needs no async runtime and works inside or outside Tokio.

```rust
pub const LAUNCH_META_KEY: &str = "dev.coqui.ai6/launch";   // _meta of session/new|load|resume
pub const EVENTS_CAPABILITY: &str = "dev.coqui.ai6/events"; // agentCapabilities._meta key
pub const EVENT_NOTIFICATION: &str = "_ai6/event";          // {sessionId, providerSessionId, event?}
pub const RUNNER_TOKEN_METHOD: &str = "ai6-runner-token";   // authenticate._meta.token
pub fn duplex() -> (Channel, Channel);                       // in-memory pair (the SDK's Channel)

#[non_exhaustive] pub struct HandlerSpec { pub id: String, pub config: serde_json::Value }
// Every AgentStart field ACP has no home for; cwd is ACP's cwd, the prompt is session/prompt.
// Debug lists environment variable NAMES only. JSON via serde (`to_value()`).
#[non_exhaustive] pub struct LaunchMeta { pub mission_id, pub agent_id, pub initial_context, pub clearance,
    pub environment: BTreeMap<String,String>, pub model, pub effort, pub max_cost_micro_usd,
    pub project_settings, pub project_mcp, pub answer_only, pub assistant, pub handler: Option<HandlerSpec>,
    pub terminal: Option<String>, pub key: Option<String>,
    pub cables: bool }               // a field runner gives the harness a Cable endpoint relayed over this connection
impl LaunchMeta { pub fn from_start(&AgentStart) -> Self; pub fn into_start(self, prompt: String,
    working_directory: String) -> AgentStart; pub fn with_handler(self, HandlerSpec) -> Self;
    pub fn with_terminal(self, Option<String>) -> Self; pub fn with_key(self, impl Into<String>) -> Self;
    pub fn with_cables(self, bool) -> Self; pub fn to_value(&self) -> Value }
#[non_exhaustive] pub struct ResumeMeta { /* as AgentResume: session_id, message, clearance, environment, ... */ }
impl ResumeMeta { pub fn from_resume(&AgentResume) -> Self; pub fn into_resume(self, working_directory: String)
    -> AgentResume; /* with_handler, with_terminal, with_key, with_cables, to_value as LaunchMeta */ }

// Re-exported from `coqui_acp`, the ACP client the Session is built on (pinned by revision):
// `Decider` as PermissionDecider, `Decision` as PermissionDecision, PermissionRequest,
// PermissionOption, RefuseAll (the default decider), Authenticate, RpcFailure and `Exchange`, the
// calls that are no session: `Exchange::new().client_info(name, version).authenticate(a)
// .initialize_timeout(Option<Duration>).request_timeout(Option<Duration>).describe_limits(l)`, then
// `.run(transport, calls)` or `.describe(transport)` (what an agent offers before any session).
pub trait PermissionDecider: Send + Sync { fn decide(&self, &PermissionRequest) -> PermissionDecision; }
pub enum PermissionDecision { Allow, Deny(String) }   // Allow -> the first allow_once option, Deny -> reject_once
pub const UNBOUNDED_MS: u64 = u64::MAX;               // a timeout that never fires
pub const UNBOUNDED_LINE_BYTES: u64 = u64::MAX;       // a line length that never fires

pub struct AcpOptions {          // Default: timeouts 20000 / 30000 / 2000 ms, no floors
    pub cwd: String, pub mcp_servers: Vec<Value>, pub meta: Map<String, Value>,
    pub model: Option<String>, pub effort: Option<String>,
    pub model_option: Option<String>, pub effort_option: Option<String>,  // config option ids
    pub prompt: Option<String>, pub resume: Option<String>, pub restricted: bool,
    pub authenticate: Option<Authenticate>, pub initialize_timeout_ms: u64,
    pub request_timeout_ms: u64, pub close_grace_ms: u64, pub decider: Arc<dyn PermissionDecider>,
    pub fail_on_disconnect: bool,                     // true: a connection lost mid-turn is `failed`
    pub limit_phrases: Vec<String>,                   // empty: no failure is a limit
    pub disconnect_detail: Option<DisconnectDetail>,  // what became of a lost agent
    pub text_flush_bytes: usize,                      // DEFAULT_TEXT_FLUSH_BYTES (65536); any value
    pub max_tool_kinds: usize,                        // DEFAULT_MAX_TOOL_KINDS (4096); any value
    pub extensions: Option<Arc<dyn ExtensionHandler>> }  // the agent's `_` notifications but `_ai6/event`
pub type DisconnectDetail = Box<dyn FnOnce() -> Option<String> + Send>;  // asked at most once
/// Handed each extension notification as the Session's reader takes it: return at once.
pub trait ExtensionHandler: Send + Sync { fn handle(&self, method: &str, params: &Value, reply: Notifier); }
#[derive(Clone)] pub struct Notifier;   // notifies the agent on the Session's connection while it is held
impl Notifier { pub fn new(send: impl Fn(&str, Value) -> bool + Send + Sync + 'static) -> Self;
    pub fn notify(&self, method: &str, params: Value) -> bool; }  // whether it was taken
pub struct AcpSession;           // implements AgentSession
impl AcpSession {
    /// initialize -> (authenticate) -> session/new | session/resume | session/load -> config
    /// options -> session/prompt. `on_close` ends the agent's process on close or failed open.
    pub fn connect(transport: impl ConnectTo<Client> + 'static, options: AcpOptions,
        on_close: Option<Box<dyn FnOnce() + Send>>) -> Result<Self, ProviderError>;
    pub fn into_handle(self) -> Object<dyn AgentSession>;
}

pub struct AcpFacade;            // an AgentHandler served as an ACP agent
impl AcpFacade {
    pub fn new(handler: Arc<dyn AgentHandler>, terminal: TerminalFactory) -> Self;
    pub fn with_auth(self, FacadeAuth) -> Self;           // authenticate required before session/*
    pub fn serve(&self, transport: impl ConnectTo<Agent> + 'static) -> JoinHandle<()>;
}
// NDJSON transports are `coqui_acp::lines::over_io(reader, writer, diagnostic_lines, max_line)`;
// `UNBOUNDED_LINE_BYTES` as `max_line` sets no limit.
pub struct TerminalIo;           // a TerminalSession as blocking Read + Write
```

- **Client behaviour.** The protocol state machine is `coqui_acp`'s (connect, resume, turns,
  permissions, refused kinds, line transports); `AcpSession` adds the launch extension, the
  runner's sequence numbers and the words of `limit`, `blocked` and `failed`. Client capabilities: no `fs`, no `terminal`. `session/resume` is used
  when advertised, else `session/load` (replayed `session/update`s before its answer are dropped),
  else the connect fails. `authenticate` is sent only when `AcpOptions::authenticate` is set; a
  session error mentioning auth lists the agent's `authMethods`. MCP servers of `type` `http`/`sse`
  need `mcpCapabilities`; without them the connect fails with that reason. Model and effort go
  through the agent's config options (by id, else category `model` / `thought_level`, else the
  ids `model` / `effort`); an option the agent lacks is reported as a `notice` event, not guessed.
- **Events.** An agent advertising `EVENTS_CAPABILITY` (the façade) is read through
  `_ai6/event` alone: every `AgentEvent` verbatim, so `limit` and `quota` survive. Any other agent
  is read from `session/update` and the stop reason: message chunks coalesce into one `text`
  event per turn segment, `tool_call` is a `tool` event, `usage_update.cost` (USD) is `usage` with
  `cost_micro_usd` (token counts are not guessed); `end_turn|max_tokens|max_turn_requests` ->
  `completed`, `refusal` -> `failed`, `cancelled` -> `completed` after `interrupt`, else
  `cancelled` (`coqui_acp_provider::CANCELLED_EVENT`) with the text `TURN_CANCELLED`, a JSON-RPC
  error -> `failed` in its message and the agent's reason (`data`, or an object `data`'s
  `message`), or `limit` when that text holds one of `limit_phrases`. A connection that ends unclosed
  while the turn is not over is `failed`: `the agent closed its connection`, then
  `disconnect_detail`'s answer, and never a limit, whatever the agent last wrote; a turn the façade
  already ended (a terminal event on `_ai6/event`) is not failed by the agent leaving. A bridge's
  report of output lost while nobody was connected is a `failed` event naming the lines and bytes
  lost; a tool kind forgotten past `max_tool_kinds` is a `notice`, and its request reaches the
  decider with no kind. An agent answering `initialize` with a protocol version other than 1 is
  refused.
- **Restricted.** With `AcpOptions::restricted`, only a request of kind `read`, `search` or `think`
  or for a tool of a server in `mcp_servers` (the Grok CLI's `UseTool` input, else a title
  `mcp__<server>__`) reaches the decider; every other is refused. A tool of a `RESTRICTED_KINDS`
  kind that runs unasked ends the Session.
  No limit or quota detection for external agents.
- **Permissions.** `session/request_permission` always gets an outcome: what cannot be parsed,
  decided or answered is `cancelled` with a `blocked` event, never an error. It goes to the decider on its own thread. With
  `restricted`, a request for a tool of kind `edit|delete|move|execute` is refused without asking
  the decider, and a tool of those kinds that runs without a request ends the Session (`failed`).
  An answer with no option of the needed kind cancels the request and emits `blocked`.
- **Façade.** `session/new` needs `_meta[LAUNCH_META_KEY]` (`LaunchMeta`); the operation starts on
  the first `session/prompt` (`handler.start`), so a start failure is the prompt's error and the
  client's `connect` fails with it. `session/resume|load` with `ResumeMeta` calls
  `handler.resume` (a `message` there is submitted by the handler); `session/cancel` ->
  `interrupt` and answers waiting prompts `cancelled`; `session/close` -> `close`. A `completed`
  event answers waiting prompts `end_turn`, `limit` `refusal`, `cancelled` (or `blocked` with
  `TURN_CANCELLED`) `cancelled`, `failed` a JSON-RPC error. It
  advertises `loadSession`, `sessionCapabilities.resume|close` and no MCP (the environment
  carries the native handlers' MCP servers).

Station use: every Session a Station supervises itself (local, detached wrapper, Moneypenny's
desk) is an `AcpSession` over an in-process `AcpFacade` of its handler (`station::acp`), with no
start, resume or close limit (`UNBOUNDED_MS`, as before). `Plugins::handler_with(provider,
&agent.handler_settings)` builds a handler with an Agent's settings as `HandlerConfig {
settings_toml }` (the provider refuses unknown keys); empty settings build it with defaults.
`GateDecider::from_environment(&environment, cwd)` (feature `gate`) is a `PermissionDecider` that
asks the Session's Cable endpoint, through the same `gate()` a Claude `PreToolUse` hook calls,
whether an `execute` request's command may run; other tool kinds are allowed. Each answer is a
`permission` event. Operator terminal input for a handler that declares `HarnessStdio::Json` is
not written to its pipes: typed lines become messages, Ctrl-C interrupts, other control bytes
are refused with a reason.

## Identity and handler API

`MissionId::try_new(impl Into<String>)` and `SessionId::try_new(impl Into<String>)`
validate non-empty values up to 256 UTF-8 bytes with no control characters. Both also
implement `FromStr` and `Display`. A Mission can group several provider Sessions; a
Session ID is issued by the provider and stays stable when that Session resumes.

With `station-tls` (`ai6::station::identity`), a Station's mutual TLS is
[`coqui-peer-tls`](https://github.com/Boriken-Dev/coqui-peer-tls) and `coqui-peer`, and this
module fixes where AI6 keeps things and the protocol such a link speaks. `ai6 identity` and `ai6
peer` use the identity and the store; nothing in `ai6 serve` dials or listens on the protocol, since
Stations coordinate through the Registry:

```rust
pub const STATION_ALPN: &[u8] = b"ai6-station/1";
pub const IDENTITY_FILE: &str = "identity.pem";
/// The sealing key `public_key` of `station`, signed by this identity, which signs and never decrypts.
pub fn sign_sealing_key(identity: &Identity, station: &str, public_key: &str) -> Result<StationSealingKey, String>;
/// A signature it carries must hold; `fingerprint`, when named, must be its certificate's.
pub fn check_sealing_key(record: &StationSealingKey, fingerprint: Option<&str>) -> Result<(), ()>;
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
`identity.pem` (Unix mode `0600`). AI6 passes `Policy::modern()`: TLS 1.3, SHA-2 pins;
coqui-peer-tls's default `intermediate()` and `legacy()` loosen it, and `Built::weakened` lists
what is looser than that default.
`client_config` pins the server's fingerprint; `server_config` requires a client certificate
and proof of its key, then applies `trust`. A peer not in the store is refused and, once it
has proved its key, recorded under `peers/waiting` with the address it came from; waiting is
bounded per source address, in total, and by how often one peer is written down again (the
policy's `waiting`: `per_source`, `total` and `rerecord`). An operator lists
them with `ai6 peer waiting` (fingerprint and source address) and accepts one with
`ai6 peer accept <fingerprint>`. Both work on `ai6.cfg`'s `[station.identity] dir` (default: its
state directory).
`AccessTokenRecord` is the device-token metadata value. It holds the
canonical UUID revocation ID, SHA-256 digest, authoritative Directory person ID, and issue,
expiry and optional revocation times; its debug output redacts the digest. `into_session_cookie()`
marks a record as a sign-in's session cookie (`is_session_cookie()`) and `in_sign_in(token_id)` a
bearer token as belonging to that sign-in (`sign_in()`).
`AccessTokenStore` (`store_access_token`; `find_access_token`, which never answers with a session
cookie, nor with a token whose sign-in was revoked; `find_session_cookie`, the converse;
`login_by_id`, an active session cookie or bearer record (whose sign-in stands) by its ID;
`revoke_access_token`, where a token of a sign-in takes the sign-in with it;
`extend_access_token`, which moves an active bearer record's expiry later and never earlier);
`FileRegistry` stores and validates records in the `access-tokens/` and `access-token-digests/`
directories, returns only active records, and serializes revocation and extension under the
shared Registry lock.

## Q Branch loadouts (`ai6::directorates::q`)

```rust
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
    pub fn overlay_names(&self) -> &[String];   // [overlay.<name>] names, as declared
    pub fn secret_names(&self) -> &[String];
    pub fn env(&self) -> &SessionEnv;   // [loadout.X] env: non-secret values a Session is given, which exposure does not strip
    /// Declares only the account's secret (agent "pool:<id>/<name>"), so a broker leases it
    /// alone; None for a config-dir account.
    pub fn for_account(pool: &AccountPool, account: &Account) -> Option<Self>;
}
#[non_exhaustive] pub enum AccountKind { ClaudeOauthToken, AnthropicApiKey, AnthropicAuthToken,
    OpenaiApiKey, CodexAccessToken, ConfigDir, HarnessLogin }   // as_str()/parse(): "claude-oauth-token", …, "config-dir"
impl AccountKind {
    pub fn env(self) -> Option<&'static str>;          // CLAUDE_CODE_OAUTH_TOKEN, ANTHROPIC_API_KEY, ANTHROPIC_AUTH_TOKEN
    pub fn login_flag(self) -> Option<&'static str>;   // --with-api-key, --with-access-token
    pub fn reads_secret(self) -> bool;                 // all but ConfigDir
    pub fn carries_session(self) -> bool;              // the env kinds: a Session may move accounts
}
pub const CREDENTIAL_VARIABLES: &[&str];   // every provider credential variable a pooled Session is stripped of
#[non_exhaustive] pub enum PoolStrategy { Headroom /* default */, RoundRobin, Priority }   // "headroom" | "round_robin" | "priority"
pub struct Account;   // Account::new(name, kind) .secret(name) .dir(path) .env(var) .priority(i64) .max_concurrent(n) .label(k, v)
                      // HarnessLogin only: .files(paths in a home) .renew_command(argv) .renew_env(k, v)
impl Account { pub fn name(&self) -> &str; pub fn kind(&self) -> AccountKind;
    pub fn login_files(&self) -> &[String]; pub fn keeps_login_files(&self) -> bool;   // sealed in the Registry
    pub fn renewal(&self) -> &[String]; pub fn renewal_env(&self) -> &BTreeMap<String, String>;
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
    pub fn admits(&self, operation: Option<&str>, ancestors: &[String]) -> bool;   // a glob matching its ID or an ancestor's
    pub fn validate(&self) -> Result<(), String> }   // the catalog also refuses an account whose secret is green ink
pub enum PoolChoice<'a> { Unpooled, Pools(Vec<&'a AccountPool>), Refused(String) }
/// The three-way rule: a pool is usable when it serves `handler`, its globs admit
/// `operation` or one of its `ancestors` (nearest first), and `allowed` (the Operation's
/// `pools`; None = every pool) names it; `pinned` (the Agent's or route's `pool`) narrows to one,
/// the only way to a `named_only` pool. Refused names each pool and why.
pub fn choose_pools<'a>(pools: impl IntoIterator<Item = &'a AccountPool>, handler: Option<&str>,
    operation: Option<&str>, ancestors: &[String], allowed: Option<&[String]>,
    pinned: Option<&str>) -> PoolChoice<'a>;
/// What `choose_route` compares of a route with room; `used_permille: None` counts as `unknown_use`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RouteStanding { pub used_permille: Option<u16>, pub active: usize, pub last_used: Option<f64>, pub priority: i64 }
/// Which of `open` a Session takes; an earlier entry wins every tie. `None` when empty.
pub fn choose_route(strategy: PoolStrategy, open: &[RouteStanding], unknown_use: u16) -> Option<usize>;
pub trait AccountLogin: Send + Sync {
    fn login(&self, pool: &AccountPool, account: &Account, home: &Path, credential: &str) -> Result<(), String>;
}
// directorates::q::sealing (feature `daemon`): values sealed to named Stations. X25519 sealed boxes
// (crypto_box). What is sealed names its recipients, so a record edited in the Registry redirects nothing.
pub const SEALING_KEY_FILE: &str;   // "sealing.key", beside identity.pem
pub struct SealingKey;   // ::generate() ::open_or_create(path) .public_key(); Debug shows the public half
pub struct SealingPublicKey;   // .to_text() ::parse(text): base64
pub enum VaultError { NotFound, NotNamed, Unreadable, NoKey(String), Unverified(String), Sealing, Conflict { held: Option<u64> }, Registry(io::Error) }
pub struct Recipient;   // Recipient::named(station) .pinned(fingerprint): its key must be signed by that identity
pub struct Opened { pub value: Zeroizing<String>, pub revision: u64, pub stations: BTreeSet<String>, .. }
pub struct Vault;   // Vault::new(registry, station, Arc<SealingKey>)
impl Vault { pub fn station(&self) -> &str; pub fn registry(&self) -> &dyn RegistryBackend;
    pub fn open(&self, name: &str) -> Result<Opened, VaultError>;
    pub fn seal_to(&self, name, value, stations: &[Recipient], expect: SealedExpect, now) -> Result<u64, VaultError>;   // caller need not be named
    pub fn replace(&self, name, value, expect: u64, now) -> Result<u64, VaultError>;       // holder only, to the Stations sealed inside
    pub fn add_station(&self, name, station: &Recipient, now) -> Result<u64, VaultError>;  // holder only
    pub fn remove_station(&self, name, station: &str, now) -> Result<u64, VaultError>; }   // holder only; what it opened stays with it
pub struct RegistrySecrets;   // ::new(Arc<Vault>): SecretProvider `registry`, read answers a revision, store refuses another with GUATIAO_ERR_BUSY
// directorates::q::logins (feature `daemon`): harness logins kept sealed. Only files decide: a copy that
// lost its tokens or ends sooner is never kept, and exit status proves nothing (Grok exits 0 when refused).
pub fn sealed_name(account: &AccountRef) -> String;   // "login/<pool>/<account>"
pub fn add_login(vault, account, dir: &Path, files: &[String], stations: &[Recipient], now) -> Result<u64, LoginError>;   // reads dir, removes nothing
pub struct LoginFiles;   // ::read(home, files, largest) .write(home) .get(file) .expires_at(); Debug shows paths; wiped on drop
pub struct SessionLogin { pub account, pub files, pub home, pub revision, pub started, .. }   // kept beside the home as <home>.login.json
impl SessionLogin { pub fn materialize(vault, account, files, home) -> Result<Self, LoginError>;
    pub fn recall(home) -> Option<Self>; pub fn remove(&self) -> io::Result<()>; pub fn harness_starts(&mut self);
    pub fn sync(&mut self, vault, registry_revision: Option<u64>, now) -> Result<Synced, LoginError>; }
pub enum Synced { Unchanged, Renewed(u64), Rewritten(u64), Restored(u64, &'static str) }
pub trait LoginRenewer: Send + Sync { fn renew(&self, account: &Account, home: &Path, timeout: Duration) -> RenewEnd; }
pub struct CommandRenewer;   // HOME and USERPROFILE at home, renew_env, PATH and what Windows needs; output never read
pub fn renewal_due(login: &LoginState, limits: &station::limits::AccountLimits, now: f64) -> bool;
pub fn login_accounts(pools) -> Vec<(AccountRef, &Account)>;
pub fn assign_renewals(registry, logins, headquarters: &str, live: &[String], limits, now) -> io::Result<Vec<LoginNotice>>;   // one lease, one write
pub fn run_leases(vault, logins, renewer: &dyn LoginRenewer, scratch: &Path, limits, now) -> io::Result<Vec<LoginNotice>>;   // this Station's leases
pub struct LoginNotice { pub account: AccountRef, pub kind: NoticeKind, pub text: String }   // Leased(station), Renewed, Unchanged, Failed, NeedsPerson, MayLapse
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
pub struct SessionEnv;   // env = { NAME = "value" } on a record; From<BTreeMap<String, String>>, values(); ToValue, FromValue, Schema, Deserialize
pub struct LoadoutCatalog;   // Default: empty
impl LoadoutCatalog {
    pub fn load(path: impl AsRef<Path>, limits: &ConfigLimits) -> Result<Self, QError>;   // TooLarge past loadout_bytes
    pub fn parse(text: &str, limits: &ConfigLimits) -> Result<Self, QError>;   // FromStr: the default limits
    pub fn with_pool(self, pool: AccountPool) -> Result<Self, QError>;   // validates; replaces the same ID
    pub fn check_session_env(&self, env: &SessionEnv, chars: Limit) -> Result<(), String>;   // refuses a credential's, secret's, pool account's or AI6_ variable
    pub fn pools(&self) -> impl Iterator<Item = &AccountPool>;
    pub fn pool(&self, id: &str) -> Option<&AccountPool>;
    pub fn for_agent(&self, agent: &str) -> Result<&Loadout, QError>;
    pub fn sandbox_for(&self, loadout: &Loadout) -> Option<&SandboxProfile>;
    pub fn servers_for(&self, loadout: &Loadout) -> Result<Vec<&McpServer>, QError>;
    pub fn loadouts(&self) -> impl Iterator<Item = &Loadout>;
    pub fn sandboxes(&self) -> impl Iterator<Item = &SandboxProfile>;
    pub fn mcp_servers(&self) -> impl Iterator<Item = &McpServer>;
    pub fn overlays(&self) -> &BTreeMap<String, OverlayRecord>;   // by identity
    pub fn overlay(&self, name: &str) -> Option<&OverlayRecord>;  // name compared as an identity
    pub fn secret(&self, name: &str) -> SecretDefinition;   // its [secret.<name>], or every default
    pub fn secrets(&self) -> Vec<SecretDefinition>;         // every name a loadout, table or pool account names
    pub fn secret_providers(&self) -> &BTreeMap<String, SecretInstance>;  // [secret_provider.<name>]
    pub fn provider_of<'a>(&'a self, provider: &'a str) -> &'a str;       // an instance's provider, else itself
}
impl FromStr for LoadoutCatalog { type Err = QError; }
pub struct SecretDefinition;
impl SecretDefinition {
    pub fn new(name: impl Into<String>) -> Self;  // env = NAME upper, -/. as _; source: file provider, key <name>
    pub fn name(&self) -> &str; pub fn env(&self) -> &str;
    pub fn green_ink(&self) -> bool; pub fn kinds(&self) -> &[String];   // empty: any approval kind
    pub fn targets(&self) -> &[String];                   // globs on the approval's target; empty: any
    pub fn unlocked_by(&self, kind: &str, target: &str) -> bool;
    pub fn green_ink_weakening(&self) -> Option<String>;  // Some for green ink without targets
    pub fn source(&self) -> &SecretSource;
}
pub struct SecretInstance;   // name(), provider() (a loaded provider, never another instance), settings() -> &SecretFields
pub struct SecretSource { pub provider: String, pub key: String, pub fields: SecretFields }  // Debug: names only
impl SecretSource { pub fn new(provider: impl Into<String>, key: impl Into<String>) -> Self; }
pub struct SecretFields;     // a JSON map; Debug shows names only; every text zeroed on drop
impl SecretFields { pub fn new(serde_json::Map<..>) -> Self; pub fn get(&self, name) -> Option<&serde_json::Value>;
    pub fn names(&self) -> impl Iterator<Item = &str>; pub fn is_empty(&self) -> bool;
    pub fn as_json(&self) -> &serde_json::Map<..>; pub fn to_map_in(&self, Alloc) -> Result<guatiao::Map, ProviderError>; }
```

### Secret providers (`ai6::directorates::q::secret_kind`)

A secret is read from a Guatiao provider of kind `ai6_secret`. `file`, `env` and `inline` are built
in (`secret_kind::builtin`, registered by `Plugins` before any scan, so a library offering one of
their IDs is skipped); others load through `[station.plugins]` like any provider. Slot order is the
ABI; optional slots answer `GUATIAO_ERR_UNSUPPORTED` by default.

```rust
pub const RESERVED_FIELDS: [&str; 7];  // provider, key, secret, env, green_ink, kinds, targets
pub const BUILT_IN: [&str; 3];         // file, env, inline
#[guatiao::kind(name = "ai6_secret")]
pub trait SecretProvider: Send + Sync {
    #[sensitive] fn read(&self, ask: &Value) -> Result<Value, ProviderError>;   // SecretAsk -> SecretRead
    fn fields(&self) -> Result<Value, ProviderError>;   // JSON Schema of a secret's further fields; default none
    #[sensitive] fn lease(&self, ask: &Value) -> Result<Value, ProviderError>;  // SecretLeaseAsk -> SecretLeased
    fn revoke(&self, lease: &str) -> Result<(), ProviderError>;
    #[sensitive] fn store(&self, request: &Value) -> Result<Value, ProviderError>; // SecretStore -> SecretStored;
}                                       // GUATIAO_ERR_BUSY when it holds a revision other than `expect`
// models (ToValue, FromValue, Schema; values are guatiao::Secret<String>, built in Alloc::wiping()):
pub struct SecretAsk { key: String, fields: Map }
pub struct SecretRead { value: Secret<String>, revision: Option<String> }
pub struct SecretLeaseAsk { key: String, fields: Map, ttl_secs: f64, station: String, holder: String }
pub struct SecretLeased { lease: String, value: Secret<String> }
pub struct SecretStore { key: String, fields: Map, value: Secret<String>, expect: Option<String> }  // expect None: no check
pub struct SecretStored { revision: Option<String> }
pub struct SecretKeeper;   // a built provider with typed calls
impl SecretKeeper {
    pub fn new(provider: Box<dyn SecretProvider>) -> Result<Self, String>;  // refuses a RESERVED_FIELDS field
    pub fn fields(&self) -> &[String];
    pub fn check_fields(&self, fields: &SecretFields) -> Result<(), String>; // names an undeclared field
    pub fn read(&self, key: &str, fields: &SecretFields) -> Result<SecretRead, ProviderError>;
    pub fn lease(&self, key, fields, ttl_secs: f64, station, holder) -> Result<Option<SecretLeased>, ProviderError>; // None: does not lease
    pub fn revoke(&self, lease: &str) -> Result<(), ProviderError>;
    pub fn store(&self, key, fields, value: &str, expect: Option<&str>) -> Result<SecretStored, SecretStoreError>;
}
pub enum SecretStoreError { Conflict, NotOffered, Failed(ProviderError) }   // #[non_exhaustive]
// builtin: FileSecrets (config FileSecretsConfig { dir: Option<PathBuf> }; key a path, relative to
// dir, one trailing newline dropped), EnvSecrets (key a variable), InlineSecrets (field `value`);
// none takes another field; builtin::library is their local_providers! entry.
```

The Station builds one provider per `[secret_provider.<name>]` instance, or per provider named
directly with its default settings (the `file` provider's `dir` is the Station's secrets directory),
and keeps it while its definition is unchanged. An instance's setting its provider's config schema
marks `x-sensitive` is a `SecretRef`, read before the instance is built; one leading back to itself
is refused. An instance may not take a loaded provider's ID. A provider's own error text is never
shown: the broker answers `SecretError::Backend`, and a build refusal says only which provider.

Loadouts use strict `[loadout.<agent>]`, `[sandbox.<name>]`, `[mcp.<name>]`,
`[secret.<name>]` (`env`, `green_ink`, `kinds` and `targets` (globs, only with `green_ink`),
`provider` (an instance or a loaded provider; default `file`), `key` (default the name), and that
provider's fields flat beside them, refused for a built-in that does not take them), `[secret_provider.<name>]`
(`provider`, a loaded provider, and its settings flat; a built-in's name is refused) and `[pool.<id>]` (`handler`, `operations`, `strategy`,
`login_command`, accounts `[pool.<id>.account.<name>]` with `kind`, `secret`, `dir`, `env`,
`priority`, `max_concurrent`, `labels`) and `[overlay.<name>]` (exactly one of `source`, an
AOS source string, and `path`; `pin`, a `sha256:` digest; `secrets`, names) TOML tables; a
loadout's `overlays` names records of the same document, each identity once. A document needs a
loadout, a pool, a secret or an overlay. Parsing is bounded to one MiB, denies unknown
fields and invalid or duplicate names, and validates references before returning a catalog.
Errors distinguish I/O, size, encoding, TOML syntax and schema failures; diagnostics expose
source positions but never document contents. The catalog is inert: it does not expand
environment variables, resolve paths, launch MCP servers or create sandboxes. A Station
loads `loadouts.toml` from its configuration directory (or `StationConfig::with_loadouts`)
and applies only the secrets: see "Credentials" under the Station daemon.

## Overlays (`ai6::directorates::q::overlays`)

Overlays in the Agent Overlay Spec's format (AOS `0.1.0-draft`, vectors at commit `061b8f5`
in `tests/fixtures/aos-vectors/`): reading, validating, digesting, resolving and composing,
and the environment, setup and dependencies an overlay asks for, as pure functions. Nothing
here runs a script, installs a package, starts a server or reads the network: a Station's own
`station::overlays` (not public) fetches overlays to its store under `<state_dir>/overlays`,
places them on a Theater and prepares them, and hands the handler `AgentStart::overlays`. The
Store class (catalogs, icons) is not implemented.

```rust
pub const MANIFEST: &str = "overlay.cfg";
pub const LATER_MANIFEST: &str = "OVERLAY.md";   // read_overlay refuses a tree with it and no overlay.cfg
pub fn hooks_asked(manifest: &Document, files: &OverlayTree) -> Option<String>;   // [hooks], or a root hook.<ext>: read_overlay refuses it
#[non_exhaustive] pub enum OverlayFrom { Source(Source), Path(PathBuf) }
pub struct OverlayRecord;   // [overlay.<name>]
impl OverlayRecord {
    pub fn new(name, from: OverlayFrom) -> Self; pub fn pinned(self, Digest) -> Self; pub fn with_secrets(self, Vec<String>) -> Self;
    pub fn name(&self) -> &str; pub fn identity(&self) -> String; pub fn from(&self) -> &OverlayFrom;
    pub fn pin(&self) -> Option<Digest>; pub fn secrets(&self) -> &[String];
}
pub struct ReadOverlay { pub digest: TreeDigest, pub manifest: Document, pub issues: Vec<Issue>,
    pub skills: Vec<(String, Result<(), String>)> }   // tree(), usable()
pub fn read_overlay(root: &Path, limits: &OverlayLimits) -> Result<ReadOverlay, String>;   // `ai6 overlay pin DIR`
#[non_exhaustive] pub enum OverlayList { Loadout, Operation }
#[non_exhaustive] pub enum OverlayRefusal { InvalidName { list, name }, Unknown { list, name },
    Missing { by, name }, Cycle(Vec<String>), Unpinned(String) }   // Display, Error
pub struct IssuedOverlay { pub identity: String, pub record: OverlayRecord, pub manifest: Document }
pub struct IssuedOverlays { pub install_order: Vec<String>, pub overlays: Vec<IssuedOverlay>, pub weakenings: Vec<Weakening> }
pub fn issue_overlays(loadout: &[String], operation: &[String],
    records: &BTreeMap<String, OverlayRecord>, manifests: &BTreeMap<String, Document>,
    allow_unpinned: bool) -> Result<IssuedOverlays, OverlayRefusal>;

// AOS
pub type Document = serde_json::Map<String, serde_json::Value>;   // the data model
pub struct ReadLimits { pub bytes: Limit /* 1 MiB */, pub depth: Limit /* 64 */ }
pub struct ReadError;   // class() (AOS error class), line(); Display names no document text
pub fn read_document(data: &[u8], limits: &ReadLimits) -> Result<Document, ReadError>;
pub fn parse_dotenv(data: &[u8]) -> Result<Vec<DotenvLine>, ReadError>;
pub fn read_dotenv(data: &[u8], env: &mut Environment) -> Result<Vec<(String, String)>, ReadError>;
pub fn read_env_script_output(data: &[u8]) -> Result<ScriptOutput, ReadError>;
pub fn substitute(text: &str, lookup: &Lookup<'_>) -> Result<String, SubstitutionError>;
pub fn valid_overlay_name(&str) -> bool; pub fn normalize_name(&str) -> String;
pub fn root_variable(identity: &str) -> String; pub fn state_variable(identity: &str) -> String;
pub fn valid_relative_path(&str) -> bool; pub fn is_reserved_variable(&str) -> bool;
pub fn is_sensitive_variable(&str) -> bool; pub fn os_matches(Option<&[String]>, &str) -> bool; pub fn current_os() -> &'static str;
pub struct Source { pub kind: SourceKind, pub location, pub reference: Option<String>, pub path: Option<String> }   // parse(), shown()
pub struct Digest;   // sha256: and 64 hex; of(bytes), parse(text), bytes(), Display lowercase
pub struct TreeLimits { pub files: Limit, pub bytes: Limit, pub file_bytes: Limit }
pub struct TreeDigest { pub digest: Digest, pub files: BTreeMap<String, Digest> }   // lines()
pub fn tree_digest(root: &Path, limits: &TreeLimits) -> Result<TreeDigest, DigestError>;
pub fn digest_entries(entries) -> Result<TreeDigest, DigestError>;   // abstract (components, TreeEntry)
pub struct OverlayTree;   // from_paths, from_texts; paths(), contains(), text()
pub fn validate(manifest: &Document, tree: Option<&OverlayTree>) -> Vec<Issue>;   // Issue { at, code }
pub fn applies(issues: &[Issue]) -> bool;   // only problems under /about
pub fn resolve(..) -> Result<Resolution, Cycle>; pub fn composition_order(..) -> Vec<String>;
pub fn compose(overlays, requested, trees) -> Result<Composition, Cycle>;
pub fn environments(overlays, active, trees, os, removed) -> Result<Environments, String>;
pub fn effective(manifest: &Document, facts: &Facts) -> Result<Effective, Refusal>;
pub fn composed_environment(active: &[(&Document, Facts)], base: Environment, &mut Vec<Weakening>) -> Result<Environment, Refusal>;
pub fn setup_phase(Change, recorded: Option<&str>, digest: Option<&str>) -> Option<Phase>;
pub fn preparation(Change, Option<&SetupRecord>, digest, setup, dependencies, fails) -> Preparation;
pub fn check_skill(dir: &Path, name: &str, allow_tools: bool, tools_setting: &str, file_bytes: Limit) -> Result<(), String>;
pub const SKILL_INERT_KEYS: [&str; 12];
```

`read_document` drops one byte-order mark, takes the format from a `#!` first line (its last
word, after the last `/` or `\`, leading dots dropped, ASCII lowercased) or else TOML, and
parses through `anycfg` with codecs of its own over guatiao's `Value`: TOML refuses a date,
JSON a duplicate key, a lone surrogate, `NaN` and comments, YAML (from `granit-parser`
events) any tag, anchor, alias, merge key, directive, second document and U+0085/U+2028/U+2029,
typing plain scalars by the 1.2 core schema. The value is checked against the data model:
no null, finite floats, signed 64-bit integers, `30.0` kept a float, within `depth`. Every
error carries an AOS class (`parse-error`, `duplicate-key`, `yaml-tag`, `too-deep`, ...).

`OverlayTree::with_text(path, text)` adds a file's text; `skill_description(dir)` reads a `SKILL.md`'s
`description`; `check_requirements(text, locked)` refuses an option, a direct URL, or in a lock a
requirement not pinned with `==`. `IssuedDigest { overlay, digest }` is what a launch records.

`issue_overlays` refuses a name that is no overlay name or no record's (`InvalidName`,
`Unknown`, naming the list), resolves `requires` over the records whose manifests are given
(a cycle or a missing requirement refuses), orders by `(priority, name, identity)`, and
refuses an overlay without a pin unless `allow_unpinned`, which reports `unpinned` for it.

`Environment`'s `Debug` lists names only, and `Facts`' omits grants, secrets and files.

Secret references can be leased through `SecretBroker`:

```rust
pub struct SecretLeaseRequest;
impl SecretLeaseRequest {
    /// InvalidTtl for zero, or a lifetime past what this process's clock can reach.
    pub fn with_ttl(station: impl Into<String>, mission_id: impl Into<String>, ttl: Duration)
        -> Result<Self, SecretError>;
    pub fn station(&self) -> &str;
    pub fn mission_id(&self) -> &str;
    pub const fn ttl(&self) -> Duration;
}
pub type SecretProviderLookup = dyn Fn(&str) -> Result<Arc<SecretKeeper>, String> + Send + Sync;
pub struct SecretBroker;
impl SecretBroker {
    /// Each secret as catalog.secret(name) defines it, from the provider `providers` finds
    /// (an instance or a loaded provider); no ceiling on a lease.
    pub fn new(catalog: Arc<LoadoutCatalog>,
        providers: impl Fn(&str) -> Result<Arc<SecretKeeper>, String> + Send + Sync + 'static) -> Self;
    pub fn with_max_ttl(self, max_ttl: TimeLimit) -> Self;        // a longer request is InvalidTtl
    pub fn lease(&self, loadout: &Loadout, request: SecretLeaseRequest)
        -> Result<SecretLease, SecretError>;
    /// Some of the loadout's names; an undeclared one is AccessDenied and nothing is acquired.
    /// A provider that leases is asked for the request's time, any other is read; a failure
    /// revokes what was leased. Any provider failure is Backend; an empty value InvalidResponse.
    pub fn lease_names(&self, loadout: &Loadout, names: &[String], request: SecretLeaseRequest)
        -> Result<SecretLease, SecretError>;
}
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
    pub fn revoke(&self) -> Result<(), SecretError>;   // each provider lease; a failed one is retried next call
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

`CableScope` defines Agent unicast and Station, Theater, global, named-channel or Operation
(serde `operation`: that Operation's Agents and those of every Operation under it) broadcast.
`CableTarget::try_new(scope, name)` requires a name except for global.
`CableRecipient::try_new(agent, session_id, state)` validates the relay-resolved
recipient. `CableDraft::new(sender_agent, sender_operation, sender_station, target, body,
recipients, idempotency_key, id, created_at, expires_at)` creates the required fields;
builder methods add the optional Theater, Mission, Session, conversation and reply
identities. `Cable::try_new(draft, &CableLimits)` validates a complete envelope. Bodies are
limited to `body_bytes` UTF-8 (16 KiB), expiry must be later than creation and no more than
`max_ttl` later (24 hours), and recipient `(agent, session_id)` pairs must be unique; a
Cable read back from its record is not held to them again. Serde uses the Registry's field names
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
(`limits()` gives that Registry's `RegistryLimits`)
uses the shared Registry, an authoritative Agent/Session directory and named channel
members. `with_limits` configures bounded fan-out (at least 1) and persistent sender/scope
token buckets; `with_message_limits(CableConfig::message_limits())` the body size, longest and
default lifetime a sender gets (16 KiB, a day and 15 minutes by default), which
`body_bytes()` reports; the Registry's `[registry.limits.cables]` bound what it stores.
`CableSendRequest::new` asks for the relay's default lifetime, `with_ttl` for another. `send(principal, CableSendRequest)` checks sender and every receiver,
deduplicates the audience, snapshots recipient Sessions, applies deliver/hold/refuse and
stores an idempotent Cable. Same-Station unicast uses Station grants; cross-Station
unicast uses explicit Agent grants. Theater, global and channel broadcasts require exact
scope grants; an Operation broadcast needs a Send grant and each receiver a Receive grant on
that Operation or on one it is subordinate to. `CableRelay::with_lineage(impl Fn(&str) ->
Vec<String>)` gives the relay the tree (an Operation's ancestors, nearest first; the daemon
reads its live roster); without it an Operation audience is that Operation alone. No grant is
implied. An assistant acting for a person still reaches its own Operation only. `discover` exposes only same-Station peers allowed by both sides' grants.
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

`CableRelay::with_durable_audit(write: impl Fn(&CableAuditRecord) -> Result<(), String>)`
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

Liaison (Universal Exports' A2A) sends for a peer outside AI6 with
`CablePrincipal::with_peer(proof) -> Result<Self, CableError>` (1-256 characters; `peer()`
reads it), its sender `a2a:<peer>` (`LIAISON_PREFIX`) and its Station `LIAISON_STATION`
(`"liaison"`). The Cable carries it (`CableDraft::with_peer`, `Cable::peer()`, serde field
`peer`, omitted when `None`; `CableDelivery::peer`, `CableAuditRecord::peer`). A reply to a peer's
Cable reaches the sending Session as the Cable stamped it, wherever it runs, its `session_id`
required as for one sent for a person; the grants still decide. A send whose Agent target names a
peer (`a2a:<peer>` or `a2a:<peer>/<addressee>`, `liaison_peer(name) -> Option<(&str,
Option<&str>)>` splits it) and matches no endpoint is addressed to `a2a:<peer>` with no Session,
waiting in the Registry for Liaison; a principal with a peer cannot send to one. A grant whose
Agent target is `a2a:<peer>` also covers `a2a:<peer>/<addressee>`.
`CableSendRequest::with_affinity(session)` binds a Cable to the provider Session its thread is
with (`Cable::affinity()`, serde field `affinity`): `inbox` gives it to that Session only, or,
once the Registry has that Session `Ended` or `Lost`, to a Session of the Agent whose record
`opened_at` is later than its `closed_at`; a Session the Registry cannot tell about keeps it. A
peer's Cable addressed to the Agent as a whole records the Session that took it
(`CableRecipient::taken_by()`, serde field `taken_by`), and only that Session sees, acknowledges or
answers it after. `withdraw(principal, cable_id) -> Result<Cable, CableRelayError>` lets the
sending principal (Agent and Session) take back a Cable no recipient has had: queued and held
recipients become `CableState::Withdrawn`, which is final; `Unavailable` once any recipient has it.

`CableRelay::with_residents(impl Fn(&CableTarget) -> Vec<CableEndpoint>)` says where a Cable waits
when its addressee has no live Session: for an `agent` target none of whose endpoints is live, or an
`operation` target with no endpoint, the relay adds what it returns (endpoints with no Session; the
daemon gives the Agent in each Operation it serves, or the Operation's 00 Agents), and the Cable is
addressed to the Agent as a whole, queued; without it such a send is `Unavailable`. Grants and
compartments are applied to those endpoints as to live ones. `deliver_new(principal, limit, permit)`
reads as `inbox` does for what is newly queued only (what was delivered is not returned again), asks
`permit` once before the first delivery and delivers nothing when it says no: a brief reads the
Cables that waited for its Agent's next Session with a principal that has no Session (a Cable with an
`affinity` is not given to it). `wake(principal, limit, wakes_per_hour)` is `deliver_new` for a Session
between turns, its permit taking one wake of `wake:<agent>` (`wakes_per_hour` an hour, burst
`wakes_per_hour`) and one token of `scope:agent:<agent>` through `take_quotas`; `0` delivers nothing.
Both recheck the receiver's grants, compartments and inbound mode first, as `inbox` does. The daemon
calls `wake` each beat for every live Session of an Agent with `wake_on_cable` whose Mission is
`blocked` with no reason, and sends the deliveries as that Session's message (which turns the Mission
`working`); such a Session is also a Cable endpoint, as a `working` one is. It fills the brief of a
Mission start or resume (a standing window's too) through `deliver_new`, when the Session has the
Cable tools. Neither creates a Mission, a brief or a placement.

The daemon reads strict `cables.toml` from its config directory. Missing configuration
denies every action. `[[grants]]` entries name `agent`, `action`, `scope`, and optional
`name`; `[inbound]` selects `deliver`, `hold`, or `refuse` by Agent; `[channels]` maps
channel names to Agent lists. `max_fanout`, `sender_rate`, `sender_burst`, `scope_rate`,
and `scope_burst` bound routing. `ServeConfig::with_station_id` supplies
the Station identity; the default is `<host>-<random>`, created once and kept in `station-id`
in the AI6 state directory so a restarted daemon stays the same Station.

`CableStore::submit_cable(&cable)` stores (in a `FileRegistry`) the envelope under `cables/` with atomic
replacement and the same `.ai6-registry.lock` used by Registry claims. A retry with the
same sender and idempotency key returns the stored record when immutable request content
matches; conflicting content returns `AlreadyExists`. `find_cable(sender, key)` and
`get_cable(id)` read accepted records. `cable_inbox(agent, session_id, limit, now)` lists
unexpired queued/delivered records for an Agent Session, sorted by creation time and ID;
`limit` is 1–1,000 and `now` is optional Unix time for deterministic callers.
`update_cable_delivery(id, agent, session_id, state, taken_by, now)` serializes recipient state
changes under the Registry lock and automatically expires overdue deliveries; `taken_by` records
the Session that took a delivery addressed to the Agent as a whole
(`Cable::with_recipient_taken`). `cables_from(sender_agent, limit, now)` lists the unexpired
Cables one sender sent, oldest first.
`held_cables(limit, now)` lists unexpired envelopes containing at least one held recipient
for the scoped relay to authorize.
`take_quotas(buckets, now)` atomically consumes sender and scope token buckets from
`cable-quotas.json`; each bucket is `(key, rate_per_second, burst_capacity)`, and its key
is SHA-256 hashed before it is stored. The file backend bounds scans to 10,000 Cable files
and records to 2 MiB; `SqlRegistry` keeps Cables in `ai6_cables` and the buckets, keyed the
same way, in `ai6_cable_quotas`, with the same bounds.

`ai6::directorates::section_00::agent` defines `AgentStart`, `AgentResume` and the `AgentHandler`
Guatiao kind `ai6_agent_handler`. The Session a handler returns is the shared agent session kind of
[`coqui-acp-provider`](https://github.com/Boriken-Dev/coqui-acp) (object kind `agent_session`,
pinned by revision with `coqui-acp`, on the same Guatiao revision): `AgentSession`, `AgentMessage`,
`AgentEvent`, `UsageLimit`, `TokenUsage`, `QuotaReport`, `LIMIT_EVENT` and `DENIED_EVENT` are
re-exported at the crate root as `ai6::AgentSession` and so on; the event kinds `cancelled`
(`CANCELLED_EVENT`) and `TURN_CANCELLED` are that crate's alone. `AgentSession::session_id`
answers `Result<Option<String>, ProviderError>`: the Station checks the ID as a `SessionId` where
it records it, and an ID that is not one fails the Session (`the provider reported a Session ID AI6
cannot record: ...`). Handlers own harness launch, initial-context delivery, resume,
message submission and event decoding. AI6 supplies an `AgentTerminal` object bridge
over its selected `Terminal` provider. `AgentHandler::stdio(&self) -> Result<HarnessStdio,
ProviderError>` is an appended slot (default `Ok(HarnessStdio::Terminal)`): a handler whose
harness speaks JSON over standard input and output answers `Json` (the `claude` and `codex`
handlers do), and the Station gives it a provider without a terminal.
`AgentHandler::models(&self, environment: BTreeMap<String, String>) -> Result<ModelListing, ProviderError>`
lists the models the account behind `environment` may use with no model turn; the default answers
`ModelListing::unknown("this handler has no way to list models")`, and a handler that cannot never
guesses. `ModelListing { known: bool, unknown_because: Option<String>, models: Vec<ModelInfo { id,
name: Option<String>, description: Option<String> }> }` (`ModelListing::unknown(reason)`,
`::of(models)`). `section_00::listing::ask(program, args, environment, wait, |harness| ...)` runs a
harness that speaks JSON lines with exactly `environment`, kills it afterwards and returns what the
closure reads (`Harness::send`, `Harness::next_where`); no credential of `environment` is ever part
of a listing. The Station asks it for each pool account (the account's credential leased as for a
Session, no claim made, the lease revoked after) and for each route no pool serves (its env and
secrets), every `[accounts] models_every` seconds (3600; 0 never on its own), on a thread of its
own beside the check-in beat, asking `[accounts] model_probes` (`Limit`, 4; 0 none) at once and
waiting `model_probe_timeout` (`TimeLimit`, 20 s) for each; one that takes longer keeps the last
answer for its source, or is unknown. It keeps the answers
(`station::server::models::ModelBoard`) and reports them at check-in under `models` (`Found
{ source, pool, account, agent, route, handler, listing, at }`). Universal Exports serves them at
`GET /v1/user/models` (`roster.read`), `ai6ctl models` prints them, and the Pools page shows them.
`AgentHandler::clearance_refusal(&self, clearance: String) -> Result<Option<String>, ProviderError>`
says why the handler, with its settings, would refuse a Mission's Session at that clearance; a route
that would be refused is passed over (default: `None`).
`AgentHandler::assistant_scripts(&self, environment: BTreeMap<String, String>) ->
Result<Option<String>, ProviderError>` is another appended slot: `None` when the harness,
started with `environment`, can run an assistant's interpreters inside its own sandbox on this
Station, else why not; the default answers that it cannot. Claude's answers `None` on macOS,
and on Linux when `bwrap` and `socat` are on the environment's `PATH` (else the Station's);
Codex's runs `codex sandbox -P` once in 15 minutes. `AgentHandler::assistant_skills(&self) ->
Result<Option<String>, ProviderError>`, a further appended slot: `None` when the harness loads
exactly `AssistantMode::skills` and no other skill, else why not (default: it cannot; Claude
answers `None`, Codex why not); a Station installs no skill for a handler that cannot.
`AgentHandler::weakenings(&self) -> Result<Vec<String>, ProviderError>`, a further appended slot:
each way this handler's settings are looser than its default, one line each (default: none). The
Station prefixes each with `Agent "ID" is weakened: `, logs them at start and on a roster change
and publishes them at check-in as `agents_weakened`, leaving out an Agent whose handler it cannot
build; a field runner logs them at each launch.
`AgentStart`, `AgentResume`, `AgentMessage`,
`AgentEvent`, `UsageLimit` and `TokenUsage` are also serde `Serialize` and `Deserialize`
(field names as declared): a runner's API carries them as JSON.

```rust
// coqui_acp_provider's event contract; consumers match `kind` as text.
pub const LIMIT_EVENT: &str = "limit";
pub const DENIED_EVENT: &str = "denied";
// coqui_acp_provider::CANCELLED_EVENT = "cancelled": the turn stopped because of `interrupt`, or
// the agent gave it up. The Station counts it as ending the turn; a Mission's Session that ends
// one by itself leaves the Mission `Blocked`, and Moneypenny's answer ends with a notice.
// coqui_acp_provider::TURN_CANCELLED = "the agent cancelled the turn": the text of a `blocked`
// event that means `cancelled`, read the same way.
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
// AgentStart and AgentResume carry `overlays: SessionOverlays` (default empty), what a Session's
// overlays hand its harness beside their environment and `initial_context`:
pub struct SessionOverlays { pub roots: Vec<String>, pub skills: Vec<OverlaySkill>, pub plugin_dir: Option<String> }
pub struct OverlaySkill { pub name: String, pub overlay: String, pub description: String,
    pub path: String /* absolute, on the harness's host */, pub native: bool /* may load natively */ }
impl SessionOverlays {
    pub fn routing(&self, loads_skills: bool) -> Option<String>;       // lines naming each SKILL.md it does not load
    pub fn with_routing(&self, context: &[String], loads_skills: bool) -> Vec<String>;
}
// `plugin_dir` lays the `native` skills out as `skills/<name>/` and `.claude-plugin/plugin.json` naming
// the plugin `ai6`. A handler that does not load skills calls `with_routing(.., false)`.
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
// Both carry `clearance: String`, the Agent's effective clearance at that launch (on
// AgentResume serde default empty, which counts as `desk`). A Station passes `desk` for a Chief of
// Staff review and for Moneypenny, whose limits are the two above.
pub fn clearance_runs_commands(clearance: &str) -> bool; // field and 00 true; desk and anything else
                                                         // false. A handler keeps commands from such a
                                                         // Session, or refuses to start it.
// AgentResume also carries `initial_context: Vec<String>` (serde default empty): the standing
// part of the start-up context (the Agent's context, its role's purpose and policy; for
// Moneypenny her instructions, never the conversation recap), which Claude Code is given again
// as `--append-system-prompt` (2.1.310 reuses the prompt it recorded with the Session until
// the Session is compacted, then builds it from the resuming launch); Codex keeps a thread's
// instructions and ignores it.
pub struct AssistantMode { pub mcp_servers: Vec<McpServerRef>,
                           pub interpreters: Vec<String> /* serde default */,
                           pub unsandboxed: bool /* serde default; scripts without the sandbox, a weakening */,
                           pub skills: Vec<String> /* serde default */,
                           pub skills_dir: Option<String> /* serde default; absolute; required with skills */,
                           pub skill_commands: bool /* serde default; false: injected commands off */,
                           pub network: AssistantNetwork /* serde default */,
                           pub files: Vec<String> /* serde default; directories it may write */,
                           pub read_only: Vec<String> /* serde default; relative paths scripts may not write */,
                           pub context_file: Option<String> /* serde default; absolute, outside the working directory */ }
pub struct McpServerRef { pub name: String /* moneypenny */,
                          pub env_prefix: String /* AI6_MONEYPENNY_MCP: <prefix>_URL, <prefix>_TOKEN */ }
pub struct AssistantNetwork { pub policy: String /* limited (default) | denied | open */,
                              pub allowed_hosts: Vec<String>,
                              pub denied: Vec<String> /* serde default; denied::DenyEntry
                                                         strings; empty denies nothing */ }
// Default: limited, no host, nothing denied. `open` reaches everything the Station reaches, the
// Station's own machine included; only `denied` takes anything out.

// ai6::directorates::section_00::denied
pub const DENY_CATEGORIES: [&str; 4];   // loopback, private, link-local, metadata
pub enum DenyEntry { Host(String), Net(IpAddr, u8), Category(&'static str) }
impl DenyEntry { pub fn parse(entry: &str) -> Result<Self, String>;  // category | CIDR | address
                                                                       // ([v6] too) | host pattern
                 pub fn is_range(&self) -> bool; }  // a category but metadata, or a network > 1 address
pub struct DenyList;  // Default: empty
impl DenyList { pub fn parse<S: AsRef<str>>(entries: &[S]) -> Result<Self, String>;
                pub fn is_empty(&self) -> bool; pub fn entries(&self) -> &[DenyEntry];
                pub fn denies_name(&self, host: &str) -> bool;       // a name, or an address
                pub fn denies_address(&self, address: IpAddr) -> bool; // v4-mapped/compatible as v4
                pub fn names_and_addresses(&self) -> Vec<String>;    // categories expanded; [v6]
                pub fn ranges(&self) -> Vec<&DenyEntry>; }
// loopback: 127.0.0.0/8, 0.0.0.0, ::1, ::, localhost, *.localhost; private: 10/8, 172.16/12,
// 192.168/16, 100.64/10, fc00::/7; link-local: 169.254/16, fe80::/10; metadata: 169.254.169.254,
// fd00:ec2::254, metadata.google.internal.

// ai6::directorates::section_00::programs: where a harness's programs are installed
pub fn program_dirs(programs: &[&str], cwd: &Path, environment: &BTreeMap<String, String>)
    -> Vec<PathBuf>;   // install dirs on the environment's PATH, links followed; leaves out `/`
                       // and any dir that is/holds HOME, cwd, cwd's parent or a credential dir
pub fn install_dir(path: &Path) -> PathBuf;   // its dir, or the one above when that is `bin`
pub fn find_program(program: &str, environment: &BTreeMap<String, String>) -> Option<PathBuf>;
pub fn missing_program(agent: &str, program: &str, environment: &BTreeMap<String, String>)
    -> Option<String>; // why the harness cannot be found with `environment` alone: a path that is no
                       // file, a name not on its PATH, or no PATH (on Unix then only /bin:/usr/bin,
                       // where execvp looks); never the Station's own PATH
pub fn canonical(path: &Path) -> PathBuf;     // links followed, no `\\?\`; as given on failure
pub fn path_text(path: &Path) -> String;
pub fn starts_with(path: &Path, base: &Path) -> bool;   // case ignored on Windows
pub fn variable(environment: &BTreeMap<String, String>, name: &str) -> Option<String>;
pub const STATION_CONFIG_ROOTS: [&str; 3];   // /etc/ai6, /usr/local/etc, /usr/etc
/// What a scripts sandbox denies again: STATION_CONFIG_ROOTS (not on Windows), the platform
/// state directory, the `station` directories (`AssistantMode::station_paths`: the Station's
/// state, identity and secrets directories) and the running program's directory (the program
/// alone when an `open` directory holds or lies in it); a path that is or holds `cwd` or an
/// `open` directory is left out.
pub fn station_paths(cwd: &Path, open: &[PathBuf], station: &[String]) -> Vec<PathBuf>;
pub fn denied_station_paths(cwd: &Path, open: &[PathBuf], station: &[PathBuf],
    program: Option<PathBuf>) -> Vec<PathBuf>;   // station_paths, given
impl AssistantMode { pub fn moneypenny() -> Self; pub fn validate(&self) -> Result<(), String>;
                     pub fn with_cables(self) -> Self; } // adds server `cables`,
                                                         // prefix AI6_ASSISTANT_CABLE_MCP
impl AssistantNetwork { pub fn reaches_nothing(&self) -> bool;   // denied, or limited with no host
                        pub fn open(&self) -> bool; }
// Claude Code: `--restricted --tools= --disallowedTools <its shell, file, web, sub-agent and
// Skill tools, no mcp__> --allowedTools mcp__<name>,… --setting-sources user
// --strict-mcp-config --mcp-config {"mcpServers":{<name>:{"type":"http","url":"${P_URL}",
// "headers":{"Authorization":"Bearer ${P_TOKEN}"}}}}`, no gate. Codex: `-c mcp_servers=
// {<name>={url="…",bearer_token_env_var="P_TOKEN"}}` (replacing the user's servers), the same
// `-c` settings that switch its built-in tools off as answer-only, the project untrusted, and
// the answer-only thread policy. Codex refuses to start (UNSUPPORTED) with skills,
// interpreters or a network reaching more than the servers; both refuse (BAD_VALUE) a mode
// `validate` refuses, or when the environment lacks a server's URL or token; tokens never
// reach a command line.
// `validate` also checks `files` and `read_only` (1 to 8 `/`-separated names of letters,
// digits, `.`, `-`, `_`, none starting with `.`), interpreters (`interpreter_name`), hosts
// (`host_pattern`), that `context_file` is absolute, and that `skills` come with an absolute
// `skills_dir`: a Claude Code plugin directory (`skills/<name>/SKILL.md`,
// `.claude-plugin/plugin.json` naming `ai6`). Claude Code offers
// exactly the built-in tools the parts call for in `--tools` (none: `--tools=`) under
// `--permission-mode dontAsk`, denies the rest by name: with `files`, `Read,Write,Edit,Glob,
// Grep` and `--allowedTools …,Read,Glob,Grep,Edit(/<dir>/**),…` (no path with a name starting
// with `.`); with interpreters, `Bash` and `Bash(<interpreter> *)`; with a `limited` network
// that lists hosts, `WebFetch` and `WebFetch(domain:<host>)`; with `open`, `WebFetch`,
// `WebSearch`, `WebFetch(domain:*)` and `WebSearch`; with `skills`, `Skill`,
// `Skill(ai6:<name>)` per skill and `--plugin-dir <skills_dir>`. One `--settings` value holds
// every setting the parts call for. Interpreters add the
// sandbox (`enabled`, `failIfUnavailable`, `autoAllowBashIfSandboxed: false`,
// `allowUnsandboxedCommands: false`, `filesystem` {`allowWrite`: the `files` directories,
// `denyWrite`: `read_only`, `.claude` and the `.git` of the working directory and each writable
// directory, `denyRead`: `["/"]` and the Station's own paths, `allowRead`: the working
// directory, the platform paths an interpreter needs (not `/etc` whole), each interpreter's
// install directory and, on Linux, the sandbox's two relay sockets by name (in `/tmp`, or in
// the Session's `CLAUDE_CODE_TMPDIR`)}, `network` {`allowedDomains`: the hosts, `["*"]` when open;
// `deniedDomains`: the names and single addresses of `network.denied`; `strictAllowlist`},
// `credentials.envVars` denying every environment variable but PATH, HOME, temp, locale ones,
// and the model credentials, `credentials.files` denying the credential directories
// (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `*_HOME`)) and
// `permissions.blockReadsOutsideWorkingDirectories`;
// `unsandboxed` makes it `{"sandbox":{"enabled":false},…}`. Skills without `skill_commands`
// add `"disableSkillShellExecution": true`. Any network but `denied` adds a
// `PreToolUse` hook on `WebFetch` running `<gate program> assistant web-gate [--hosts …]
// [--deny …]` (`onFailure: block`). Refused: a `network.denied` range with scripts that reach a
// host (UNSUPPORTED), an environment variable name `credentials.envVars` cannot take
// (BAD_VALUE). A network reaching nothing adds `CLAUDE_CODE_DISABLE_WEB_FETCH=1`;
// a `context_file` is `--append-system-prompt-file`; every assistant Session has
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
// The bounds of jobs, reports and changes are AssistantLimits ([assistant.limits]), passed to
// each validate(); both the Station and Universal Exports check them.
pub const EVENT_ACCEPTED, EVENT_REJECTED, EVENT_TEXT, EVENT_TOOL, EVENT_COMPLETED,
          EVENT_FAILED, EVENT_LIMIT, EVENT_INTERRUPTED, EVENT_CLOSED: &str; // "accepted", …
pub const EVENT_NOTICE: &str = "notice";       // text: work saved elsewhere than main, a repository not opened
pub const EVENT_CABLE_PROPOSAL: &str = "cable_proposal"; // `cable`: held, body included
pub const EVENT_CABLE_SENT: &str = "cable_sent";      // `cable`: sent, `cable_id` set
pub struct AssistantJob { pub job_id: String, pub conversation_id: String, pub agent_id: String,
    pub route: Option<RouteRef> /* default, omitted when None: the route `session_id` ran on, the only one it resumes on */,
    pub message: String, pub context: Vec<String> /* default: standing context */,
    pub recap: Option<String> /* default, omitted when None: for a harness started afresh only */,
    pub mcp_token: String,
    pub token_expires_at: f64, pub session_id: Option<String> /* default */,
    pub close: bool /* default */,
    pub interrupt: bool /* default */,
    pub person_id: Option<String> /* default, omitted when None; required with workspace, commit or cables */,
    pub workspace: Option<AssistantWorkspace> /* default, omitted when None */,
    pub commit: Option<AssistantCommit> /* default, omitted when None */,
    pub cables: Vec<AssistantCableGrant> /* default, omitted when empty */,
    pub cable_confirm: Option<AssistantCableDecision> /* default, omitted when None */,
    pub cable_discard: Option<AssistantCableDecision> /* default, omitted when None */,
    pub reach: AssistantReach /* default: runs and reaches nothing */,
    pub skills: Vec<String> /* default, omitted when empty: catalog skills the person is given */,
    pub groups: Option<Vec<String>> /* default, omitted when None; the person's directory groups,
                                       so the Station re-caps reach and skills by its per-group
                                       tables and takes only those groups' shared repositories;
                                       requires person_id; None makes a group-narrowing roster
                                       refuse and gives no group's repository */,
    pub images: Vec<AssistantImage> /* default, omitted when empty: at most
                                       AssistantLimits::images, each named by digest; the Station
                                       reads the bytes from Universal Exports */ }
pub struct AssistantImage { pub sha256: String /* lowercase hex */, pub media_type: String,
    pub bytes: u64 }   // validate(&AssistantLimits): IMAGE_MEDIA_TYPES, at most image_bytes
// section_00::content: a message with images, as the text an AgentSession is sent:
pub const CONTENT_PREFIX: &str = "\u{0}ai6-content\n";   // then the JSON of MessageContent
pub const IMAGE_MEDIA_TYPES: [&str; 4];   // image/png, image/jpeg, image/webp, image/gif
pub struct MessageContent { pub text: String, pub images: Vec<MessageImage> }
impl MessageContent { pub fn encode(&self) -> String; pub fn decode(message: &str) -> Option<Self>; }
pub struct MessageImage { pub media_type: String, pub data: String /* base64 */ }
                                                     // Debug redacts the token
impl AssistantJob { pub fn validate(&self, limits: &AssistantLimits) -> Result<(), String>; }
#[derive(Default)] // serde: every key default, unknown keys refused
pub struct AssistantReach { pub interpreters: Vec<String>, pub network: AssistantNetwork,
                            pub unsandboxed_scripts: bool }
impl AssistantReach {
    /// The interpreters both name, the stricter policy (denied < limited < open; unknown is
    /// denied) with the hosts both reach (an open side reaches what the other lists), and
    /// unsandboxed scripts only when both allow them.
    pub fn capped_by(&self, cap: &AssistantReach) -> AssistantReach;
    pub fn validate(&self) -> Result<(), String>;
}
pub fn host_pattern(host: &str) -> bool;       // lowercase DNS name with a dot, optional leading "*."
pub fn interpreter_name(name: &str) -> bool;   // 1-64 letters, digits, . - _, not a path or flag
pub fn credential_dir_var(name: &str) -> bool; // CLAUDE_CONFIG_DIR, CODEX_HOME or any *_HOME, case ignored
pub const SCRIPT_ENVIRONMENT: [&str; 14]; // the variables an assistant's sandboxed scripts keep (PATH, HOME, TEMP, LANG, …)
pub struct AssistantWorkspace { pub personal: String /* the person's repository, expanded */,
    pub shared: BTreeMap<String, String> /* default; checkout name (group or "org") -> repository */,
    pub author_name: String, pub author_email: String /* the person's */ }
pub struct AssistantChange { pub digest: String /* hex SHA-256 of the files, then the bases */,
    pub files: BTreeMap<String, Option<String>> /* path -> new text, None removes */,
    pub base: BTreeMap<String, Option<String>> /* default, omitted when empty: path -> hex SHA-256
        of the text it was made from, None = did not exist; a commit is refused when it differs */ }
impl AssistantChange { pub fn new(files) -> Self /* no base */; pub fn based(files, base) -> Self;
    pub fn digest_of(&files) -> String; pub fn digest_over(&files, &base) -> String;
    pub fn validate(&self, limits: &AssistantLimits) -> Result<(), String>; }
    // bases only of its files, 64 hex
pub struct AssistantCommit { pub repo: String /* a shared repository */,
    pub group: String /* a group or "org", file_safe */,
    pub change: AssistantChange, pub message: String /* trailer included */,
    pub author_name: String, pub author_email: String }
impl AssistantCommit { pub fn validate(&self, limits: &AssistantLimits) -> Result<(), String>; }
    // message 1 to commit_message_bytes
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
    pub job_id: Option<String>, pub session_id: Option<String>,
    pub route: Option<RouteRef> /* default, omitted when None: the route that Session runs on */,
    pub run: Option<AssistantRun> /* default, omitted when None */,
    pub events: Vec<AssistantEvent> }
pub struct AssistantRun { pub account: Option<AccountRef>, pub model: Option<String>,
    pub tokens: Option<u64> }   // what the harness ran on for these events; Default
impl AssistantReport { pub fn cut_texts(&mut self, limits: &AssistantLimits); } // each text to
    // event_bytes, on a char boundary; the desk cuts every report it sends
pub struct AssistantEvent { pub kind: String, pub text: Option<String>, pub tools: Vec<String>,
    pub cable: Option<AssistantCable> /* default, omitted when None */,
    pub shared: Vec<AssistantSharedBase> /* default, omitted when empty: on a `tool` event,
        one per propose_shared_change call, at most shared_bases */ }
impl AssistantEvent { pub fn new(kind: &str, text: Option<String>) -> Self; } // text as given
pub const ASSISTANT_SHARED_CHANGE_TOOL: &str = "propose_shared_change";
pub struct AssistantSharedBase { pub group: String /* the call's, the checkout name */,
    pub path: String, pub content: String /* hex SHA-256 of the call's content */,
    pub base: Option<String> /* hex SHA-256 of the file in her shared/<group>/ checkout, None = no file */ }
impl AssistantSharedBase { pub fn validate(&self) -> Result<(), String>; } // file_safe group,
    // repository_path, 64-hex digests
pub fn valid_identity(value: &str) -> bool;          // 1-256 bytes, no control characters
```

All are serde types with unknown fields refused. `validate(&AssistantLimits)` checks
identities, a message of at most `message_bytes` (blank only with `close`, `interrupt`,
`commit`, `cable_confirm` or `cable_discard`, of which a job sets one), the
`context_bytes` bound (context and recap together), a token of 32-512 bytes without
whitespace, a workspace (repositories of a known `RepoKind`, checkout names as `file_safe`
names, an author without control characters, `<` or `>`), a commit (a `file_safe` group; the
change's 1 to `change_files` files within `change_bytes`, each `repository_path`, its digest
matching; a message of 1 to `commit_message_bytes`),
at most `cable_grants` Cable grants for distinct valid Operations, which need `person_id`,
and at most `skills` distinct `skill_name` skills. A job's
answer is one report with its `job_id` and one `accepted` (for a commit, text saying where it
landed) or `rejected` event (text: the reason); the
harness's own events follow in reports without a `job_id`.

A handler reports a provider usage or rate limit as an event of kind `LIMIT_EVENT` with
`limit` set, in place of the `completed` or `failed` event the same output would
otherwise produce; `limit` is `None` on every other event. `resets_at` is `None` when the
provider names no reset time. The daemon parks the Mission (or posts a backup Agent) and
closes the Session.

A handler reports calls its harness refused by its permission rules in a turn as an event of kind
`DENIED_EVENT`, before that turn's `completed` or `failed`: `text` names the tools, separated by
`, `, and `tools` stays empty, since they did not run. The daemon records them in
`Mission.denied_tools`, logs them, keeps the event, publishes the Mission's end with the reason
`permission-denied`, and gives them to a source Mission's outcome (`denied_tools`), which the CX
report quotes unless the disclosure is `Hidden`. The Mission's state is whatever the turn ended in.

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
`AgentTerminal` is the object bridge AI6 wraps around that provider for a handler. What `spawn`
starts is the Session: its output reaches viewers. What `spawn_private` starts is a probe of the
handler's own: only the handler reads its output.
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
    fn spawn_private(&self, spec: &Value) -> Result<Object<dyn TerminalSession>, ProviderError>;
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
/// Drops CSI (one with more than `csi_bytes` parameter bytes is no sequence), OSC and two-byte
/// escapes; keeps line breaks.
pub fn strip_controls(bytes: &[u8], csi_bytes: Limit) -> Vec<u8>;
pub enum StreamLine {
    Json { value: serde_json::Value, text: Vec<u8>,
           rebuilt: bool /* joined from a broken line, or a non-SGR control removed inside */ },
    Text(String),
}
pub struct JsonLines;
impl JsonLines {
    /// A held, unfinished value past `limits.json_line_bytes` is handed over as text; a CSI past
    /// `limits.csi_bytes` is no sequence.
    pub fn new(limits: &TerminalOutputLimits) -> Self;
    pub fn push(&mut self, bytes: &[u8]);               // output as read
    pub fn next_line(&mut self) -> Option<StreamLine>;   // a whole line, if one has arrived
    pub fn finish(&mut self) -> Option<StreamLine>;      // after end of output: what is held; call until None
}
```

Lines are cut at line breaks and at cursor moves to another row (CUP, HVP, CNL, CPL, VPA,
IND, NEL, RI), controls and CR removed. A CSI ends at its final byte, or before the first byte
no CSI holds (a line break included); past `csi_bytes` parameter bytes (64 by default) it is
no sequence and only its `ESC [` goes. An OSC ends at BEL, `ESC \` or before a line break. A line starting with `{`
or `[` that does not parse yet is held, untrimmed, and joined with the lines after it; it is
parsed again only when a piece closes its outermost bracket (strings and escapes tracked).
A line that is a whole value of its own ends the held one as text. Anything else is `Text`,
trimmed, never empty. Handlers spawn with `cols: STREAM_COLS` so that few lines wrap at all.

## Mission lifecycle (`ai6::directorates::m::mission`)

`MissionStore` validates lifecycle transitions, associates provider `SessionId` values
with an AI6-owned `MissionId`, records safe-house disposition and optional `CoverReport`,
and increments a revision for changes. Repeating the current state is a no-op and does not
increment the revision. `MissionState` serializes as `queued`, `working`, `blocked`,
`stalled`, `parked`, `quarantined`, `held`, `verifying`, `done`, `failed`, `blown` or `abandoned`; `blown` means a cover policy
violation when the Operation uses `on_blown = "fail"`. `parked` means a provider usage limit
paused the work: `Mission.parked_until` (Unix seconds, `None` unless parked) is
when it resumes. `MissionStore::park(id, until)` parks a `Working`, `Blocked`, `Stalled` or
already parked Mission (parking again moves the time; `transition` refuses `Parked` because
it needs a time); `due_parked(now)` lists parked Missions at or past their time;
`requeue_for(id, agent_id)` queues an interrupted Mission again for another Agent.
`retry(id, agent_id: Option<String>)` queues a `Stalled`, `Done`, `Failed`, `Blown` or
`Abandoned` Mission again as a new attempt, for `agent_id` when given and otherwise its
Agent; it keeps `session_ids` and the safe house report, and refuses every other state
with `InvalidTransition`. `retry_past_brief(id, agent_id)` is that on the Station's own word, and
sets `Mission.runs_past_brief` until it starts: such a Mission starts whatever its brief was filed as,
while `forget_queued(id)` takes any other out of the book when its brief settled before it started.
`fail_reported(id, said)` fails it as its harness reported, keeping `Mission.reported_failure` for the
outcome; `refuse_resume(id, why)` sets `Mission.resume_refused`, so the next start begins a new Session.
`reopen` sets `Mission.after_failure` when the Mission ended `Failed` or `Blown`, and a standing
Sleeper's resume then starts a new Session too, whatever the last Session record says yet.
`reopen(id, agent_id, prompt, digest)` queues a `Done`, `Failed`,
`Blown` or `Abandoned` Mission as its next attempt for a later claim of its brief (a reopened
item, a standing Sleeper's window): the new prompt, the Agent, `Mission.brief_digest`, and the launch
manifest's Session hold and runner let go, so the next start records a run after the last
(`InvalidTransition` otherwise). `record_brief_digest(id, digest)` records `Mission.brief_digest`
(`Option<String>`, serde default: the brief the Mission runs from; the same brief claimed
again is a lapse recovered, not an attempt). A
parked Mission leaves `Parked` through `Working`, `Failed`, `Abandoned` or `requeue_for`,
and any transition clears `parked_until`.
`quarantined` means Medical or an operator stopped the work because its Agent is
quarantined: `MissionStore::quarantine(id)` moves a `Working`, `Blocked`, `Stalled` or
`Parked` Mission there (repeating it is a no-op), it leaves only through `requeue_for`
(back to `Queued`, for the next Agent) or `Abandoned`, and `reassign(id, agent)` gives a
queued Mission another Agent. `block_for(id, reason)` blocks an active Mission and records
`Mission.blocked_reason` (`"budget"`; cleared by the next state change); `fail_for(id, reason)`
fails a queued Mission whose harness cannot be started and records `Mission.failure`
(`Option<String>`, serde default, cleared by the next state change or a retry), which its brief's
outcome carries as `reason`;
`note_denied_tools(id, tools)` records `Mission.denied_tools` (`Vec<String>`, serde default empty,
cleared by a transition to `Working`); `expose(id, sources)`
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
each mutation. The daemon opens `missions.json` under its state directory; this is
runtime Mission state, separate from the durable Registry. The snapshot
stores non-secret handler, terminal, work-directory, model and effort settings for each
started Mission (the terminal is the one the Station chose at start), including the cover policy, starting commit and identity; environment values
are never persisted. The snapshot format is version 1; any other version is refused. On
startup, `Working` Missions
become `Blocked` and the scheduler retries reattachment through the same provider Session ID.
The daemon's
`/internal/missions` routes expose Mission state to local control components, each request with
`Authorization: Bearer TOKEN` from `<state dir>/api-token` (`server::API_TOKEN_FILE`; 401
otherwise, from loopback too; `/healthz` is open); Universal Exports remains the public API.

## Deliverables and verifying (`ai6::directorates::m::deliverables`)

A Session that ends done does not settle its Mission: a Mission of an Operation goes `verifying`
(`MissionState::Verifying`; non-terminal, no harness, its brief stays claimed) and leaves it only by a
decision. Review and panel Missions, Head of Station rounds and Missions with no Operation settle done as
before. A `held` Mission (a role's `require_approval_before_done` unmet) stays `held`.

```rust
pub const BRIEF_DELIVERABLES_KEY: &str = "deliverables";   // a brief's `data` key: an array of strings
#[non_exhaustive] pub enum DeliverableKind { Branch, CommitOn, Comment, Artifact, Approval, File }
    // ALL, as_str() ("branch", "commit_on", "comment", "artifact", "approval", "file"), from_name()
#[non_exhaustive] pub struct Deliverable { pub kind: DeliverableKind, pub target: Option<String> }
    // text `kind` or `kind:target` (serde, Display, Deliverable::parse); commit_on, comment, artifact
    // and file need a target: `commit_on:BRANCH[@COMMIT]`, `comment:TEXT`, `artifact:URL`, `file:PATH`
pub fn parse_all(&[String]) -> Result<Vec<Deliverable>, InvalidDeliverable>
pub fn declared(role: &[String], brief_data: &BTreeMap<String, serde_json::Value>)
    -> Result<Vec<Deliverable>, InvalidDeliverable>   // the role's, then the brief's, each once
```

`AgentRole::deliverables` is validated with the role. The Station records `declared(..)` on the attempt's
`SessionRecord::deliverables` when it goes verifying. Probes (crate-private, `station::probes`) read them
back, each within its `[probes]` timeout, and never say whether they are enough: `branch` is `git ls-remote
--exit-code --heads origin` in the Mission's working directory (else its Operation's checkout), `commit_on`
adds a fetch of the branch to read its history, `file` stays inside the working directory, `artifact` is an
http(s) GET with the body unread, `approval` is a decided, approved, unrevoked green ink request,
`comment` reads the linked tracker item back by id and looks for the text (only a Station that holds the
tracker's credentials can). Work on a Theater this Station cannot read is `unprobed`; a read that failed or
timed out is `failed`, never `missing`. `SessionStore::request_probe` asks the Station holding the attempt
to run them on its next pass; the findings are appended to `SessionRecord::evidence`.

A decision is a verdict on the verifying attempt (`record_verdict`): `Confirmed` settles the Mission done,
`Disputed` and `FailedVerification` fail it, `Redispatch` queues its next attempt: attempt N+1 of the same
Mission for the best qualified Agent (`MissionStore::redispatch(id, agent, amendment)`, which keeps the safe
house and starts a new provider Session), told the person's reason and amendment after its brief
(`Mission::task()`). The verified attempt keeps its verdict and counts against its Agent on the scorecard.
Nothing else leaves `verifying`.

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
`with_plugin_paths`, `with_config_dir`, `with_config_provider`, `with_registry`,
`with_station_id`, `with_station_config`, `with_max_concurrent_missions`, `with_watchdog`
and, with feature `publisher`, `with_publisher` set those options before calling
`serve_with_options`. The configuration directory (`with_config_dir`, default
`config_dir()`) holds the Station's Theaters, loadouts and Cable settings, and its roster
unless `with_config_provider(Arc<dyn ConfigProvider>)` names another provider; the roster is
read once at start. `with_registry(Arc<dyn RegistryBackend>)` replaces the
`FileRegistry::from_environment()` default.

Each scheduler round also tends the watches on the roster this Station may run
(`docs/watches.md`): on a check-in it claims or renews each, closes one another Station took, and
looks at its area (Missions of its Operation in `verifying`, quarantines, dark Stations for M,
controls); every round it takes the watch's Cables, adds a `wake_every` timer, and starts one turn
of waiting events when the hour's `max_turns_per_hour` has room. The harness runs on this host in
`<state dir>/watches/watch--<scope>`, started as `AgentStart.mission_id = "watch--<scope>"` (no
Mission has that ID), resumed with the record's provider Session, or started fresh with the
logbook when the resume is refused or fails before it answers. A Station's watch gets
`station_report` and `file_report` on its Cable endpoint. Stopping closes every watch's harness.

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
snapshots only currently managed Sessions as recipients, and the internal API needs the
Station's API token.
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
Session start and resume, last before the launch (one-hour lease, each secret from the provider
its definition names: `file` relative to the secrets directory by default; at start and on each
loadout reload the Station logs each env or inline secret as a weakening and each provider or
secret it refuses) and puts each value in the start or
resume request's `environment` under its `env`, after removing the loadout's variables an
earlier Session held. A resume that re-attaches to a runner's live harness issues nothing.
A provider refusal of an ordinary secret fails the start, and leases acquired by a failed
issue are revoked. A `green_ink` secret is leased only while the Mission holds a live
approval (`ApprovalStore::mission_approvals`) of one of its `kinds` whose target matches one of
its `targets`, once per approval ever (`ApprovalStore::spend_approval`, so another Station or a
restart does not lease it again; a Station that loses the race revokes its lease), with the
approval's remaining time, at most an hour, as the TTL; otherwise its variable is absent. At
start the Station logs as weakenings a green ink secret without `targets` and one served by
a built-in provider (`file`, `env`, `inline`), whose values a revocation cannot take back. A start or
resume request's `environment` cannot set `AI6_GREEN_INK_GATE`. Each scheduler pass revokes, at the backend, every lease past its TTL
and each green ink lease whose approval has expired or been revoked (a
`credential`/`revoked` timeline event naming the secrets and the approval); an approval the
Registry cannot read keeps its lease until the next pass. An exposed Session under the `secrets` restriction gets none. Leases are
revoked when the Session's operation is finished, stopped, finalized or fenced out, on a
failed start or resume, and when a later issue for the Mission replaces them. Each issue is a
`credential`/`leased` timeline event (`secrets` = names, `approval` = request ID). Values are
never written: the launch record and publication carry variable names only, the detached
wrapper passes the environment through a FIFO, and a runner keeps it in memory.
Routes: for an Agent with `routes` the Station surveys each at the start (handler available,
`AgentHandler::clearance_refusal` when it hosts the handler, secrets and `env` acceptable, account
source with room), picks by `balance` (`choose_route`) and goes on as `Agent::on_route`. The route's
`env` passes a review's filter, its `secrets` are leased through `Loadout::for_route` whatever the
Session read, and one with no pool claims an account of its own (`route-<digest>`/`login`,
`AccountKind::HarnessLogin`), so its limit is shared. `LaunchManifest` and `SessionLaunch` carry the
`pool`. With no route open the Mission waits, naming each. A resume stays, moves to a route of the
same handler whose accounts carry the history, or queues a new attempt.

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
Theater's loopback, re-opened on the same port when a read had to wait for the link. Placement
(`execution_mode`) rules the wrapper out on a transport without `forward` unless the Theater
says `cables = "optional"` (`CableReach`), and such a Session then starts with no Cable
endpoint, logged as a weakening.
This private listener is for
daemon health, provider discovery and Universal Exports-to-daemon operations; user-facing
Station and Mission operations belong to Universal Exports's authenticated API.

## Field runner (`ai6::station::runner`)

`ai6 runner serve` is the binary's Theater mode: it supervises harnesses on the Theater they
run on, so a Session outlives its Station. It loads AgentHandler and Terminal providers like a
Station and has no scheduler, Registry or roster.

```rust
pub const CONNECT_NOT_RUNNING: u8 = 3;         // exit status of `ai6 runner connect` with no runner
#[non_exhaustive]
pub struct RunnerInfo { pub address: String, pub pid: u32, pub version: String }   // runner.json
impl RunnerInfo { pub fn read(state_dir: &Path) -> io::Result<Self>; }       // absent while none serves
#[non_exhaustive]
pub struct RunnerConfig { /* state_dir, listen (default 127.0.0.1:0), plugin_paths, terminal, settings */ }
impl RunnerConfig {
    pub fn new(state_dir: impl Into<PathBuf>) -> Self;
    pub fn with_listen(self, address: SocketAddr) -> Self;   // loopback only: serve refuses any other
    pub fn with_plugin_paths(self, paths: impl IntoIterator<Item = PathBuf>) -> Self;
    pub fn with_terminal(self, terminal: impl Into<String>) -> Self;
    pub fn with_limits(self, limits: RunnerLimits) -> Self;
    pub fn with_provider_configs(self, configs: BTreeMap<String, serde_json::Value>) -> Self;
}
pub const SETTINGS_FILE: &str = "settings.json";             // what serve runs by, in the state directory
/// {"runner": RunnerLimits, "providers": {...}}; serde default, deny_unknown_fields
#[non_exhaustive] pub struct RunnerHandoff { pub runner: RunnerLimits, pub providers: BTreeMap<String, serde_json::Value> }
impl RunnerHandoff {
    pub fn new(runner: RunnerLimits, providers: BTreeMap<String, serde_json::Value>) -> Self;
    pub fn read(path: &Path) -> io::Result<Self>;            // InvalidData: not settings, replay_batch 0, a provider value Guatiao cannot hold
    pub fn parse(bytes: &[u8]) -> io::Result<Self>;
    pub fn in_state_dir(state_dir: &Path) -> io::Result<Self>;   // SETTINGS_FILE, defaults when absent
}
pub fn default_state_dir() -> io::Result<PathBuf>;           // `<AI6 state>/runner`
pub async fn serve<F>(config: RunnerConfig, shutdown: F) -> io::Result<()>;  // F: Future<Output = ()> + Send + 'static
pub fn connect_stdio(state_dir: &Path, limits: &RunnerLimits) -> Result<(), String>;  // `ai6 runner connect`
pub fn spawn_detached(program: &Path, arguments: &[OsString], state_dir: &Path, limits: &RunnerLimits)
    -> io::Result<RunnerInfo>;
```

`serve` takes `runner.lock` (a second runner on the same directory gets `AddrInUse`), creates
`token` once (mode `0600` on Unix), writes `runner.json` (`address` is `host:port`) once it
listens and removes it on a clean stop. It speaks the Agent Client Protocol, one JSON-RPC
message per line, on that port; a connection serves any number of Sessions and several
connections may be open. `initialize` advertises `authMethods: [{id: "ai6-runner-token"}]`,
`loadSession`, `sessionCapabilities.resume|close` and `_meta["dev.coqui.ai6/events"]`.
`authenticate` with `_meta.token` (compared in constant time) must come before any
`session/*` or `_ai6/*` call, and may come before `initialize`. Then:

- `session/new` with `_meta["dev.coqui.ai6/launch"]` (`LaunchMeta` with `handler`
  (`HandlerSpec`: id and the Agent's `handler_settings`), `terminal` and `key`) names a Session
  (letters, digits, `.`, `_`, `-`; the ACP session ID is the key); the first `session/prompt`
  starts the harness with that text as the task. `session/resume` with `ResumeMeta` (also
  `key`, `handler`) resumes a provider Session under a new key. Later prompts are messages.
- `session/load {sessionId: key, _meta: {"dev.coqui.ai6/after": N}}` attaches to a Session the
  runner still holds and replays the events after `N` (the cursor the Station last
  acknowledged when absent), then follows it live. A newer connection to a Session takes it
  over: the older gets an `_ai6/event` of kind `detached` and is closed.
- Every event is sent as `_ai6/event {sessionId, providerSessionId, seq, replay, event}`; the
  start marker (no `event`) carries `terminal`, `handler` and `last`. `_ai6/ack {sessionId,
  seq}` stores the cursor. `session/cancel` interrupts, `session/close` ends the Session and
  its harness, `_ai6/session_status {key}` answers handler, terminal, `session_id`, `ended`,
  `last`, `cursor` (`-32002` when unknown). A `completed` event answers the waiting prompt `end_turn`, a
  `cancelled` one (or `blocked` with `TURN_CANCELLED`) `cancelled`, a `failed` or `limit` one an
  error.

Every event is appended to `sessions/<key>/events.jsonl` with its sequence number (from 1)
before any Station can read it, and the cursor is kept in `cursor`, so a Station that comes
back replays what it missed (at least once: the last event it took may come again). A
`completed`, `failed` or `limit` event ends the Session and stops the harness. A runner that
restarts ends every Session it had running with a `lost` event saying so (a Station gives
such a Session up as lost); ended Sessions older than `keep_ended` (seven days) are removed
when a runner starts. `serve` builds providers with `[providers.<id>]` when a request gives no
configuration, and records its settings in `SETTINGS_FILE`; `ai6 runner serve --settings PATH`
(`-`: standard input, which `--detach` writes to `SETTINGS_FILE` and passes on) reads them.
When the launch's `LaunchMeta::cables` (or `ResumeMeta::cables`) is set, the runner starts a
loopback relay with its own URL and token and gives the harness those as `AI6_CABLE_MCP_URL`
and `_TOKEN` (any it was handed are dropped). Each HTTP request goes to the connection bound to
the Session now as `_ai6/cable_request {sessionId, id, method, headers, body}` (base64 body,
MCP's headers only); that Station answers with `_ai6/cable_answer {sessionId, id, status,
headers}`, then `{data}` pieces and `{end}`, or `{error}`, and the relay sends
`_ai6/cable_cancel {sessionId, id}` when the harness stops reading. With no Station bound the
relay answers 503; a connection that ends drops its open requests (502). On `session/resume`
the connection is bound before the harness starts. Stopping the runner stops its harnesses and
closes its connections.

`ai6 runner connect [--state-dir D]` connects to the loopback port within `connect_timeout` of
the state directory's `SETTINGS_FILE`, authenticates with the token it reads from the state
directory, prints `ai6-runner-connect-ready` (after putting a terminal on its standard input
in raw mode on Unix) and relays standard input and output both ways; it exits
`CONNECT_NOT_RUNNING` (3) when no runner answers. `spawn_detached` starts
`program runner serve <arguments>` in its own process group (Unix) or detached (Windows),
with output in `runner.log`, and answers once that process serves; on Windows it inherits
the caller's inheritable handles, such as an output pipe.

A Station uses the runner of the Theater a Session runs on when the Theater declares one
(`Theater::runner`). On its own host it connects to the port directly, authenticating with
the token file, and starts the runner with `spawn_detached` (its own binary unless `command`
is set) when none answers, its `[runner]` and `[providers.<id>]` written to the runner's
`SETTINGS_FILE` and passed with `--settings`. On another host it opens `<command> runner
connect [--state-dir D]` on a terminal through the Theater's transport (`command` defaults to
`ai6`), waits for the ready line, and speaks ACP over it; `<command> runner serve --detach
--settings -` with the settings' plugin paths and terminal, and the settings JSON on standard
input, starts the runner when none answers. The Theater's `runner` settings
`connect_timeout_ms` (30000), `initialize_timeout_ms` (20000) and `request_timeout_ms` (30000)
bound those steps. A launch is an `AcpSession` with `close_on_drop` and `fail_on_disconnect`
off, and, for a Session with a Cable endpoint, `AcpOptions::extensions` set to a `CableTunnel`
that answers the relayed requests from the Session's endpoint with its bearer (at most
`[runner] cable_streams` at once, reaching it within `cable_connect_timeout`), on that and on
every later connection; the launch environment carries neither the endpoint's URL nor its
bearer. `RemoteSession` wraps it: one read waits at most `read_wait` for an event, a dropped
connection costs only time, the next `next_event` attaches again with `session/load` after the
last event taken. Past the Operation's `lost_after_secs` out of reach it probes once more and, if
the runner is gone (or answers not found for the Session), gives the Session up as lost (see
Recovery). A Session another connection took over stops reading (`detached`). The start records
the runner Session's key in the launch record before the harness starts. Shutdown and losing
the Session lease let go without stopping the harness; the Mission stays Working, so a restarted
Station re-attaches to the same runner Session (a parked Mission is resumed instead) and
answers its Cable relay from its own endpoint. Events are acknowledged when the Station asks for
the next one, and closing the Session acknowledges what it took, so nothing is replayed after
it. Watchdog terminal-output checks (blocked prompt, loops) do not see a harness on a runner;
its events still count as activity.

## Universal Exports publication (`ai6::station::publisher`)

`Config::try_new(origin, token)` accepts a bare HTTPS origin or an HTTP loopback origin;
its `Debug` output redacts the token. `ServeConfig::with_publisher(config)` enables
publication through `serve_with_options`. The `ai6` command publishes when its bootstrap
file's `[unex]` has a `token` besides its `url`; a `url` alone (what `ai6ctl` reads)
publishes nothing. `Config::with_ca_certificates(&[der])` (`[unex] ca_file`, read by
`bootstrap::read_ca_file`; a bad file stops `ai6 serve` at start) trusts those authorities
besides the public roots on every link to the gateway: publishing, input, controls and the
assistant desk. Without it the built-in public roots alone apply. Redirects are disabled. A bounded
background thread orders Session registration, lifecycle updates and raw terminal chunks
on one event cursor. A separate authenticated long-poll receives validated Guatiao input
and resize frames from `/v1/station/inputs/next`; AI6 dispatches them only to the live terminal
bound to the matching Station, Mission and provider Session. The gateway's user-scoped
write grant and lease checks remain the authorization boundary. Requests time out after
two seconds; outbound writes are not retried, while input polling retries after errors.
A queue overflow (64 events) or outbound gateway error disables publication and is reported
by `GET /internal/publisher`; it does not block Mission execution. Terminal chunks and the
pre-registration output buffer are each bounded to 1 MiB. The `station` record's
`[publisher]` table sets each of these. Output read before the provider
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

A job's `skills` must all be ones this Station's own roster gives its person
(`roster.assistant().skills_for(person_id, groups)`, the groups as for its reach; a job with no
`person_id` is given none), or the job is `rejected` naming the first one that is not; a
running harness started with other skills is restarted, as for a changed reach. Before the harness starts (and
before a workspace is prepared), the desk empties `<conversation dir>/.claude` and copies each
skill's directory into `.claude/skills/<name>/` (regular files and directories only: a link,
device or pipe anywhere, or the `ProjectManifest` limits passed, leaves the skill out), then
checks the copy: `ProjectManifest::read_tree(copy).digest_with(pin.digest()) == pin` when the
skill has a pin, and `check_skill(copy, name, skills_allow_tools)`. A skill that fails is
removed, logged (`ai6: assistant skill <name> does not match its pin`, or `… was not
installed: <reason>`), filed as a `StationReport` with one `provider` warning finding of the
same text, and reported to the conversation as a `notice` with its first event. With one skill
or more it writes `.claude/.claude-plugin/plugin.json` (`{"name":"ai6",…}`) and starts the
harness with `skills` (those installed), `skills_dir` (`.claude`) and `skill_commands =
skills_allow_tools`; the workspace's standing text lists them. When `Harness::skills` (the handler's `assistant_skills`) says the harness cannot keep to them, the desk installs none, logs `assistant conversation <digest>: skills are unavailable with handler <id>: <reason>` and gives the start context the line `Skills are unavailable in this conversation (<reason>), …`.

A job with a `workspace` gets, through the `git` safe-house provider (`SafeHouseSpec::clone`,
`CheckpointSpec::branch`): `personal/`, a clone of `workspace.personal` on `main` (a missing
local or `file://` repository is made bare first); `shared/<name>/` per shared repository,
cloned afresh at each harness
start (one that cannot be opened is skipped with a `notice`); every git directory in
`<conversation digest>.git/{personal,shared/<name>}` beside the conversation's directory, so
none is inside it (`SafeHouseClone::git_dir`); `scratch/`; `CLAUDE.md` and `AGENTS.md` (the
desk's instructions); and `<conversation digest>.context.md` beside the
directory with the same text. The harness's mode is `moneypenny()` with `files =
["personal", "scratch"]` and that `context_file`. When the turn's `completed` arrives, and again
after the harness closes, the desk commits `personal/` (paths with a name starting with
`.` left out) to `main` as the person, message
`moneypenny: update <paths>` (or `save her work`) with the trailer `Via: Moneypenny
(conversation <id>)`; work `main` refuses twice lands on `moneypenny/<conversation digest>`
with a `notice`. A `commit` job has
the desk clone the commit's repository into a temporary directory, commit the files'
text there exactly without writing them, pushes, and removes it; a file whose
`AssistantChange::base` differs from what `main` holds refuses the commit. A `tool` event whose raw
line is Claude Code's call of `mcp__moneypenny__propose_shared_change` (a `tool_use` block), or Codex's `item/started` of an `mcpToolCall` item with `server` `moneypenny`, `tool` `propose_shared_change` and its `arguments`, carries, in `shared`,
the SHA-256 of the file it names in that `shared/<group>/` checkout (`None` when there is no
such file); a call naming a checkout that is not there, or a path that is not a plain file,
gets none. The desk never reads, writes
or removes a file in the conversation's directory through a symbolic link, junction or other
reparse point (refused, or skipped when it walks a tree), and the provider checks links out as
plain files.

Her scripts and network: the desk caps `job.reach` by its own roster's
`assistant().reach_for(person_id, job.groups)` (a job with no `person_id` takes the strictest
reach; a roster that narrows by group refuses a job carrying no `groups`), so a job never
widens what this Station's roster gives. It recomputes the cap on every message and, when it
differs from the reach the running harness started under, restarts the harness rather than
reaching it warm. It sets the mode's `network` from the reach, logging
`weakening: assistant.network = "open": …` when it is open. With interpreters left, it asks
`Harness::scripts` (the handler's `assistant_scripts` with the harness environment): when the
harness can sandbox them, the mode gets them and, with a workspace, `read_only = ["shared",
"CLAUDE.md", "AGENTS.md"]`; when it cannot, the mode gets none, the start and resume context
get the line `Scripts are unavailable on this Station (<reason>), …`, and the log
`assistant conversation <digest>: scripts are unavailable on this Station: <reason>`, unless
the reach allows `unsandboxed_scripts`: then the mode gets them with `unsandboxed`, and the
log `weakening: assistant scripts run unsandboxed: …` (loud: a shell as the Station user). A
shell interpreter (`bash`, `sh`, `env`) is logged as a weakening too. The desk gives the
harness its own `CLAUDE_CONFIG_DIR` (`<conversation digest>.config` beside the directory),
removed with it, so the Agent's configuration directory (a shared login, other people's
transcripts) stays out and each conversation keeps its own, and its own temporary directory
(`<first 12 digest digits>.t` beside it, as `TMPDIR` and `CLAUDE_CODE_TMPDIR`: Claude Code
derives the sandbox's `$TMPDIR` from the harness's `TMPDIR`, which would otherwise be the
`/tmp/claude-<uid>` every conversation shares).

Her conversations carry no exposure: nothing she reads marks one, a Session's exposure
(`medical::exposure`) is for a Mission's Session only.

A job with a `workspace` or a `commit` must name `person_id`, and the Station refuses
one whose repositories its own roster does not give: the personal repository of a workspace
must be `personal_repo_for(person_id)` of its `[assistant]`, each shared one
`shared_named(name, job.groups)` (`org`, or a group the job puts the person in), and a commit's
repository `shared_named(commit.group, job.groups)`: Universal
Exports sends a commit's job with the person's groups as they are when it is sent.

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
every call the desk asks `GET /v1/station/assistant/cables?station&conversation&operation`,
which answers `send` and `read`, the person's `cable.send` and `cable.read` there now; without
an answer the call is refused. A send or reply needs the job grant's `send` and `send` now, a read or
acknowledgement `read` in both, discovery either; otherwise the answer is the relay's own
indistinguishable refusal. Reads use `inbox_from` the call's Operation; a reply or
acknowledgement only touches a Cable of that Operation delivered to the conversation. Retry
keys are scoped to the conversation. The Session is in the relay's endpoint directory,
conversation-only, while the harness runs, so replies to its Cables reach it and nothing else
does. A later job's grants replace the endpoint's. A Cable she reads marks nothing, and a Session
that reads a Cable from her is not exposed by it: her Session is no Mission's. `send_cable` and
`reply_to_cable` send nothing, whatever she has read: the
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
    pub depth: Option<u32>,        // a shallow clone of the last N commits of `base`
    pub timeout_secs: Option<u64>, // give up (removing what it made) after this; None: the provider's own
    pub max_bytes: Option<u64>,    // give up (removing what it made) past this on disk; None: no limit
}
pub struct CheckpointSpec {
    pub remote: String,    // Git remote name or URL
    pub message: String,   // already under the Operation's cover
    pub author_name: Option<String>, pub author_email: Option<String>, // the cover identity
    pub branch: Option<String>,   // commit in place and fast-forward this branch instead (a clone)
    pub fallback: Option<String>, // with `branch`: where the work goes when that branch refuses it
    pub files: Option<Vec<CheckpointFile>>, // with `branch`: commit exactly these, nothing of the working directory
    pub exclude: Option<Vec<String>>, // with `branch`, no `files`: paths left out with everything under them
    pub max_bytes: Option<u64>, // with `branch`, no `files`: refused, nothing committed, past this many bytes added or changed
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
the Mission and finished by the daemon, under the Station's state directory
(`StationConfig::with_state_dir`, default `ai6::bootstrap::platform_state_dir()`).

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
(see the terminal API) to them, with a pinned provider built and asked its own `tty`; a
Station and a runner choose a harness's terminal with it.

```rust
impl Plugins { pub fn scan_with(paths: &[PathBuf], policy: &PluginPolicy) -> Plugins; } // scan() = default policy
impl Plugins {
    /// Provider <id> built with configs[<id>] when its caller gives None (every kind).
    pub fn with_provider_configs(self, configs: BTreeMap<String, Value>) -> Self;
}
/// The station record's [providers.<id>] as Guatiao values; Err names a number Guatiao cannot hold.
pub fn provider_configs(configs: &BTreeMap<String, serde_json::Value>) -> Result<BTreeMap<String, Value>, String>;
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
    pub fn with_keep_verified(self, keep: KeepVerified) -> Self;
    pub fn profile(&self) -> PluginProfile;  pub fn pins(&self) -> &PinSet;
    pub fn keep_verified(&self) -> KeepVerified;
}
pub struct PluginScanSummary { pub profile: String, pub loaded, skipped, failed, unreadable: Vec<String>,
    pub checked: Vec<LibraryCheck>, pub refused: Vec<String>, pub weakened: Vec<String>,
    pub verified_removed: Vec<String>, pub verified_held: Vec<String> /* left, with why */ }
pub struct LibraryCheck { pub path: String, pub digest: String /* sha256:HEX */,
    pub verdict: String /* accepted: ... | unpinned | refused: ... */, pub loaded_from: Option<String> }
pub fn pin_entry(path: &Path) -> io::Result<LibraryPin>;  // the digest entry for an AI6 provider library
pub fn publisher_entry(path: &Path, publisher: PublisherKey) -> io::Result<LibraryPin>; // the same, by publisher
pub fn check_libraries(paths: &[PathBuf], pins: &PinSet) -> PluginScanSummary; // strict verdicts; loads and writes nothing
pub struct PublisherSigner;                               // Debug shows the public key only
impl PublisherSigner {
    pub fn generate(path: &Path) -> io::Result<Self>;     // PKCS#8 DER, 0600 on Unix, the directory's ACL on Windows; AlreadyExists when path exists
    pub fn read(path: &Path) -> io::Result<Self>;         // PKCS#8 DER or PEM, v1 or v2; no error holds the file's text
    pub fn public_key(&self) -> PublisherKey;
    pub fn sign(&self, path: &Path) -> io::Result<PathBuf>; // writes FILE.sig beside an AI6 provider library
}
pub fn log_scan(summary: &PluginScanSummary);           // refusals, weakenings and copies cleared, to stderr

// ai6::directorates::registry::bona_fides
pub enum PluginProfile { Strict, Report, Off }           // lowercase; FromStr; default_for(pins: bool)
pub enum KeepVerified { Newest(u64), All }               // default Newest(8); FromStr/Display/TOML: a count or "none" (All)
pub struct PublisherKey;                                 // "ed25519:" + 64 hex; from_bytes, as_bytes
pub enum Require { Either /* default */, Both }
pub struct LibraryPin { pub file: String, pub pin: Option<coqui_peer::PeerId>, pub publisher: Option<PublisherKey>, pub require: Require }
impl LibraryPin { digest(file, pin) / publisher(file, key) / with_pin / with_publisher / requiring_both }
pub struct PinSet { pub libraries: Vec<LibraryPin> /* [[library]] */ }
impl PinSet {
    pub fn new() -> Self;  with_library;
    pub fn from_toml(text: &str) -> Result<Self, String>;
    pub fn load(path: &Path) -> io::Result<Option<Self>>;   // None when the file is absent
    pub fn validate(&self) -> Result<(), String>;
    pub fn weakenings(&self) -> Vec<String>; // SHA-1 pins; a publisher whose signature alone is enough, one line per key
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
made for. A publisher entry without `require = "both"` admits anything that key signed for the
name, older builds included, with no revocation; `PinSet::weakenings` lists each such key. A
`strict` scan then removes the copies it did not hand to Guatiao, past the newest
`KeepVerified`: it holds `verified_dir/.lock` shared from before its first copy until its last
load, takes it exclusive to clear (not at all while another scan holds it), and keeps a shared
lock on each copy it loaded for the life of the process, which clearing asks about first. A
copy left is listed in `verified_held`; it never fails the scan. A pin covers the library file
only: a non-system library it imports is found by
the operating system's loader (on Windows also beside the executable, in the current
directory and on `PATH`). Under `report` libraries load from where they were found and each
unaccepted one is a `weakened` line; `off` checks nothing. The Station's check-in carries
`plugins_weakened` when the list is not empty.

`Plugins::handler_with(provider, settings)` builds a handler with an Agent's `handler_settings`
beside its provider's `[providers.<id>]` table: `HandlerConfig`'s `settings_toml` is one more key of
that table, so a provider with a configuration of its own declares it too.

## Command line

- `ai6 serve [--config PATH] [--provider LOC] [--registry LOC] [--state-dir PATH] [--station ID] [--listen 127.0.0.1:7081] [--plugin-path PATH]... [--plugin-profile strict|report|off]`
  (feature `cli`): run the Station from its bootstrap file with these flags over it. Its
  runtime settings are the provider's `station` record (`StationRuntime`, applied by
  `serve_with_options` over `ServeConfig`): `max_concurrent_missions`, the watchdog's
  `stall_timeout`, `loop_timeout`, `loop_min_lines`, `blocked_after`, `max_runtime`,
  `history` and `normalize_digits`, `roles` (default control and execution), `labels`,
  `terminal` (refused at Mission start for a JSON harness when it gives a terminal; without
  it `pipe` is preferred for a JSON harness, `pty` otherwise), `resources_every` (`0` leaves
  only the 300-second probe), `safe_house_caches` (replacing `DEFAULT_CACHE_DIRS`) and
  `safe_house_cache_age`; seconds may be fractional, and an invalid value exits 1. The pins
  are the provider's `plugin` records (`ServeConfig::with_plugin_policy` overrides them in
  code); the profile is `[station.plugins] profile` (`ServeConfig::with_plugin_profile`); verified
  copies go to the state directory's `plugins/verified/`, and `[station.plugins] keep_verified`
  (`ServeConfig::with_keep_verified`) says which old ones stay.
- `ai6 config show FAMILY [KEY] [--scope station:ID]` prints records as TOML;
  `ai6 config import DIR [--scope station:ID]` loads a configuration directory's records
  into a writable provider with `ConfigProvider::put_all` (checked whole, one transaction on
  SQL); `ai6 config export DIR` writes every family of every scope as TOML family files
  (`config::export`, owner-only on Unix). `ai6 config rm FAMILY KEY [--scope station:ID] [--force]`
  removes one record through `ConfigProvider::remove_on_host`: `config::references_to` reads every
  family at every scope first (a read error removes nothing) and refuses, listing them, what another
  record still names unless `--force` (a secret only through a `secrets` list entry, a `secret`
  value or a pool account of its name that reads a secret and names none; any other record through
  any equal string or table key, `handler`, `provider`, `model` and `effort` values aside); audited
  `roster.remove`/`roster.refused`, `cli:<user>`, before the removal. `--force` is reported; the
  provider's own checks still apply.
- `ai6 runner serve [--state-dir PATH] [--listen ADDR] [--plugin-path PATH]... [--pins PATH] [--plugin-profile P] [--keep-verified N|none] [--terminal ID] [--settings PATH] [--detach]`:
  run the field runner on this host until Ctrl-C or SIGTERM (provider libraries checked as
  the Station's, verified copies under its state directory); `--detach` starts it in the
  background and prints its `runner.json`. `ai6 runner connect [--state-dir PATH]` relays
  standard input and output to it over ACP, authenticated; exit 3 means no runner answered.
- `ai6 green-ink gate`: Claude Code's `PreToolUse` hook for shell commands (see Harness
  gate); the Claude handler passes it through `--settings`. Always exits 0.
- `ai6 plugins keygen --out PATH` writes a new publisher key (`PublisherSigner::generate`) and
  prints only its `publisher = "ed25519:..."` line, refusing a file that exists.
  `ai6 plugins sign --key PATH FILE...` writes `FILE.sig` beside each AI6 provider library and
  prints each path; the key comes only from a file.
- `ai6 plugins pin [--publisher KEY] FILE...` prints the `plugins.toml` entries that pin AI6
  provider libraries by SHA-256, or with `--publisher` (`ed25519:HEX` or a key file) accept what
  that key signed, with its weakening on standard error; anything else is refused with a
  non-zero exit. The runner's `--pins PATH` must name an existing file. With `--import
  [--scope station:ID] [--config PATH]` it stores them instead as `plugin` records of a
  writable provider in one `ConfigProvider::put_all`, each replacing the record of its file name
  and leaving the others, and prints `added|changed|unchanged FILE: ...` per library (`changed`
  shows old `->` new) and how many it wrote; a record already the same is not rewritten. A
  provider that takes no changes (a directory) and two libraries of one file name are refused
  before anything is written.
- `ai6 plugins check [--config PATH] [PATH...]` prints `PATH  DIGEST  VERDICT` per library named,
  else on `[station.plugins] paths`, against the `plugin` records of this host's Station under `strict`
  (`check_libraries`); it opens only the configuration provider, loads nothing, writes no copy,
  and exits 1 on any refusal or when no library was checked.
- `ai6 harness pin DIR` prints a working directory's harness manifest (`# sha256:..  PATH`
  per file) and the `pin = "sha256:..."` line for `[operation.ID.harness]`.
- `ai6 assistant pin-skill DIR` prints a skill directory's manifest
  (`ProjectManifest::read_tree`, `# sha256:..  PATH` per file) and the `pin = "sha256:..."`
  line for `[assistant.skills.NAME]`; a directory it cannot read exits non-zero.
- `ai6 theater provision` also takes `--config`, `--plugin-path` and `--plugin-profile`.
- `ai6 identity` creates or prints this Station's certificate fingerprint.
- `ai6 peer waiting` lists rejected peer certificates; `ai6 peer accept <ID>` pins one.
- `ai6 sleeper [--config PATH] list|authorize <OPERATION> <NAME>|revoke <OPERATION> <NAME>`:
  show and approve an Operation's standing orders. It reads the roster from the
  configuration provider and the Registry the bootstrap file names, like the Station.
- `ai6 config path [--config PATH]` prints the bootstrap file in use, or `none (defaults)`.
- `ai6 config public [--output PATH] [--config PATH]` writes the public `ai6.cfg` a web page reads
  (`bootstrap::public`): an allow list of the gateway's address, written as `[unex] url` (the host's
  `[unex] public_origin`, else its `[unex] url`) and, of `[oo]`, its tables' numbers and booleans
  and the words `[oo.appearance] theme` and `[oo.assets] cdn`. Other text under `[oo]` is left out and
  named on standard error; an address holding `@`, `?` or `#` (`PublicError::NotAnOrigin(key)`,
  naming `public_origin` or `url`) and this host's own file as the output are refused.
- `ai6 quarantine list|add <AGENT> <REASON> [--operation ID | --all]|release <AGENT> (--operation ID | --all)`:
  stop an Agent, or lift that, through the Registry; a Station applies the change within a
  scheduler tick.
- User-facing management goes through Universal Exports and the separate `ai6ctl` command.
