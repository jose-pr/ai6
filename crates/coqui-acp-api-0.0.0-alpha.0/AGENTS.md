# `coqui-acp-api` — API header

Header-file-style reference for the `coqui-acp-api` crate: every public item with its signature, contract and gotchas, so the crate can be used without reading its source. It ships inside the package and is self-contained. Development documentation lives with the source at <https://github.com/Boriken-Dev/coqui-acp>.

MSRV Rust 1.89. A default build depends on `guatiao` (with `provider`), `guatiao-channel` (both pinned by one revision) and `serde`: a host and every provider build against the same one, and a library built against another `guatiao::library::ABI_VERSION` is refused at load. Feature `contract` (off by default) adds the contract suite, `contract::Contract`, and the program `acp-harness-check`, with `serde_json`, `coqui-acp` and Guatiao's `load`. Version `0.0.0-alpha.0`, not published; the API may change in any release before 1.0.

Imports: everything is at the crate root, but the suite (`contract`). `coqui_acp_api::guatiao` is Guatiao, so a provider names one version of it (`#[provider(AcpHarness, crate = ::coqui_acp_api::guatiao)]`).

No `unsafe` code (`#![forbid(unsafe_code)]`). No process-wide state.

## Kinds

A kind is a C ABI. Its table carries a hash of every slot as spelled, so adding, removing or respelling a slot, or renaming a model, breaks every built provider:

| kind | table | `FLOOR_HASH` | who implements it |
| --- | --- | --- | --- |
| `acp_harness` | `AcpHarnessVtable` | `4111118208` | the provider |
| `channel` | `ChannelVtable` | `3787630980` | the provider (Guatiao's, re-exported) |
| `channel_sink` | `ChannelSinkVtable` | `4115266557` | the host (Guatiao's, re-exported) |
| `process_spawner` | `ProcessSpawnerVtable` | `4038904472` | the host |
| `spawned_process` | `SpawnedProcessVtable` | `3306532587` | the host |
| `account_keeper` | `AccountKeeperVtable` | `708146915` | the host |
| `login_prompter` | `LoginPrompterVtable` | `2606822037` | the host |

The package holds all seven as data in `include/coqui_acp_api.kinds.json`, every model's JSON Schema in `include/coqui_acp_api.schemas.json`, and the kinds for Python in `python/coqui_acp_api_kinds.py` (written by `python/write_kinds.py`).

**How the kind changes.** Until its first release every method a harness has to answer is required, and a method added or changed is a break: every provider rebuilds, and the hash moves. No compatibility code is kept: no default so that an older provider loads, no deprecated alias, no reading of an older request's shape. After its first release the kind grows only by appending, as Guatiao provides for released kinds.

**The hash does not cover a model's fields**: a model crosses as a map, and a reader generated in another language skips a key it does not know, so a renamed field can be dropped with no error. Two things gate that. `tests/kinds.rs` pins every model's keys on the declaring side, so a field renamed here fails it. And every provider (in Rust, in Python, the translators) runs the contract suite at each change of this crate's revision or of Guatiao's: a provider reading a field under its old name fails a check.

```rust
#[guatiao::kind(name = "acp_harness")]
pub trait AcpHarness: Send + Sync {
    fn describe(&self) -> Result<HarnessInfo, ProviderError>;
    fn accounts(&self) -> Result<AccountCatalog, ProviderError>;
    fn start(&self, request: StartRequest, spawner: Object<dyn ProcessSpawner>, sink: Object<dyn ChannelSink>,
             keeper: Object<dyn AccountKeeper>) -> Result<Object<dyn Channel>, ProviderError>;
    fn refusal(&self, requirements: Vec<Requirement>, environment: Secret<Environment>, account: Option<AccountRef>)
        -> Result<Option<String>, ProviderError>;
    fn program(&self) -> Result<Option<String>, ProviderError>;
    fn import_session(&self, session: SessionImport, spawner: Object<dyn ProcessSpawner>, keeper: Object<dyn AccountKeeper>)
        -> Result<ImportReport, ProviderError>;
    fn export_session(&self, export: SessionExport, spawner: Object<dyn ProcessSpawner>, keeper: Object<dyn AccountKeeper>)
        -> Result<ExportPage, ProviderError>;
    fn models(&self, request: ModelsRequest, spawner: Object<dyn ProcessSpawner>, keeper: Object<dyn AccountKeeper>)
        -> Result<ModelListing, ProviderError>;                                                       // default: unknown
    fn session_record(&self, query: SessionQuery) -> Result<Option<SessionRecordFile>, ProviderError>; // default: None
    fn usage(&self, query: UsageQuery, spawner: Object<dyn ProcessSpawner>, keeper: Object<dyn AccountKeeper>)
        -> Result<UsageReport, ProviderError>;                                                        // default: unsupported
    fn login(&self, request: LoginRequest, spawner: Object<dyn ProcessSpawner>, prompter: Object<dyn LoginPrompter>)
        -> Result<LoginOutcome, ProviderError>;
    fn renew(&self, request: RenewRequest, spawner: Object<dyn ProcessSpawner>) -> Result<RenewReport, ProviderError>;
    fn non_defaults(&self) -> Result<Vec<NonDefault>, ProviderError>;                               // default: empty
}
```

Every method is synchronous; a host calls them from a thread of its own. Every method answers for the provider **as configured**: a host builds an instance from the provider's settings (`offer.instantiate(&settings)`) and asks it, so `describe` and `refusal` are a configured harness's answers. The nine without a default are the ones a host acts on for every harness (a harness that cannot import says so by refusing, with its reason; one with no way to sign in answers `login` `GUATIAO_ERR_UNSUPPORTED`); the four defaults are true answers for a harness with nothing to say (no way to list models, no record kept, no usage to report, settings its schema shows in full). Every object a method is handed (`spawner`, `sink`, `keeper`, `prompter`) is the provider's from then on, and it destroys each, even when it refuses.

- `describe`: who the harness is and what its `initialize` will advertise, with nothing started. Cheap, the same on every call.
- `accounts`: the kinds of account the harness takes, the ways to sign in to make one, and every variable it reads for a credential ("Accounts", below). Cheap, the same on every call.
- `start`: one ACP connection. The host is the client, the provider the agent. `spawner` and `sink` are handed over: the provider destroys both, even when it refuses, and the spawner by the time it calls `closed`. It returns once the channel takes messages; the program may start later, and a failure then is answered in ACP before the provider closes. It refuses exactly when `refusal` would for the request's requirements, with the same status: `GUATIAO_ERR_UNSUPPORTED` for a requirement it cannot hold, `GUATIAO_ERR_BAD_VALUE` for a request it cannot read. It never starts with a requirement left out. With `request.account` it applies the account first ("Accounts"), and `keeper` is handed over like the spawner: the provider reports through it what happens to the account's login while the connection runs, and destroys it by the time it calls `closed`.
- `refusal`: why the harness could not hold a connection to **all of** `requirements` together, started with `environment`, in a sentence; `None` when it can. An empty environment asks about the configuration alone and is answered. `GUATIAO_ERR_BAD_VALUE` for requirements `start` would refuse as unreadable, and for an `account` (an `AccountRef`: a grant without its credentials) of a kind it does not list. A host asks before it starts a connection and passes over a harness that would refuse.
- `program`: the program the harness starts through the spawner (a path, or a name looked up on the `PATH` where it runs, the start's environment's `PATH` when that sets one); `None` for one that starts none.
- `import_session`: seeds a session at the host's UUID from a history (below); an empty history seeds a fresh one. A UUID the provider already holds is `GUATIAO_ERR_BAD_VALUE`, never overwritten: a host moving a session back to a harness that still holds it imports at a new UUID or into a fresh store. Answers an `ImportReport`. Refuses `GUATIAO_ERR_UNSUPPORTED`, with the reason, when the harness cannot import at all or does not read the history's version; `GUATIAO_ERR_BAD_VALUE` for an id that is not a UUID or already names a session, or a history it cannot read. Never a session half seeded and never a silent fallback: a harness with no native session to seed (a generic ACP agent) may import by replaying the history as context in its first prompt, and then says so in the report's losses (`replayed_as_prompt`; `ImportReport::replayed()` reads it, so a host tells a native import from a replay). A replay is not a session of the harness: the context reaches the model in the first prompt only, and the report's other losses say what was cut or not carried.
- `export_session`: one page of the history of a session it reported, by its UUID, in the standard format, as it is. A page is whole lines, each ended by a line feed, never more than `max_bytes`: the caller's memory bound, since one line can hold a whole tool output or an attachment. A next line longer than `max_bytes` (line feed included) is refused `GUATIAO_ERR_BAD_VALUE` with the message `line too large: <N> bytes` (`LINE_TOO_LARGE`, read back with `line_too_large(message) -> Option<u64>`), so the host asks again with a `max_bytes` of at least N or stops and says why. Only the first page starts with the header, which carries the format's version and the digest of the whole body. `next` names the page after; absent, the export is complete; the pages joined are the export of one page as large as the whole. `after` is opaque, the provider's own. `GUATIAO_ERR_NOT_FOUND` for a session it does not have; `GUATIAO_ERR_BAD_VALUE` for an `after` it never gave or one the session has changed since (the host starts again from the first page); `GUATIAO_ERR_UNSUPPORTED`, with the reason, when it cannot export.
- `models`: what the account behind `request` (a `ModelsRequest`: `environment`, `account`) may use, with no model turn, or why it cannot say. A `ModelInfo` carries the model's `name` (with its version where the provider knows it), `resolved` (the model an alias or default resolves to), `version` (`Opus 5.5`), its own `efforts` (empty: none listed) `default_effort` and `is_default` (used when none is named). `spawner` and `keeper` are handed over as `import_session`'s are.
- `session_record`: where the harness keeps its own record of a session, by the UUID it reported.
- `login`, `renew`: sign in to make an account, and renew one with no model turn ("Accounts").
- `usage`: what the account behind `query` has used and has left, with no session and no model turn, as a `UsageReport` ("Usage", below). Cheap: a host may poll it as often as it likes. A harness that learns usage only during a turn answers the newest report its connections saw for `query.account`'s `id`, with its `observed_at`. A provider that runs a program to ask (Codex's `account/rateLimits/read`) runs it through `spawner`, on the account's directory, and destroys it, as `import_session` does. The default, `UsageReport::unsupported("this harness reports no usage")`, is a true answer. Never carries a value of the environment.
- `non_defaults`: how a configured harness departs from its defaults in ways its settings' schema cannot show (the detail of a rule a setting holds as text). The rest a host reads from the schema itself, with Guatiao's `schema::differences`.

**Nothing a provider answers carries a value of an environment or a credential**: not an answer, a refusal, an error, a log line or an argument vector. `Secret<Environment>` prints `[redacted]`, and its schema marks it `writeOnly` and `x-sensitive`. The copies Guatiao's proxy makes of an argument while converting it are not wiped.

A provider in-process is trusted code: the kind cannot check that it keeps a requirement, and a requirement it accepts is its promise.

### The channel

`Channel` and `ChannelSink` are Guatiao's (`guatiao-channel`), with every rule of theirs; what this kind adds is what the bytes are.

- **One message is one whole JSON-RPC 2.0 message**, as UTF-8 JSON text, either way: a request, a notification or a response, never a batch and never part of one. Whitespace around it is allowed, so a line read from a stdio agent, newline included, is a message.
- **ACP is agreed on the channel.** The host sends `initialize` first; the version is what the answer says. `describe` is advance notice; `initialize` governs.
- **Back-pressure blocks**, as the channel's rules say. A provider's intake never waits on its agent's work, so a `session/cancel` reaches a running turn; it may wait on the host's sink. A host's `receive` never waits for its own `send`.
- **Cancelling a turn is ACP's `session/cancel`**, a message like any other: the turn ends with `stopReason` `cancelled`. `$/cancel_request` passes through the same way. The kind has no cancellation of its own; a `send` waiting for room ends only with `close`, answered `GUATIAO_ERR_GONE`.
- **What is not a message is answered, and the connection goes on**: the provider answers text that is not JSON `-32700`, a batch or anything not one JSON-RPC message `-32600` (id null), a request whose `method` is not text `-32600`, params of the wrong type `-32602`, an unknown method `-32601`; an unknown notification is ignored. A host receiving text that is not JSON, or a batch, refuses it from `receive` with `GUATIAO_ERR_BAD_VALUE`, which ends the connection.
- **Closing.** `close(status, reason)` and `closed(status, reason)` carry a reason: UTF-8 text lent for the call, empty when there is none, for logs and people only; nothing parses it. Dropping the channel is `close(GUATIAO_OK, "")`. On the host's `close` the provider answers every request it still holds (a running prompt with `stopReason` `cancelled`, any other with an error), stops what it started (its whole process tree), destroys the spawner, then calls `closed` with the host's status, passing the host's reason on or giving its own. On its own end it does the same and calls `closed` with its status; **a provider that fails says why in the reason**, which is how a failure between requests reaches the host. After `closed`, the next `send` answers `GUATIAO_ERR_GONE`.

| the end | `closed` status |
| --- | --- |
| the host closed with `s` | `s` |
| the agent ended normally, or the provider ended the connection | `GUATIAO_OK` |
| the agent exited with a failure or could not start | `GUATIAO_ERR_INTERNAL`, after answering what is pending with errors saying why; the reason says it too |
| a message over the provider's size limit | `GUATIAO_ERR_BAD_VALUE`; the `send` that carried it answers the same |
| the sink refused a message | the sink's status, its message as the reason |

A message's size limit is each side's own setting; the kind sets none. Requests either side was waiting on end unanswered when the other closes; a host does not resend them.

### The spawner

```rust
#[guatiao::kind(object, name = "process_spawner")]
pub trait ProcessSpawner: Send + Sync {
    fn describe(&self) -> Result<SpawnerInfo, ProviderError>;
    fn spawn(&self, spec: SpawnSpec) -> Result<Object<dyn SpawnedProcess>, ProviderError>;
    fn write_file(&self, path: &str, bytes: Secret<SecretBytes>, private: bool) -> Result<(), ProviderError>;
    fn read_file(&self, path: &str, max_bytes: u64) -> Result<Secret<SecretBytes>, ProviderError>;
    fn file_info(&self, path: &str) -> Result<Option<FileInfo>, ProviderError>;   // None: not there
    fn remove_file(&self, path: &str) -> Result<(), ProviderError>;               // not there: no error
    fn notice(&self, notice: HarnessNotice) -> Result<(), ProviderError>;         // what the connection tells the host
}
#[guatiao::kind(object, name = "spawned_process")]
pub trait SpawnedProcess: Send + Sync {
    fn write(&self, bytes: &[u8]) -> Result<(), ProviderError>;           // all of it; GONE once input closed or ended
    fn close_input(&self) -> Result<(), ProviderError>;
    fn read(&self, dst: &mut [u8], timeout_ms: u64) -> Result<i64, ProviderError>;        // stdout, or the terminal
    fn read_error(&self, dst: &mut [u8], timeout_ms: u64) -> Result<i64, ProviderError>;  // stderr when Separate; else 0
    fn resize(&self, size: TerminalSize) -> Result<(), ProviderError>;    // UNSUPPORTED without a terminal
    fn wait(&self, timeout_ms: u64) -> Result<ProcessExit, ProviderError>; // 0 polls
    fn kill(&self) -> Result<(), ProviderError>;                          // the whole tree; idempotent
    fn link(&self) -> Result<LinkState, ProviderError>;
}
pub const READ_TIMED_OUT: i64 = -1;   // nothing within the timeout
pub const READ_LINK_DOWN: i64 = -2;   // the link is down; the process may run on, its output kept
```

The host hands a spawner to every `start`: where and how the harness's programs run, on this machine, over SSH, in a container or a virtual machine. A provider starts its programs through it, or ignores it and destroys it. `read` answers the byte count, `0` once no more can come, or one of the two codes; it never skips or repeats a byte across a dropped link. Destroying a process kills its tree. `spawn` refuses `GUATIAO_ERR_UNSUPPORTED` what `describe` says it cannot give, `GUATIAO_ERR_NOT_FOUND` a missing program, `GUATIAO_ERR_BAD_VALUE` a spec it cannot read. A local spawner is `coqui_acp_harness::LocalSpawner` (feature `kind` of `coqui-acp-harness`).

```rust
#[non_exhaustive] pub struct SpawnSpec { pub program: String, pub args: Vec<String>, pub cwd: Option<String>,
    pub environment: Secret<Environment>, pub clear_environment: bool, pub remove_environment: Vec<String>,
    pub terminal: Option<TerminalSize>, pub error_output: ErrorOutput }
pub fn SpawnSpec::new(program) -> SpawnSpec;  .arg(a)  .cwd(d)  .environment(env)  .clear_environment(b)  .terminal(size)  .error_output(o)
#[non_exhaustive] pub enum ErrorOutput { Separate /* default */, Merged, Discarded }   // "separate" | "merged" | "discarded"
#[non_exhaustive] pub struct TerminalSize { pub rows: u16, pub cols: u16 }   TerminalSize::new(rows, cols)
#[non_exhaustive] pub struct SpawnerInfo { pub os: String, pub arch: String, pub local: bool, pub terminal: bool,
    pub separate_error: bool, pub survives_link_loss: bool, pub kills_escaped: bool, pub ends_with_host: bool,
    pub files: bool }
#[non_exhaustive] pub struct FileInfo { pub len: u64, pub modified_ms: i64 }   FileInfo::new(len, modified_ms)   // Unix epoch
pub fn SpawnerInfo::new(os, arch, local: bool) -> SpawnerInfo   // the rest false
#[non_exhaustive] pub struct ProcessExit { pub running: bool, pub code: Option<i64>, pub signal: Option<i64> }
ProcessExit::running();  ProcessExit::ended(code, signal)
#[non_exhaustive] pub struct LinkState { pub up: bool, pub drops: u64 }   LinkState::up()
#[non_exhaustive] pub struct HarnessNotice { pub kind: NoticeKind, pub text: String }   HarnessNotice::new(kind, text)
#[non_exhaustive] pub enum NoticeKind { Exited, Notice, Fault }   // "exited" | "notice" | "fault"; .as_str()
```

**Notices.** `notice` is how a connection tells the host what is no message of the client's, so the host can log it: `Exited` (the harness ended mid-session; the text says how it ended and never what it last wrote), `Notice` (the harness's own, such as a version it was not tested with) and `Fault` (a part of the provider's plumbing failed, such as a permission answer that could not reach the harness). One line of text, with no client text and no value of the environment or the credentials. The provider calls it from threads of its own at any time while the connection runs, and ignores a failure; a spawner with no use for it returns `Ok`. The providers built on `coqui-acp-harness`'s `Serving` route `Settings::on_notice` (now `Fn(NoticeKind, &str)`) to it, holding the spawner weakly, so a notice after the spawner is destroyed is dropped.

**A provider never weakens a spec to fit a spawner.** A spawner that cannot keep standard error apart says so (`separate_error` false) and refuses `ErrorOutput::Separate` with `GUATIAO_ERR_UNSUPPORTED`; a provider that needs it (a JSON-lines harness reads its standard output as protocol) refuses in turn, with the reason, and never falls back to `Merged`. The same holds for a terminal.

**The file methods** serve one private directory the host granted the spawner: an account's `state_dir`, or a login's. **The host resolves every path** against that directory and never trusts a provider's. `path` is relative to it, and is `GUATIAO_ERR_NOT_FOUND`, on a spawner with no such directory too, when it is empty, absolute, has a `..` part, has a `:` (a drive or a stream), has a part ending in a dot or a space or one a Windows device answers to (`NUL`, `COM1`, `CONIN$` ...), leads out of the directory through a link, names a file that is not a regular file (a pipe), or names a file with another name for the same data (a hard link, where the system lets the host see it: Unix, not Windows). What is opened is checked after it is opened, so a link planted after a check is not followed out. `write_file` makes the directories on the way and replaces a file; on Unix it makes the file in a temporary name beside it, owner-only (mode 0600) when `private`, and renames it into place, so there is no moment it holds bytes and others can read it; on Windows `private` adds nothing and **the granted directory's ACL is the guard**. `read_file` of a file longer than `max_bytes` is `GUATIAO_ERR_BAD_VALUE`, of one that is not there `GUATIAO_ERR_NOT_FOUND`. A spawner that cannot (a transport without files) says `files` false and refuses all four `GUATIAO_ERR_UNSUPPORTED`; a provider whose account kind needs a directory then refuses that kind. `PrivateDir` is the one implementation, on this machine, that `LocalSpawner` and the suite's spawner share; `contract::spawner_files(spawner, root)` holds any host's spawner to the same rules. File bytes cross as Guatiao's bytes kind in `Secret<SecretBytes>` (wiped when dropped; `Debug` shows the length only).

`os` and `arch` are spelled as `std::env::consts` spells them: which rules the programs' paths follow (`PathStyle::of_os`). `kills_escaped` says whether `kill` reaches processes that left the program's group or session; `ends_with_host`, whether the tree ends when the host dies without a kill.

## Requests and answers

What a host sends (`StartRequest`, `Requirement`, `SessionQuery`, `SessionImport`, `SessionExport`, and the provider's `SpawnSpec`) **refuses a key it does not declare** (`GUATIAO_ERR_BAD_VALUE`), so a narrowing a newer side adds is never dropped by an older one: a new key is a break. What is answered is read leniently.

```rust
#[non_exhaustive] pub struct StartRequest {
    pub environment: Secret<Environment>,   // over the configuration's; sensitive
    pub working_roots: Vec<String>,         // every session's directories lie in one; empty adds none
    pub requirements: Vec<Requirement>,     // what the connection is held to; each name once
    pub account: Option<AccountGrant>,      // the account to run on; its `id` is echoed into every usage report, never interpreted
    pub cache_lifetime: CacheLifetime,      // how long the prompt cache should live; Default
}
StartRequest::new();  StartRequest::builder() -> StartRequestBuilder   // .environment(env) .working_root(r) .requirement(r) .account(grant) .cache_lifetime(l) .build()
#[non_exhaustive] pub enum CacheLifetime { Default, Short, Long }   // "default" "short" "long"; the harness's own, about five minutes, about an hour

#[non_exhaustive] pub struct Requirement { pub name: String, pub value: Option<Value> }
Requirement::new(name);  Requirement::with_value(name, value);  requirement.narrowing() -> Result<Narrowing, RequirementError>
#[non_exhaustive] pub enum Narrowing { ReadOnly, NoCommands, AnswerOnly, NoProjectSettings, NoProjectMcp,
    FilesUnder(Vec<String>), ReadOnlyPaths(Vec<String>), DeniedPaths(Vec<String>), Network(Vec<String>),
    SandboxedScripts(Vec<String>), OnlySkills(Vec<String>) }
Narrowing::read(&Requirement);  Narrowing::read_all(&[Requirement]) -> Result<Vec<Narrowing>, RequirementError>   // a repeated name refused
impl From<Narrowing> for Requirement
#[non_exhaustive] pub enum RequirementError { Unknown { name }, BadValue { name, why }, Repeated { name } }
error.status()   // UNSUPPORTED for Unknown, BAD_VALUE otherwise; impl From<RequirementError> for ProviderError
pub const NAMES: [&str; 11];

pub struct Environment;   // BTreeMap<String, String> inside; Debug names the variables only; Wipe zeroes names and values
Environment::new(); .insert(name, value); .get(name); .iter(); .names(); .len(); .is_empty(); FromIterator; From<BTreeMap<String, String>>

#[non_exhaustive] pub struct HarnessInfo { pub title: String, pub versions: Vec<u16>, pub takes: Takes, pub resumes_wider: bool,
    pub usage: UsageSupport, pub cache_lifetime: Option<String> }
HarnessInfo::new(title, versions)
#[non_exhaustive] #[derive(Default)] pub struct Takes { pub images: bool, pub audio: bool, pub embedded_context: bool,
    pub mcp_http: bool, pub mcp_sse: bool, pub load: bool, pub resume: bool, pub additional_directories: bool, pub system_prompt: bool }
#[non_exhaustive] pub struct ModelListing { pub known: bool, pub unknown_because: Option<String>, pub models: Vec<ModelInfo> }
ModelListing::unknown(reason);  ModelListing::of(models)
#[non_exhaustive] pub struct ModelInfo { pub id: String, pub name: Option<String>, pub description: Option<String>, pub resolved: Option<String>, pub version: Option<String>, pub efforts: Vec<String>, pub default_effort: Option<String>, pub is_default: bool }   ModelInfo::new(id)
#[non_exhaustive] pub struct SessionQuery { pub session_id: String, pub working_directory: String, pub environment: Secret<Environment> }
SessionQuery::new(session_id, working_directory).environment(env)
#[non_exhaustive] pub struct SessionImport { pub session_id: String, pub working_directory: String, pub history: String,
    pub environment: Secret<Environment>, pub account: Option<AccountGrant> }
SessionImport::new(session_id, working_directory, history).environment(env).account(grant)
#[non_exhaustive] pub struct SessionExport { pub session_id: String, pub working_directory: String,
    pub environment: Secret<Environment>, pub account: Option<AccountGrant>, pub after: Option<String>,
    pub max_bytes: u64 /* DEFAULT_PAGE_BYTES */ }
SessionExport::new(session_id, working_directory).environment(env).account(grant).after(next).max_bytes(n)
pub const DEFAULT_PAGE_BYTES: u64 = 1 << 20;
pub const LINE_TOO_LARGE: &str = "line too large: ";   // a refusal's message: "line too large: <N> bytes"
pub fn line_too_large(message: &str) -> Option<u64>   // N, from such a message
#[non_exhaustive] pub struct ExportPage { pub text: String, pub next: Option<String> }   ExportPage::new(text, next)
#[non_exhaustive] #[derive(Default)] pub struct ImportReport { pub seeded_from: Option<SeedPoint>, pub losses: Vec<HistoryLoss>, pub compacted: bool }
ImportReport::new();  ImportReport::replayed(&self) -> bool   // `replayed_as_prompt` among the losses
pub const REPLAYED_AS_PROMPT: &str  // "replayed_as_prompt"
#[non_exhaustive] pub struct SeedPoint { pub line: u64, pub digest: Option<String> }   SeedPoint::line(n)   // the header is line 1
#[non_exhaustive] pub struct HistoryLoss { pub kind: String, pub count: u64, pub detail: Option<String> }   HistoryLoss::new(kind, count)
// kind: `thinking`, `tool_as_note`, `attachment_by_digest`, `replayed_as_prompt`, as the format's loss report names them
#[non_exhaustive] pub struct SessionRecordFile { pub path: String, pub format: String }   // `claude-code-jsonl`, `codex-rollout-jsonl`
#[non_exhaustive] pub struct NonDefault { pub setting: String, pub value: String, pub default: String }   NonDefault::new(setting, value, default)
```

`usage`: what the harness can report (`UsageSupport { sources, spend, needs_turn }`; empty `sources` is none), which `initialize` agrees with: a harness that reports any advertises the usage capability, one that reports none does not. `needs_turn` is true for a harness that learns its usage only during a turn (Claude Code): `_coqui_acp/usage` then answers what the connection last saw, a host keeps a timed retry instead of polling, and the capability carries `"needs_turn": true` (absent means false). False means a read re-reads the account, so a host may poll it (Codex). `needs_session` is true for a harness whose figures come from the process a session starts and need no turn (Antigravity): with a session open a read re-reads, with none it answers what the connection last saw and starts nothing, and the capability carries `"needs_session": true` (absent means false).

`cache_lifetime` (`HarnessInfo`) says in a sentence what the harness does with `StartRequest::cache_lifetime` (Claude Code sets `CLAUDE_CODE_PROMPT_CACHE_TTL` and `CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL`); absent, it has no such control and ignores the hint. The hint never refuses a start.

`resumes_wider`: a session held to requirements can be reopened (`session/load` or `session/resume`) on a later connection started with fewer, and then has what that connection allows. A host that changes what a session may do starts a new connection with the new requirements and reopens the session there.

### Requirements

**Requirements only narrow.** Each takes something away from what the provider's configuration allows; none grants anything, and a provider never reads one as permission. A name the provider does not know is refused; a known name with a value it does not take (one missing included), or a name given twice, is a bad value. What a session is given (more directories, a plugin, context) travels in ACP: `additionalDirectories` and `_meta` on `session/new`.

| name | value | the connection's sessions |
| --- | --- | --- |
| `read_only` | none | create, change and remove no file, by the agent or what it runs; commands may still run where the file system is read-only to them |
| `no_commands` | none | start no command or program: no shell, script or terminal, sandboxed or not |
| `answer_only` | none | only answer from what the host gives: no tool of any kind (no file read or write, command, MCP tool from any source, web search or fetch); no settings, MCP servers, skills, hooks or instruction files, neither the session's directories' nor the user's own global ones, so the model sees only the prompt and what the host sends; what the harness runs to answer is sandboxed, writes no file and reaches no network but the harness's own model endpoint. Holds every name above and below that takes no value |
| `no_project_settings` | none | read no configuration from the session's directories or their parents (settings, permission rules, hooks, commands, sub-agents); instruction files (`AGENTS.md`, `CLAUDE.md`) still load |
| `no_project_mcp` | none | start no MCP server declared in the session's directories or their parents |
| `files_under` | list of paths | create, change and remove files only under these; an empty list allows none |
| `read_only_paths` | list of paths | create, change or remove nothing under these |
| `denied_paths` | list of paths | read, list, create, change or remove nothing under these |
| `network` | list of `host`, `host:port`, `*.domain`, `[v6]:port` | reach only these, besides the harness's own model endpoint; an empty list reaches nothing else |
| `sandboxed_scripts` | list of program names | run only these interpreters, inside the harness's sandbox; no other command |
| `only_skills` | list of skill names | load at most these skills; an empty list loads none |

A path in a value is absolute (either platform's form) with no `.` or `..` part. A spending limit is not a requirement: the host keeps its own.

### Working roots and session ids

```rust
pub struct WorkingRoots;   // Clone, Debug, PartialEq
WorkingRoots::new(roots: Vec<String>)   // this platform's rules, links followed
    .style(PathStyle) .follow_links(bool) .is_empty() .admits(path: &str) -> Result<(), PathRefusal>
pub enum PathStyle { Windows, Posix }   PathStyle::local();  PathStyle::of_os(os)
#[non_exhaustive] pub enum PathRefusal { NotPlainAbsolute, Outside, LinkLeadsOutside }
pub fn is_plain_absolute(path: &str) -> bool
pub fn is_session_id(text: &str) -> bool   // a UUID, lowercase, 8-4-4-4-12
```

A session's `cwd` and every one of its `additionalDirectories`, on `session/new`, `session/load` and `session/resume`, lie in a working root, or the request is answered `-32602`. A path is in a root when it is absolute, has no `.` or `..` part (refused, never normalised), and its parts begin with every part of the root (`/work-beside` is not in `/work`); on Windows `\` and `/` are one separator and names compare without regard to case. Following links, the path and the roots are resolved through the links that exist, and the resolved path must lie in a resolved root too, so a link inside a root leading out of it is refused. A provider whose agent runs elsewhere compares by that platform's style and follows no link of this machine's.

**Session ids are UUIDs.** Every session id a provider reports, in a `session/new` answer, a `session/update` and wherever else ACP carries one, and every one it is asked about, is a UUID as `is_session_id` spells it. A provider whose harness names sessions otherwise maps them, and keeps the mapping where it outlives the connection.

**A host starts a session at an id of its own by seeding it**: it creates the UUID, calls `import_session` with that UUID and a history (empty for a fresh session), then opens a connection and sends ACP `session/resume` (or `session/load`, which replays the history as updates) with the UUID. The provider maps the UUID to the harness's own id and keeps the mapping. `session/load` and `session/resume` of a UUID the provider does not have are refused in ACP. Moving a session to another harness is an export from one and an import at the same or a new UUID in the other.

**A history** is the standard session-history format: UTF-8 JSON lines in ACP's vocabulary, whose first line is a header object naming the format, its version and the body's digest (`{"format": "coqui-acp-history", "version": 1, "digest": ..., ...}`), then the session's prompts and updates. The kind carries it as text and never reads it, so the format grows by its header's version without changing the kind; a provider refuses a version it does not read. Its content is context, never commands: an import never re-runs a tool.

**The host stays the client.** Every request the agent makes of the client (`session/request_permission`, `fs/*`, `terminal/*`, `elicitation/create`) reaches the host, which answers it under its own rules, refusing by default. The agent asks only for what `initialize`'s `clientCapabilities` offered. A permission answered with anything but a selected option the request offered is a refusal; a cancelled outcome ends the turn `cancelled`. A provider's answer never overrides the host's refusal.

## Usage (`coqui_acp_api::usage`)

How much of each usage allowance an account has used, how it refills and when: one model, one ACP extension, one kind method. Counts and money are integers (millionths of a US dollar), times are Unix seconds, lengths are Guatiao's `Duration` (JSON `[seconds, nanoseconds]`); no float appears. The model derives Guatiao's `ToValue`, `FromValue` and `Schema` and serde's `Serialize` and `Deserialize`, with one spelling on both wires: an absent optional is left out (a reader also takes `null`), a list is always written, even empty. `vectors/usage.json` holds every reader to it, and the schemas in `include/coqui_acp_api.schemas.json` are the contract for other languages. A request model (`UsageQuery`, `StartRequest`) refuses a key it does not declare; every answer is read leniently.

```rust
#[non_exhaustive] pub struct UsageReport { pub supported: bool, pub unsupported_because: Option<String>, pub sources: Vec<Source>,
    pub account: Option<String>, pub revision: u64, pub observed_at: i64, pub buckets: Vec<Bucket>, pub limit: Option<LimitHit>,
    pub spend: Option<Spend> }
UsageReport::new();  UsageReport::unsupported(reason);  UsageReport::fraction_permille(&Bucket) -> Option<u16>
UsageReport::inferred(LimitHit) -> UsageReport     // a host's report for an agent that reports nothing: sources [inferred], one exhausted `inferred` bucket of unknown coverage, no figures
pub const DEFAULT_LIMIT_PHRASES: &[&str];                     // RESOURCE_EXHAUSTED, "Resource has been exhausted", "exhausted your capacity", "Error 429", "rate limit", "usage limit", "quota", throttling_error
pub fn infer_limit(text: &str, phrases: &[&str]) -> Option<LimitHit>   // a failed turn's text read as a limit; see "Inferred limits"
#[non_exhaustive] pub struct Bucket { pub id: String, pub label: Option<String>, pub models: Coverage, pub unit: Unit,
    pub used: Option<u64>, pub limit: Option<u64>, pub remaining: Option<u64>, pub refill: Refill, pub status: Status,
    pub observed_at: i64, pub source: Source, pub evidence: Option<String> }
Bucket::new(id);  bucket.resets_in(now: i64) -> Option<Duration>
#[non_exhaustive] pub enum Coverage { All, Unknown, Only { models: Vec<String> }, Other }       // JSON {"kind": "only", "models": [..]}
#[non_exhaustive] pub enum Unit { Tokens, Requests, MicroUsd, Permille, Other(String) }          // "tokens" "requests" "micro_usd" "permille"
#[non_exhaustive] pub enum Refill { FixedWindow { length: Option<Duration>, resets_at: Option<i64> },
    TokenBucket { amount: u64, per: Duration, max: u64 }, Never, Unknown, Other }                // kinds fixed_window, token_bucket, none, unknown
#[non_exhaustive] pub enum Status { Ok, Warning, Exhausted, Unknown, Other(String) }
#[non_exhaustive] pub enum Source { Reported, Inferred, Other(String) }
#[non_exhaustive] pub struct LimitHit { pub bucket: Option<String>, pub resets_at: Option<i64>, pub retry_after: Option<Duration>,
    pub source: Source, pub text: Option<String>, pub observed_at: i64 }                         // LimitHit::new(observed_at)
#[non_exhaustive] pub struct Spend { pub session_id: String, pub input: u64, pub output: u64, pub cache_read: u64,
    pub cache_write: u64, pub reasoning: Option<u64>, pub cost_micro_usd: Option<u64>, pub by_model: Vec<ModelSpend> }   // Spend::new(session_id)
#[non_exhaustive] pub struct ModelSpend { pub model: String, pub input: u64, pub output: u64, pub cache_read: u64,
    pub cache_write: u64, pub cost_micro_usd: Option<u64> }                                      // ModelSpend::new(model)
#[non_exhaustive] pub struct UsageSupport { pub sources: Vec<Source>, pub spend: bool, pub needs_turn: bool, pub needs_session: bool }   // none(); new(sources, spend); .needs_turn(bool); .needs_session(bool); reports()
#[non_exhaustive] pub struct UsageQuery { pub environment: Secret<Environment>, pub account: Option<AccountGrant> }   // new(); .environment(env); .account(grant)
pub const USAGE_CAPABILITY: &str = "dev.boriken.coqui.acp/usage";   pub const USAGE_METHOD: &str = "_coqui_acp/usage";
pub const USAGE_NOTIFICATION: &str = "_coqui_acp/usage_update";     pub const LIMIT_META_KEY: &str = "dev.boriken.coqui.acp/limit";
pub const USAGE_VERSION: u32 = 1;
```

- **`supported: false` is a real answer** (this harness reports no usage, and `unsupported_because` says why); `supported` with no buckets is another, nothing reported yet. `sources` says which kinds of figure the harness can give at all: `reported` (its own structured figure) or `inferred` (read from an error's words, a 429 or a refusal, by the provider or the host; the bucket's `evidence` holds the words, for logs, and nothing parses it). A host knows from `sources` that an agent never reports figures.
- **A bucket** names the models it covers (`unknown` counts as covering every model, so an exhausted bucket of unknown coverage stops the account for each of them), what it counts (`unit`), `used`, `limit` and `remaining` as the harness gave them, and how it refills. A provider fills only what it was told and never derives one figure from another. The fraction a reader uses is `UsageReport::fraction_permille`: `used / limit`, else `(limit - remaining) / limit`, in thousandths, at most 1000, `None` without a limit. `Unit::Permille` is a percentage-only bucket: `limit` is 1000 and `used` thousandths, so the arithmetic is the same for every unit. `id` is stable per harness and account (`five_hour`, `seven_day`, `seven_day_opus`, `codex:primary`, `inferred`).
- **Refill.** `FixedWindow` resets all at once when the window ends, at `resets_at` (a subscription's five hours or week); `TokenBucket` adds `amount` units each `per`, up to `max`, continuously; `Never` is a balance. `resets_in(now)` is how long until a fixed window ends, zero once it has (the bucket is then due a new report), `None` for any other. A unit, status, source, refill or coverage a newer writer names reads as `Other` and the report is still read; `Unit`, `Status` and `Source` keep the name they were sent under.
- **Warning thresholds are the host's.** A provider reports `Status::Warning` only when the harness said so (Claude's `allowed_warning`), and never invents a level; a host applies its own to the fraction.
- **`revision` and `observed_at`.** Each bucket has its own `observed_at`; the report's is the newest bucket's. `revision` grows by one with each change on a connection (and per provider instance for the kind's answer), and a repeat of figures already held changes nothing, so a host that polls keeps the newest by revision and an unchanged account costs it nothing.
- **A refusal** is a `LimitHit`: the bucket it names (`exhausted` in the same report), `resets_at` when the harness said, and `retry_after` when the refusal did (a 429's header), both explicit fields, never only text. The report keeps it until its reset passes or a later report shows room; a retry-after can move, which is why a host reads the report again and does not trust it once.
- **Inferred limits** (`infer_limit(text, phrases)`): for a failed turn of an agent that reports nothing, a host matches the turn's text against `phrases` (ASCII case-insensitive; the earliest in the text wins; an empty list infers nothing) and gets a `LimitHit` with `source: inferred`, `bucket: "inferred"`, `text` the matched phrase (at most 120 bytes, never the turn's text) and `observed_at` 0 for the caller to stamp. `DEFAULT_LIMIT_PHRASES` is what a host uses when it sets none; **a host's own list replaces it**, it is not added to it. Antigravity's 503 "No capacity available" is capacity, not a quota, and matches no default. Only what the text states is read: `resets_at` from a UTC time after `reset at` (`2026-11-10T12:00:00Z`, with `Z` or `+00:00`), `retry_after` from a length after `retry after` / `retry in` / `try again in` / `reset after` / `reset in` (`2h30m`, `90 seconds`, `1 hour and 5 minutes`; a bare number only after `retry-after:`, as seconds); a length with a fraction, a missing or unknown unit or units out of order is left unset, and a relative length never becomes `resets_at` (no clock). It never panics, reads the first 64 KiB, skips a phrase over 256 bytes, and is linear in the text. `UsageReport::inferred(hit)` is the host's report for that agent: `sources: ["inferred"]`, one `exhausted` bucket `inferred` of `unknown` coverage and no figures, a fixed window ending at `hit.resets_at` when set (else refill `unknown`); the host numbers `revision` and sets `account`.
- **`Spend`** is a session's running totals; `cost_micro_usd` is `None` when the harness reported no cost or a partial one, never `0` for unknown. A conversion from another unit rounds to the nearest micro-USD, halves up.

**The ACP extension** runs on ACP versions 1 and 2 and has a version of its own (`USAGE_VERSION`, 1; until the first release a change is a break and stays 1):

| part | where | shape |
| --- | --- | --- |
| capability | `agentCapabilities._meta[USAGE_CAPABILITY]` (version 2: `capabilities._meta`) | `{"version": 1, "sources": ["reported", "inferred"], "spend": true}`; absent means the agent reports none, and a client does not call it |
| opt-in | `initialize`'s `clientCapabilities._meta[USAGE_CAPABILITY]` | `{"notify": true, "account": "<host id>"}`; notifications go only to a client that set `notify` |
| request | `_coqui_acp/usage` | `{"sessionId"?: uuid}` answers a `UsageReport`, with the session's `spend` when one is named and none otherwise; an unknown session is `-32602`, an agent without the capability answers `-32601` |
| notification | `_coqui_acp/usage_update` | `{"sessionId"?: uuid, "report": UsageReport}`: the whole report on every change (a bucket's figures, status or reset, a limit hit, a session's spend after a turn), never a delta |
| refused turn | the turn's answer (version 2: the `idle` update) | `stopReason: "refusal"` and `_meta[LIMIT_META_KEY]` holding the `LimitHit` |

The request needs no session and no turn and is cheap, so a host may poll it; how often is the host's choice. `account` is the host's id for the account a connection runs on, taken from `StartRequest.account`'s `id` (else the client's opt-in), echoed into every report and never interpreted; a host that launched the connection overwrites it in what it relays, so a provider cannot misattribute.

## Accounts (`coqui_acp_api::account`)

A provider owns every fact about how its harness takes an account: kinds, variables, directories, files, login commands, renewal and endpoints. The host owns which accounts exist, their secrets at rest, who holds one, claims and limits. A credential crosses only as `Secret<Credentials>`: its schema is `writeOnly` and `x-sensitive`, its `Debug` names the keys and never a value, and wiping it zeroes both.

```rust
pub const HARNESS_LOGIN: &str = "harness-login";   // every catalog lists it: nothing handed over, nothing cleared
pub struct Credentials;   // BTreeMap<String, String> inside, keyed by the sensitive setting's key path; a file is one entry holding its text
Credentials::new(); .insert(k, v); .get(k); .iter(); .keys(); .len(); .is_empty(); FromIterator; From<BTreeMap<String, String>>
#[non_exhaustive] pub struct AccountGrant { pub id: String, pub kind: String, pub settings: Value, pub credentials: Secret<Credentials>,
    pub state_dir: Option<String>, pub revision: u64 }
AccountGrant::new(id, kind);  .settings(v) .credentials(c) .state_dir(d) .revision(n);  .reference() -> AccountRef;  .check_id()
#[non_exhaustive] pub struct AccountRef { id, kind, settings, state_dir, revision }   // a grant without credentials
pub fn is_account_id(&str) -> bool;  pub const ACCOUNT_ID_MAX: usize = 128;
#[non_exhaustive] pub struct AccountCatalog { pub kinds: Vec<AccountKindInfo>, pub logins: Vec<LoginMethodInfo>, pub credential_variables: Vec<String> }
AccountCatalog::new() /* only harness-login */;  .kind(id)
#[non_exhaustive] pub struct AccountKindInfo { pub id: String, pub title: String, pub description: String, pub settings: Value,
    pub endpoints: Vec<EndpointSlot>, pub rotates: bool, pub history_portable: bool, pub state_dir: bool, pub renews: bool, pub notices: Vec<String> }
AccountKindInfo::new(id, title);  AccountKindInfo::harness_login()
#[non_exhaustive] pub struct EndpointSlot { pub key: Option<String>, pub purpose: String /* MODEL_API AUTH PROXY */, pub default: Option<String> }
EndpointSlot::new(purpose).key(k).default_url(u)
#[non_exhaustive] pub struct LoginMethodInfo { pub id: String, pub title: String, pub description: String, pub produces: String,
    pub inputs: Value, pub steps: Vec<String> /* OPEN_URL USER_CODE ASK PASTE_CODE */, pub remote: bool }
#[non_exhaustive] pub struct LoginRequest { pub method: String, pub inputs: Value, pub state_dir: String, pub timeout_ms: u64 /* DEFAULT_LOGIN_TIMEOUT_MS: 600000 */ }
#[non_exhaustive] pub struct LoginOutcome { pub kind: String, pub settings: Value, pub credentials: Secret<Credentials>,
    pub label: Option<String>, pub expires_at: Option<String>, pub checked: bool }
#[non_exhaustive] pub struct RenewRequest { pub account: AccountGrant }
#[non_exhaustive] pub struct RenewReport { pub credentials: Option<Secret<Credentials>>, pub expires_at: Option<String>, pub not_renewed_because: Option<String> }
#[non_exhaustive] pub struct ModelsRequest { pub environment: Secret<Environment>, pub account: Option<AccountGrant> }
#[guatiao::kind(object, name = "account_keeper")]
pub trait AccountKeeper: Send + Sync {
    fn renewed(&self, renewal: AccountRenewal) -> Result<(), ProviderError>;   // AccountRenewal { id, from_revision, credentials: Secret<Credentials>, expires_at, sequence }
    fn lost(&self, loss: AccountLoss) -> Result<(), ProviderError>;            // AccountLoss { id, reason, sequence }
}
#[guatiao::kind(object, name = "login_prompter")]
pub trait LoginPrompter: Send + Sync {
    fn say(&self, text: &str) -> Result<(), ProviderError>;
    fn open_url(&self, open: OpenUrl) -> Result<(), ProviderError>;            // OpenUrl { url, user_code: Option<String>, expires_ms: Option<u64> }
    fn ask(&self, form: LoginForm) -> Result<LoginAnswer, ProviderError>;      // LoginForm { title, schema }; LoginAnswer { values: Value, secrets: Secret<Credentials> }
    fn cancelled(&self) -> Result<bool, ProviderError>;
}
```

- **The catalog.** `kinds[].settings` is a guatiao (JSON) schema of the kind's non-secret settings; a credential is a property marked `writeOnly` and `x-sensitive` (both, never one), typed text, with no default or example, and its key is the key in `Credentials`. `endpoints` name the URLs the kind talks to (`key` a text setting holding an absolute `http(s)` URL, `default` what is used without it) for the host's egress policy. `rotates`: a renewal replaces a credential so that an older copy stops working (an unmeasured rotation says `true`); the host holds such an account exclusively. `history_portable`: a session of one account resumes on another of the kind. `state_dir`: the kind needs a private directory. `renews`: `renew` can renew it. `notices`: what using it weakens. `credential_variables` is every variable any kind reads for a credential.
- **An account id** is the host's label and nothing more: a provider never puts it in a path or an argument vector, so `is_account_id` asks no more than 1 to 128 bytes and no control character.
- **Applying a grant** (`start`, `import_session`, `export_session`, `models`, `usage`, `renew`). The provider removes every name in `credential_variables` from what it starts (`SpawnSpec::remove_environment`, or `clear_environment`), then sets the chosen kind's; writes the kind's files into `state_dir` through the spawner (only when absent, after looking at what is there: the file the host last wrote may have been renewed since); and starts the harness on them. `harness-login` hands nothing over and clears nothing; with no account the environment passes as it is. `BAD_VALUE`: a kind it does not list, settings its schema refuses, a credential missing or one the kind does not take, a missing `state_dir` for a kind that needs one, an id that is not an account id. `UNSUPPORTED`, with the reason: a kind it cannot apply through the spawner (`files` false). A credential is in no answer, refusal, error, log or argument vector; a variable set to it is the one place it is allowed.
- **Renewals come back.** A provider of a kind with files looks at them through `file_info` and `read_file` on a deadline of its own, every `renew_poll_ms` (its setting; 30000 by default), whatever traffic there is, while a connection runs, and after each import, export, models call and close. A file whose content changed is `AccountKeeper::renewed` (whole credentials, `from_revision` the grant's); a file deleted or blanked is `lost`, never `renewed`. **Every keeper live for the account hears each event**, a connection's and a call's alike, not the first to notice. Each event carries a `sequence`, one count per account for renewals and losses together (1, 2, 3 ... with no repeat for as long as the provider lives), so chained rotations are ordered; **the host deduplicates by (account, sequence)** and stores a renewal only when it is newer. A change is recorded before anyone is told, so it is one event, not one for each looker. A provider calls a keeper with no lock of its own held. The first time a provider looks at an account it has no record of, a file there that was changed after the provider began is news (told as a renewal and kept), and an older one is replaced by the grant's copy.
- **`renew`** renews with no model turn: new credentials, or `not_renewed_because`. `UNSUPPORTED` for a kind whose `renews` is false; `GONE` with the reason when the refresh lost the login (a blank or missing login file is lost, never renewed from nothing).
- **`login`.** The provider runs the CLI in `LoginRequest::state_dir` through the spawner, relays what it prints through the prompter (`say`, `open_url` with a device or user code, `ask` for a form), and reads the files it wrote into `LoginOutcome::credentials`; `checked` says a call that spends nothing verified it. The person cancelling makes every prompter call answer `GONE` and `cancelled` true; that, or `timeout_ms` passing, makes the provider kill the CLI's whole tree and answer `GONE` `login cancelled` / `login timed out` within two seconds. A provider runs a watchdog of its own: at the deadline it answers even if a prompter call has not returned (the call ends by itself, and the provider lets go of the prompter and spawner then), so a host's `ask` should return by `timeout_ms`. The CLI a login starts has every credential variable cleared, as a connection's program has. A key the CLI refused is `BAD_VALUE` `login refused: <the CLI's reason>`; another failure `INTERNAL` with a reason that holds no input and no URL query; an unlisted method `BAD_VALUE`; a provider with no method `UNSUPPORTED`. The host removes the login's `state_dir` however it ended.
- **Endpoints and the cache hint.** The host reads the slots plus the grant's settings; a value that is not an `https` URL (or `http` to loopback) is the host's to refuse before it starts.

## Starting a program in a folder on a share (`coqui_acp_api::batch`)

```rust
pub fn start_in(program: &OsStr, args: &[OsString], cwd: &Path) -> io::Result<(OsString, Vec<OsString>)>
pub fn on_share(path: &Path) -> bool   // \\server\share\..., not a device path
```

cmd.exe takes no share as its working directory and falls back to the Windows folder, so a `.cmd` or `.bat` program to run in one is not started in it. `start_in` answers what to start: as asked, except on Windows for a batch file whose `cwd` is on a share, where it is the program an npm-written file names (its `node.exe` beside it, else `node`, with the script, or the executable it calls) with the file's own arguments first and `args` after. Any other batch file is refused with the reason, since running it elsewhere would be a quiet mistake. The harness's process start and the contract suite's spawner call it; a host that starts a program of its own in a session's folder should too.

## A provider in another language

Every method takes and answers plain data; `start` also takes the spawner and the sink and answers the channel, all objects. The Python module is what `python -m guatiao.kinds` writes from the description: a provider subclasses `AcpHarness` and `Channel`, calls the sink it is handed (`receive(bytes)`, `closed(status, reason)`, then `destroy()`), from a `threading.Thread` of its own if it likes, and calls `destroy()` on the spawner it does not keep; its `Channel` implements `send(bytes)` and `close(status, reason)`. A method that raises gives up every object it was handed, and Guatiao's binding destroys them: a refusing `start` just raises. The models' schemas in `include/coqui_acp_api.schemas.json` say which keys a request may have. A provider in a language whose objects are called on one engine thread (QuickJS, Node, Lua) has no thread of its own: it answers inside the `send` that took the message, so it cannot be told `session/cancel` while a turn runs.

## The contract suite (feature `contract`)

```rust
pub struct contract::Contract;   // #[must_use]
Contract::new(name) .version(u16 /* 1 */) .holds(Vec<Requirement> /* read_only, no_commands */) .message_limit(bytes)
    .imports(bool /* true */)  .logins(bool /* true */)  .account(kind)  .program_agent(bool /* false */)
    .run(&dyn AcpHarness) -> Result<(), contract::Failures>
contract::spawner_files(&dyn ProcessSpawner, root: &Path) -> Result<(), contract::Failures>   // a host's spawner, in its directory
#[non_exhaustive] pub struct contract::Failure { pub check: &'static str, pub message: String }
pub struct contract::Failures { pub provider: String, pub failures: Vec<Failure> }   // Display lists each
```

Every provider of the kind passes it, configured to drive the **contract agent**: a real harness supplies a stand-in for its program that behaves as the module's documentation says (`close`, `wait`, `ask`, `read PATH`, else `echo: TEXT`; a harness whose `describe` lists usage sources also understands `quota P` and `limit`). It checks the methods, requirements, secrets, ACP and its errors, seeding a session at the host's UUID (empty, then resumed; exported after turns, imported at another UUID, then resumed; what an import and an export refuse), paging an export (pages join to the whole, the header with its digest on the first only, a one-byte page refused with the next line's size, an unknown or stale `after` refused), usage (a harness that reports none advertises nothing and answers `_coqui_acp/usage` with `-32601`; one that reports answers it with no session and no turn, grows `revision` on a change and not on a repeat, tells only the client that asked, echoes the account, ends a limited turn `refusal` with the `LimitHit`, and has `usage()` agree with its last connection), accounts (below), reopening, cancel, permission answers, client capabilities, back-pressure, every close, working roots, the size limit and a real `coqui_acp::Client`. **The accounts checks** give the provider a virtual spawner that starts nothing (every `spawn` is answered with an idle process, and records its spec) and serves files in a directory of the suite's, and a canary for every credential. They hold the catalog (schemas parse, credentials marked sensitive, endpoints, methods and steps well formed, the same answer twice, `harness-login` listed, no login method when logins are off); that a start on each kind (all of them, or `--account` alone) removes every credential variable from what the provider spawns and leaves the canary out of every argument vector, message and close reason, and ends the programs it started; that an unknown kind, a bad id, a missing credential or directory and a spawner without files are refused as above in every call that takes an account, saying nothing of a credential; that a rotated file is `renewed` once and a blanked or deleted one `lost` once (the suite rewrites the file the provider wrote and calls `models` on the account until a keeper hears); that `renew` answers as the catalog says; and, for each login method, that it completes and what it made starts, that cancelling at the first prompt and while waiting answers `GONE` within two seconds with every program it started killed, and that a 300 ms timeout does too. A provider that spawns the real harness is held to the same by a spawner that runs it: `Contract::program_agent(true)` makes the suite's spawner start what it is asked on this machine (standard error apart, no terminal; a program is ended when it exits, and killed with its whole tree: a job object on Windows, a process group on Unix), still recording every spec, in a home of the suite's (`ACP_HARNESS_CHECK_HOME` when set, else a fresh directory removed at the end), and the provider's settings name a stand-in for the program that behaves as the table above says for `wait`, `ask`, `die` (exits with status 3: the provider tells the host through `notice` that it ended), `echo: TEXT` and, when `describe` lists usage sources, `quota P` and `limit`. What a program cannot do is then not asked: `read PATH` (it cannot ask the client for a file), `close` (a program that ends ends a session, not the connection) and the exact figures of `quota` and `limit` (their ids, the token bucket, `retry_after`), of which the shape is kept (the report changes and tells a client that asked, agrees with the kind's answer when the harness learns usage in a turn, and a limit refuses the turn with a `LimitHit` and is kept). A session is given a first turn before it is reopened, since a program keeps one from its first turn on.

Whatever the agent, the suite reads a harness's updates by their kind (it takes the words from `agent_message_chunk` and ignores a title, commands or a usage figure around them), picks the option to allow once and the one to reject once by their `kind`, takes any order of answers to requests sent together, treats a reply that chooses no offered option (an error, an outcome nobody knows, an option never put, an allow option of another harness) as a refusal that ends the turn `end_turn` with the tool not run, and only the cancelled outcome as a cancel, and counts a turn as unanswered while only updates arrive.

It makes and removes a directory under the system's temporary one, with a link (a junction on Windows) when the platform allows.

`acp-harness-check` runs it on a provider with no Rust of the provider's own: `--library FILE`, or `--source SOURCE_LIBRARY --source-config JSON` for one a Guatiao source loads; `--provider KEY`, `--settings JSON` (build an instance), `--version N`, `--holds NAME[=JSON]` (repeated), `--message-limit N`, `--no-imports` (it must refuse `import_session` with a reason), `--no-logins` (it lists no login method and must refuse `login` `UNSUPPORTED` with a reason), `--account KIND` (the account checks on that kind alone), `--program-agent` (the provider starts its harness's own program, which the checker's spawner starts for real: `--settings` must name a stand-in for it; the checker runs again in a home of its own, `ACP_HARNESS_CHECK_HOME`, with no `CLAUDE_CONFIG_DIR` or `CODEX_HOME`, and every program the suite starts gets that home, so a real command line on the `PATH` never reaches the person's; what the spec's environment sets stays). `--holds none` says the provider holds no requirement. Exits 0, 1 listing each rule broken, or 2 when the provider could not be loaded.

## Tests

`examples/echo_harness.rs` is the contract agent in Rust, built by `cargo test` into a `cdylib` the tests load, with settings (`title`, `max_message_bytes`, `imports`, `usage` of `none` (the default) or `scripted`, `logins`, `renew_poll_ms`) a host builds instances from. It takes accounts of `echo-login` (a file credential in the account's directory, `auth_file`; rotates, renews; watched, and told to the keeper when it changes) and `echo-key` (`api_key` in `ECHO_API_KEY`), besides `harness-login`, and offers the login methods `scripted` (ask, open_url, user_code), `waiting` (open_url, then a wait a cancel or a timeout ends) and `api-key` (ask). `examples/provider-python/harness/acp_echo_harness.py` in the repository is the same in Python.
