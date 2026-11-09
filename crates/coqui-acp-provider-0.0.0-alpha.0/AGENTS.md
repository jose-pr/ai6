# `coqui-acp-provider` — API header

Header-file-style reference for the `coqui-acp-provider` crate: every public item with its signature, contract and gotchas, so the crate can be used without reading its source. It ships inside the package and is self-contained. Development documentation lives with the source at <https://github.com/Boriken-Dev/coqui-acp>.

There are no Cargo features. MSRV Rust 1.89. A default build depends on `agent-client-protocol`, `futures`, `guatiao` (with `provider`), `guatiao-serde` (no format), `serde`, `serde_json` and `uuid`. Guatiao is pinned by revision. Version `0.0.0-alpha.0`, not published; the API may change in any release before 1.0.

Imports: everything is at the crate root. `coqui_acp_provider::sdk` is the protocol's own crate, for its transports; `coqui_acp_provider::guatiao` is Guatiao, so a provider names one version of it (`#[provider(AgentProvider, crate = ::coqui_acp_provider::guatiao)]`).

No `unsafe` code (`#![forbid(unsafe_code)]`). No process-wide state.

## Kinds

A kind is a C ABI. Its table carries a hash of every slot as spelled, so renaming a slot or a model breaks every built provider: `AgentProviderVtable::FLOOR_HASH` is `1182182788`, `AgentSessionVtable::FLOOR_HASH` is `1414216972`. The package holds both kinds as data in `include/coqui_acp_provider.kinds.json`, and for Python in `python/coqui_acp_provider_kinds.py`.

```rust
#[guatiao::kind(name = "agent_provider")]
pub trait AgentProvider: Send + Sync {
    fn start(&self, request: StartRequest) -> Result<Object<dyn AgentSession>, ProviderError>;
    fn resume(&self, request: ResumeRequest) -> Result<Object<dyn AgentSession>, ProviderError>;   // default: refuses, unsupported
}
```

A provider owns whatever process it runs; no terminal crosses. `start` may block until the harness runs. `resume` reattaches to an identity this provider issued, as long as it still exists. **An identity works as a bearer secret**: the request carries no owner, so whoever names it resumes it. The facade lets one of its sessions hold an identity at a time; when none holds it, who may resume it is the provider's to decide.

```rust
#[guatiao::kind(object, name = "agent_session")]
pub trait AgentSession: Send + Sync {
    fn session_id(&self) -> Result<Option<String>, ProviderError>;   // once the provider has one
    fn send_message(&self, message: AgentMessage) -> Result<(), ProviderError>;
    fn next_event(&self, timeout_ms: i64) -> Result<Option<AgentEvent>, ProviderError>;
    fn interrupt(&self) -> Result<(), ProviderError>;
    fn close(&self) -> Result<(), ProviderError>;
}
```

Any method may be called while another runs, from another thread: `next_event` blocks for its timeout, and `interrupt` and `close` have to reach it.

- `next_event` answers `None` when `timeout_ms` expires. A harness's exit must be reported as a `completed` or `failed` event, the `failed` text saying how it ended and what it wrote last: a provider that only returns `None` after end-of-file leaves its consumer waiting forever. A call that takes much longer than `timeout_ms` counts as hung.
- `interrupt` asks the provider to interrupt the running turn, which then ends with an event as any turn does.
- `close` returns once the harness has exited, or after a short wait when that cannot be told: the caller may remove the working directory next. It is called once.

A provider in another language: every method takes and answers plain data. `start`'s request is a read-only value, valid for the call; the Python binding reads it with `request.to_python()`. An event may leave out anything but `kind`. The Python module is what `python -m guatiao.kinds` writes, through `python/write_kinds.py`, and its `close` slot is `close`, as in Rust: a Guatiao older than the pinned revision refuses a Python slot of that name.

## Requests

```rust
#[non_exhaustive]
pub struct StartRequest {
    pub working_directory: String,   // absolute
    pub prompt: String,
    pub context: Vec<String>,        // to establish before the prompt
    pub environment: BTreeMap<String, String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub meta: Option<guatiao::Value>,
}
pub fn StartRequest::new(working_directory: impl Into<String>, prompt: impl Into<String>) -> StartRequest

#[non_exhaustive]
pub struct ResumeRequest {
    pub session_id: String,
    pub working_directory: String,
    pub prompt: Option<String>,      // a message to submit after resuming
    pub context: Vec<String>,        // the standing part, for a harness that may not keep it; never a recap
    pub environment: BTreeMap<String, String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub meta: Option<guatiao::Value>,
}
pub fn ResumeRequest::new(session_id: impl Into<String>, working_directory: impl Into<String>) -> ResumeRequest
```

`environment` may hold credentials: a provider logs and reports none of it, its schema marks it sensitive, and no `Debug` here shows its values. Through the facade everything else is the client's choice and untrusted input: `working_directory` is any absolute path the host's `working_roots` allows (by default any, a network share's included), and `prompt`, `context`, `model`, `effort` and `meta` are what the client wrote. `meta` is the provider's alone: nothing between the client and the provider reads it.

## Events

```rust
pub struct AgentMessage { pub text: String }
pub struct AgentEvent {
    pub kind: String,
    pub text: Option<String>,
    pub data: Option<Vec<u8>>,
    pub limit: Option<UsageLimit>,
    pub usage: Option<TokenUsage>,
    pub tools: Vec<String>,
    pub quota: Vec<QuotaReport>,     // JSON: absent when empty
}
pub fn AgentEvent::new(kind: impl Into<String>, text: Option<String>) -> AgentEvent
pub struct UsageLimit { pub resets_at: Option<i64>, pub window: Option<String> }
pub struct TokenUsage { pub input: u64, pub output: u64, pub cache_read: u64, pub cache_write: u64, pub cost_micro_usd: u64, pub cumulative: bool }  // Default, Copy
pub struct QuotaReport { pub window: String, pub used_permille: Option<u16>, pub resets_at: Option<i64>, pub status: Option<String> }
```

All derive `ToValue`, `FromValue`, `Schema`, `Serialize`, `Deserialize`. In JSON `kind` and `tools` are required; the others read as absent when left out, and are written as `null` (`quota` is left out when empty). As a Guatiao value only `kind` is required. Their fields are public and the structs are not `non_exhaustive`, so a literal names every field.

Consumers match `kind` as text, so these strings are the contract:

| Constant | Text | Meaning |
| --- | --- | --- |
| `TEXT_EVENT` | `text` | Agent text, in `text`. One message may take any number of events. |
| `TOOL_EVENT` | `tool` | The agent used a tool: `text` is its title for people, `tools` its name. |
| `BLOCKED_EVENT` | `blocked` | The agent stopped short of what it was asked, and `text` says why. The turn goes on, unless `text` is `TURN_CANCELLED`. |
| `COMPLETED_EVENT` | `completed` | The turn finished. Also a harness that exited cleanly. |
| `FAILED_EVENT` | `failed` | The turn failed, and `text` says why; for a harness that exited, how it ended and what it wrote last. |
| `CANCELLED_EVENT` | `cancelled` | The turn stopped because of `interrupt`, or the agent gave it up. |
| `LIMIT_EVENT` | `limit` | The provider refuses further work until a quota resets. In place of `completed` or `failed`, with `limit` set. |
| `DENIED_EVENT` | `denied` | The harness refused calls in this turn by its rules. Before the turn's ending. `text` names the tools, separated by `, `; `tools` stays empty. |
| `TURN_CANCELLED` | `the agent cancelled the turn` | The `text` of a `blocked` event that means `cancelled`. No `completed` follows it. |

A turn ends with exactly one of `completed`, `failed`, `limit`, `cancelled` or `blocked` with `TURN_CANCELLED`. Any other kind (`usage`, `notice`, `permission`) ends nothing and a consumer may pass over it. An event of any kind may carry `tools`, `usage` and `quota`.

## `AcpFacade`

```rust
pub fn AcpFacade::new(provider: Arc<dyn AgentProvider>) -> AcpFacade   // Clone
pub fn with_auth(self, auth: FacadeAuth) -> Self
pub fn with_settings(self, settings: Settings) -> Self
pub fn with_agent_info(self, name: impl Into<String>, version: impl Into<String>) -> Self
pub fn run(&self, transport: impl ConnectTo<sdk::Agent> + 'static) -> Result<(), Error>
pub fn serve(&self, transport: impl ConnectTo<sdk::Agent> + 'static) -> Result<JoinHandle<Result<(), Error>>, Error>
```

`run` serves one connection on the calling thread until its transport closes, then closes its sessions and waits for them, up to `close_timeout`. `serve` is that on a thread. Sessions belong to the connection that opened them: another connection cannot name one, and a client that vanishes mid-turn has its sessions closed.

What ACP sees: `initialize` answers version 1, whatever the client asked. `authenticate` as below. `session/new` checks what it is given and makes no provider call; the first `session/prompt` calls `start` and later ones `send_message`, one turn at a time. `session/cancel` calls `interrupt` for that turn alone, and the prompt is answered `cancelled`, however the provider ended the turn; an `interrupt` still running when its turn is over is not called again for it, and one not yet called is dropped. `session/resume` calls `resume` and its answer waits for the provider's, so a session that no longer exists fails there in the provider's words; a prompt before that answer is refused. Its `sessionId` is the provider's identity, it stays the ACP session's, and it is refused while another session of the facade, on any connection, holds that identity. `session/close` answers when the provider's `close` returned. `session/set_config_option` and `session/set_mode` as in [Options](#options). `session/list` answers this connection's open sessions, `{sessionId, cwd}`, filtered by `cwd`, in one page, a `cursor` refused: another connection's are not its to see. `SESSION_OPTIONS_METHOD` as in [Options](#options). `session/load` and everything else is method not found. A prompt is text and resource links, a link passing as its address; any other block is refused. A client's `mcpServers` other than an empty list is refused: no field of the kind carries them.

### What ACP has no field for

```rust
pub const LAUNCH_META_KEY: &str = "dev.boriken.coqui.acp/launch";
pub const EVENTS_CAPABILITY: &str = "dev.boriken.coqui.acp/events";
pub const EVENT_NOTIFICATION: &str = "_coqui_acp/event";
pub const SESSION_OPTIONS_CAPABILITY: &str = "dev.boriken.coqui.acp/session_options";
pub const SESSION_OPTIONS_METHOD: &str = "_coqui_acp/session_options";
```

`initialize` advertises `sessionCapabilities` `resume`, `close` and `list`, and `_meta` `EVENTS_CAPABILITY` and `SESSION_OPTIONS_CAPABILITY`, each `true`.

`_meta[LAUNCH_META_KEY]` on `session/new` and `session/resume`: `{"context": [text], "environment": {name: text}, "model": text, "effort": text, "meta": any}`, each optional. An unknown key is refused. An environment name not in `Settings::client_environment` is refused. A `model` or `effort` the host declared an option for must be one of its values, or it is refused; one it declared none for passes as written.

`EVENT_NOTIFICATION`, advertised under `agentCapabilities._meta[EVENTS_CAPABILITY]`, is `{"sessionId", "providerSessionId", "event"}`: the event as JSON, for what ACP has no update for. With no `event` it says the provider's identity is now known, once; that identity is what a later `session/resume` names. An identity with a character a terminal acts on, or longer than `max_label_bytes`, is never told, since it could not be named back as it is. In an event the kind, tool names and quota strings are cut as titles, the text and bytes as below; an event that still does not fit a line is replaced by its kind and a note that it was larger.

| Event | On the wire |
| --- | --- |
| `text` | `agent_message_chunk`s, split at `max_update_bytes` |
| `tools` of any event | a `tool_call`, kind `other`, `in_progress`; a `tool_call_update` ends it with the turn |
| `denied` | a `tool_call`, kind `other`, `failed`, per name |
| `usage` with a cost | `usage_update`, in USD |
| `completed` | stop reason `end_turn` |
| `limit` | stop reason `refusal` |
| `cancelled`, `blocked` with `TURN_CANCELLED` | stop reason `cancelled` |
| `failed` | the prompt fails: code `-32603`, the text in `data` |
| any other kind; any `limit`, `usage` or `quota` | `EVENT_NOTIFICATION` |

A failure with no prompt waiting goes as `EVENT_NOTIFICATION`, with kind `failed`.

### `Settings`

`#[non_exhaustive]`, `Default`; set the fields of `Settings::default()`.

| Field | Default | At the limit |
| --- | --- | --- |
| `max_sessions: usize` | 8 | `session/new` and `session/resume` are refused. A closed session holds its place until the provider's last call returns. |
| `poll: Duration` | 100 ms | The timeout given to `next_event`. A provider that returns sooner is called again after it. |
| `start_timeout: Duration` | 60 s | `start` or `resume`: the session ends, and what the call returns later is closed. |
| `call_timeout: Duration` | 30 s | Any other call, past its own timeout: the session ends. |
| `cancel_grace: Duration` | 10 s | A cancelled turn is answered `cancelled`. Its later events are dropped, and a prompt is refused until its ending comes, or the old ending would answer the new prompt. |
| `close_timeout: Duration` | 10 s | `session/close` fails, or `run` answers `Error::Stuck`. |
| `max_prompt_bytes: usize` | 8 MiB | The prompt is refused. |
| `max_turn_bytes: usize` | 64 MiB | What a provider sends in a turn or between turns, counted as the lines it becomes: the session ends. |
| `max_connection_bytes: usize` | 256 MiB | What the connection sends in all, as lines: the connection ends. |
| `max_update_bytes: usize` | 1 MiB | A text, as JSON, is split; an event in `EVENT_NOTIFICATION` is cut. |
| `max_label_bytes: usize` | 4 KiB | A tool's title or name, a failure text or an identity, as JSON, is cut. |
| `max_auth_failures: u32` | 5 | Every later request on the connection is refused unread, and the connection ends. |
| `working_roots: Vec<PathBuf>` | empty | A `cwd` must be under one, compared as written, with no `..` or `.`; empty takes any absolute path. |
| `environment: BTreeMap<String, String>` | empty | Given to every `start` and `resume`. |
| `client_environment: Vec<String>` | empty | The names a client may set, over `environment`. A client that can set `PATH` runs what it likes. |
| `options: Vec<StartOption>` | empty | What a client may choose before a session starts; see [Options](#options). Empty offers none. |

`max_sessions` is per connection, and so is everything above: a host serving many connections caps how many it accepts. When `run` returns, a session whose provider has not returned from a call counts against nothing but holds its identity until it does.

**What a client that never reads can cost.** The protocol library queues what goes out without limit and does not say what the client has read, so the bound is on what is sent: at most `max_connection_bytes` per connection, plus the answers to the client's own requests and a short update per tool call as its turn ends. No line is longer than `max_update_bytes`, twice `max_label_bytes` and 5 KiB, as long as `max_label_bytes` is set below `max_update_bytes`.

`Debug` shows the environment's names, never its values.

### Options

```rust
#[non_exhaustive]
pub struct StartOption {
    pub id: String,                   // what session/set_config_option names
    pub name: String,
    pub description: Option<String>,
    pub category: Option<String>,     // ACP's: "model", "thought_level", "mode", "model_config", or "_..."
    pub values: Vec<OptionValue>,
    pub default: String,              // a value's id
    pub target: OptionTarget,
}
pub fn StartOption::new(id, name, target: OptionTarget, values: Vec<OptionValue>, default) -> StartOption
pub fn StartOption::model(values: Vec<OptionValue>, default) -> StartOption    // id "model", category "model", OptionTarget::Model
pub fn StartOption::effort(values: Vec<OptionValue>, default) -> StartOption   // id "effort", category "thought_level", OptionTarget::Effort
pub fn category(self, category: impl Into<String>) -> Self
pub fn description(self, description: impl Into<String>) -> Self

#[non_exhaustive]
pub struct OptionValue { pub id: String, pub name: String, pub description: Option<String> }
pub fn OptionValue::new(id: impl Into<String>, name: impl Into<String>) -> OptionValue
pub fn description(self, description: impl Into<String>) -> Self

#[non_exhaustive]
pub enum OptionTarget { Model, Effort, Environment(String) }
```

The host declares them in `Settings::options`, since the kind cannot be asked what a provider takes. Each is a select of its own values, and a session holds one value of each, starting at `default`. The provider receives the values in `start` or `resume`: `Model` as `model`, `Effort` as `effort`, `Environment(name)` as that environment value, over `Settings::environment` and the client's. With no option of a target, `model` and `effort` are what the launch said, as before.

- `session/new` and `session/resume` answer `configOptions`: each option as ACP version 1's select `SessionConfigOption` (`id`, `name`, `description`, `category`, `"type": "select"`, `currentValue`, `options: [{value, name, description}]`) at the session's value. The option of category `mode` is also `modes`, ACP's `SessionModeState` (`currentModeId`, `availableModes: [{id, name, description}]`), kept the same. With no options, neither is in the answer.
- `session/set_config_option` `{sessionId, configId, value}` sets one and answers every option, `{"configOptions": [...]}`. `session/set_mode` `{sessionId, modeId}` sets the mode option and answers `{}`.
- A value not among the option's values, an option not declared, or a value that is not an id is refused, invalid params, and changes nothing. Once the first `session/prompt` has been taken, or for a resumed session at once, the provider has its values and a change is refused, invalid request: the kind has no way to switch.

Before any session, `SESSION_OPTIONS_METHOD` with any params answers every option at its default, as `configOptions`, and `modes`, as `session/new` would; it opens nothing and calls no provider. What the host did not declare is left out and said under `unknown`: `{"unknown": {"configOptions": "the host declared no options for this agent", "modes": "the host declared no modes for this agent"}}`. Like every request but `initialize` and `authenticate`, it needs authentication when the facade has `FacadeAuth`.

`run` checks the options first and answers `Error::Settings` when an id or a value repeats, a value list is empty, a default is not among the values, two options share a target, or two are of category `mode`.

### `FacadeAuth`

```rust
pub type Credential = dyn Fn(&Map<String, Value>) -> bool + Send + Sync;
pub struct FacadeAuth { pub method: String, pub name: String, pub check: Arc<Credential> }
```

Every request but `initialize` and `authenticate` answers auth required until an `authenticate` naming `method` has `check` accept its `_meta`. A refusal quotes nothing. After `max_auth_failures` refusals `check` is not called again: every request already sent, and any after, is refused, and the connection ends. `check` is the host's, so comparing in constant time is too.

## Errors

`Error` (`#[non_exhaustive]`): `Connection(String)` in the protocol library's words; `Thread(io::Error)`, a thread could not start; `Stuck(usize)`, this many sessions' providers had not returned from `close` when the connection ended; `Settings(String)`, `Settings::options` cannot be served, and why.

## Gotchas

- **A provider's text is untrusted when it reaches the client.** The facade carries it as data: a stop reason, an error code, a tool's kind and identifier never come from it, and titles, names, identities and failures lose control and format characters (bidirectional overrides, zero-width characters). `text` itself passes unchanged, so a client treats it as it treats any agent's output.
- **A tool's kind is always `other`.** A name says nothing a client could rely on, so a client's refused kinds (`coqui_acp`'s `refuse_kinds`) cannot hold a provider to anything: what a provider may do is the provider's to enforce.
- **A thread stuck in a provider cannot be stopped.** The session ends, its thread stays until the call returns, and it keeps its place in `max_sessions` and its identity. An `interrupt` stuck in the provider may still reach a later turn there when it returns: a provider applies `interrupt` to the turn running when it is called. A provider in the host's process that holds a lock forever, as Python code holding the interpreter does, stops every call that needs it: run one that may do that in a process of its own.
- **A panic stops at the call**, and its words are not told: they can hold anything the provider held. Through a Guatiao table it is a status; in a trait object called directly, `the provider panicked in <call>`.
- **Nothing reaches a provider's environment from a client by default**, and a provider's `meta` is whatever a client wrote.
- **Standard output is the transport** for a facade served on standard input and output, so a provider in that process must not print to it.
- `model` and `effort` reach a provider through config options only for a host that declared them in `Settings::options`, which is what `coqui_acp`'s `Builder::model` and `Builder::effort` use; otherwise only through `LAUNCH_META_KEY`.
