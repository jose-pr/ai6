# `coqui-acp-harness` — API header

Header-file-style reference for the `coqui-acp-harness` crate: every public item with its signature, contract and gotchas, so the crate can be used without reading its source. It ships inside the package and is self-contained. Development documentation lives with the source at <https://github.com/Boriken-Dev/coqui-acp>.

MSRV Rust 1.89. A default build depends on `coqui-acp-api` (for the usage model), `base64`, `serde_json` and `uuid`, and `win32job` on Windows or `rustix` on Unix. Feature `kind` (off by default) adds `spawner`, the `acp_harness` kind's spawner on this machine and `Spawn` through any host's, and `provider`, which serves a harness over the kind's channel (with `guatiao-serde`, for catalogs). Feature `antigravity` (off by default) adds `antigravity`, which reads Antigravity's quota from its loopback port, and with it `serde`, `time` (parsing) and `ureq` (rustls with ring, no default features); its tests also use `rcgen` and `rustls`. Version `0.0.0-alpha.0`, not published; the API may change in any release before 1.0.

Imports: the root re-exports `acp`'s `AgentInfo`, `ConfigOption`, `Conversation`, `EventSink`, `Harness`, `HarnessError`, `McpServer`, `McpTransport`, `Open`, `OpenRequest`, `OptionValue`, `Settings`, `Takes`, `UsageTracker` and `serve`; `event`'s `Event`, `Outcome`, `PermissionRequest` and `Responder`; `process`'s `Exit`, `Launch`, `LineWriter`, `PlainSpawn`, `Process`, `Spawn`, `Spawned`, `ending_once`, `exit_text`, `line_writer` and `watch_exit`; and `stream`'s `Answer`, `LineHook`, `Step`, `Stream` and `Waiting`. The root also re-exports `mode`'s `Mode`, and with `kind` `spawner`'s `LocalSpawner` and `ThroughSpawner`. The modules `acp`, `event`, `lines`, `mode`, `process`, `stream` and `version` are public, `spawner` and `provider` with `kind` and `antigravity` with `antigravity`.

No `unsafe` code (`#![forbid(unsafe_code)]`). No process-wide state.

## Processes (`coqui_acp_harness::process`)

```rust
#[non_exhaustive] pub struct Launch { pub program: OsString, pub args: Vec<OsString>, pub cwd: Option<PathBuf>,
    pub env: BTreeMap<OsString, OsString>, pub remove_env: Vec<OsString>, pub clear_env: bool }   // Clone, Default, PartialEq
pub fn Launch::new(program: impl Into<OsString>) -> Launch
pub fn arg(self, arg) -> Self;  pub fn args(self, args) -> Self;  pub fn cwd(self, cwd) -> Self
pub fn env(self, name, value) -> Self;  pub const fn clear_env(self, clear: bool) -> Self
pub fn display(&self) -> String          // the command line, for messages

pub trait Spawn: Send + Sync { fn spawn(&self, launch: &Launch) -> io::Result<Spawned>; }
pub trait Process: Send + Sync { fn kill(&self) -> io::Result<()>; fn try_wait(&self) -> io::Result<Option<Exit>>; }
#[non_exhaustive] pub struct Spawned { pub stdin: Box<dyn Write + Send>, pub stdout: Box<dyn Read + Send>,
    pub stderr: Option<Box<dyn Read + Send>>, pub process: Box<dyn Process> }
pub fn Spawned::new(stdin, stdout, stderr, process) -> Spawned
#[non_exhaustive] pub struct Exit { pub code: Option<i32>, pub signal: Option<i32> }   // Display: "exited with code 3"
pub fn exit_text(process: &dyn Process, wait: Duration) -> String   // how it ended, or "closed its output"
pub struct PlainSpawn;
pub type LineWriter = Arc<dyn Fn(&Value) -> io::Result<()> + Send + Sync>;
pub fn line_writer(stdin: Box<dyn Write + Send>, max_queued: usize, name: &str) -> io::Result<LineWriter>   // a thread of its own; a line past the bound fails
pub fn watch_exit(process: Arc<dyn Process>, poll: Duration, drain: Duration, done: Arc<AtomicBool>, end: impl FnOnce() + Send + 'static) -> io::Result<()>
pub fn ending_once() -> Arc<AtomicBool>
```

Each argument reaches the program as one argument, with no shell. `Launch`'s `Debug` names the environment's variables and counts the arguments, never showing their values.

`PlainSpawn` pipes all three streams. Its `kill` ends the tree. On Windows the process is put in a job object of its own just after it starts, and closing the job ends everything in it, at `kill` and when the host ends without one; a process started in the instant before it joins its job escapes. On Unix it leads a process group, which is sent `SIGKILL`, and it is not collected before that, so the group id stays its own; on Linux and Android `kill` first stops, then kills, every process descended from it, found through `/proc` by parent, those in a session of their own included. A watcher in the group (`/bin/sh` reading a pipe only the host holds) ends the group when the host ends without a kill; there a process that left the group escapes, and on other Unix systems it escapes `kill` too. `kill` then collects the process; killing one that has ended is no error, and once it is collected nothing is sent to its id or group again; `try_wait` then answers how it ended. On Unix `try_wait` leaves a running process uncollected. Dropping the process kills it as `kill` does.

Implement `Spawn` to run a harness elsewhere: in a container, over SSH, under a terminal.

## Spawner (`coqui_acp_harness::spawner`, feature `kind`)

```rust
pub struct LocalSpawner;                         // Clone, Debug, Default; #[must_use]
pub const fn LocalSpawner::new() -> LocalSpawner // 1 MiB of each output stream held unread
pub const fn buffer(self, bytes: usize) -> Self  // past it the program waits to write
pub fn files(self, dir: impl Into<PathBuf>) -> Self  // serves the file methods in dir, which must exist
impl coqui_acp_api::ProcessSpawner for LocalSpawner
pub struct ThroughSpawner;                       // Debug
pub fn ThroughSpawner::new(spawner: Object<dyn ProcessSpawner>) -> ThroughSpawner
impl Spawn for ThroughSpawner
```

`LocalSpawner` is the spawner a host hands an `acp_harness` start by default: it starts a `SpawnSpec` as `PlainSpawn` starts a `Launch`, standard error apart, into its standard output or nowhere, as the spec says, and gives no terminal (`GUATIAO_ERR_UNSUPPORTED`). A missing program is `GUATIAO_ERR_NOT_FOUND`. A thread per output stream reads it into the buffer; `read` and `read_error` wait up to their timeout. `describe` says `local`, `separate_error`, `ends_with_host`, and `kills_escaped` on Windows, Linux and Android. Its link is always up. Destroying a process kills its tree. With `files(dir)` it serves the kind's file methods (`write_file`, `read_file`, `file_info`, `remove_file`) in `dir` and `describe` says `files`; a path is relative, and an absolute one, a `..` part, an empty one and a link leading out of `dir` are `GUATIAO_ERR_NOT_FOUND`. Without it `files` is false and the four methods are `GUATIAO_ERR_UNSUPPORTED`.

`ThroughSpawner` is `Spawn` over any host's spawner, so a translator served with `serve` starts its program wherever the host's spawner runs it. Every text of the `Launch` must be UTF-8. Its reads wait through a link that is down; dropping its standard input closes the program's.

## Provider (`coqui_acp_harness::provider`, feature `kind`)

```rust
pub fn Serving::new(settings: Settings, max_message_bytes: usize) -> Serving       // Debug, Clone; #[must_use]
pub fn Serving::open(self, spawner: Object<dyn ProcessSpawner>, sink: Object<dyn ChannelSink>,
    harness: impl FnOnce(Arc<dyn Spawn>) -> Result<Arc<dyn Harness>, ProviderError>) -> Result<Object<dyn Channel>, ProviderError>
pub fn shared(harness: impl Harness) -> Arc<dyn Harness>
pub fn threads_done() -> Status                                                    // a library's `unload` hook
pub struct Reports;                                                               // Debug, Default
pub fn Reports::new() -> Reports
pub fn observe(&self, support: &UsageSupport, account: Option<&str>, report: &UsageReport)   // a connection's report
pub fn read(&self, support: &UsageSupport, account: Option<&str>, buckets: Vec<Bucket>) -> UsageReport
pub fn report(&self, support: &UsageSupport, account: Option<&str>) -> UsageReport
pub fn takes_of(takes: &Takes) -> coqui_acp_api::Takes                              // resume true
pub fn schema(json: &Value) -> guatiao::Value;  pub fn credential() -> Value        // for a catalog
pub fn account_refusal(catalog: &AccountCatalog, account: &AccountRef) -> Result<Option<String>, ProviderError>
```

What a provider of `acp_harness` needs to front a harness that `serve` serves. `Serving::open` builds the harness from a `Spawn` that starts its programs through the host's `spawner`, runs `serve` on a thread of its own and hands the host a `Channel`: each message sent is a line `serve` reads (a line feed inside a message that is not JSON is replaced by a NUL, so it stays one invalid line), each line it writes a message for the sink, which only one thread of the library calls. `send` waits while four messages wait on either side, so a sink that takes nothing holds the sender back (and `Settings::max_requests` bounds what `serve` has in hand); a message over `max_message_bytes` ends the connection `GUATIAO_ERR_BAD_VALUE`; `close` wakes a waiting `send` (`GUATIAO_ERR_GONE`), ends `serve`'s input, and `closed` follows once `serve` has returned and its lines are all handed over, with the host's status and reason, `GUATIAO_OK` when `serve` ended by itself and `GUATIAO_ERR_INTERNAL` when it failed. The spawner is destroyed before `closed`, whatever else holds the harness. A sink that refuses a message ends the connection with its status and message. A program that ends ends its session, not the connection: the host closes that.

`Reports` is what a provider keeps for the kind's `usage` method, by account: `observe` takes a connection's report (give it `Settings::on_usage`), `read` a reading the provider made itself, and `report` answers the merged figures or, for an account nothing was learned of, an empty report naming the sources. A repeat changes nothing and the revision only grows.

`account_refusal` is for a provider that applies nothing to the harness's environment yet: `GUATIAO_ERR_BAD_VALUE` for an id that is not an account id or a kind the catalog does not list, a reason for any listed kind but `harness-login`, `None` for that one.

The one process-wide state of the crate is the list of threads `provider` started; `threads_done` is `GUATIAO_ERR_BUSY` while one runs.

## Stream (`coqui_acp_harness::stream`)

```rust
pub type LineHook = Arc<dyn Fn(&Value) -> bool + Send + Sync>;
pub type EventHook = Arc<dyn Fn(Event) -> bool + Send + Sync>;
pub type Answer = (String, Result<Value, String>);
pub type Waiting = Arc<Mutex<BTreeMap<String, mpsc::Sender<Result<Value, String>>>>>;
#[non_exhaustive] #[derive(Default)] pub struct Step { pub events: Vec<Event>, pub answers: Vec<Answer>, pub stop: Option<String> }
pub const fn Step::new(events, answers) -> Step;  pub const fn Step::stopping(events, answers, why: String) -> Step
#[non_exhaustive] pub struct Stream { pub name, pub process, pub max_line, pub exit_poll, pub exit_wait, pub tail, pub closed, pub sink, pub waiting, pub ended, pub on_line }
pub fn Stream::new(name: &'static str, process, tail, closed, sink) -> Stream
pub fn read(self, stdout: Box<dyn Read + Send>, thread_name: &str, line: impl FnMut(&Value) -> Step + Send + 'static, last: impl Fn() -> Vec<Event> + Send + Sync + 'static) -> io::Result<()>
```

`read` gives each JSON line to `on_line`, then `line`, on a thread of its own, until the output ends or the process has exited with its output still open. Then every waiting request fails, `last` says what is left, and `Event::Exited` follows once, unless the session was closed. A refusal nobody waits for is told as a notice. `Usage::elsewhere` marks another thread's spending: no update, no part of the turn's usage. `TurnEnd::Stopped { stop, result }` carries the harness's result text when it gives one; `PermissionRequest::details` the harness's own fields, for a host; no client sees either.

## Lines (`coqui_acp_harness::lines`)

```rust
pub enum Line { Json(Value), Text(String), TooLong(u64) }
pub fn read_lines(reader: impl Read, max_line: u64, each: impl FnMut(Line) -> bool) -> io::Result<()>
pub fn parse(bytes: &[u8]) -> Option<Line>        // None for a blank line
pub struct Tail;  pub const fn Tail::new(max_lines: usize, max_line_bytes: usize) -> Tail
pub fn push(&mut self, line: &str);  pub fn text(&self) -> String;  pub fn is_empty(&self) -> bool
pub fn cut(text: &str, max: usize) -> &str       // on a character boundary
pub fn one_line(text: &str) -> String            // each control character a space, for a log or a label
pub fn read_diagnostics(reader: impl Read, max_line: u64, tail: &Mutex<Tail>)   // standard error into `tail`, read to its end
```

`read_lines` reads until the end or until `each` answers `false`. A line over `max_line` bytes is handed over as `TooLong` with how much was read, and reading stops. A line ending is dropped, `\r\n` included. `Tail` keeps the last lines that were not protocol, for the message shown when a harness ends.

## Events (`coqui_acp_harness::event`)

`Event` (`#[non_exhaustive]`, `Debug`) is what a harness reports, in no protocol's terms:

| Variant | Carries |
| --- | --- |
| `SessionId(String)` | the harness's session id, once it says it |
| `UserChunk { message, content: Content }`, `UserEnd { message }` | the person's messages, in replayed history |
| `MessageChunk { message, text }`, `MessageEnd { message }` | agent text, and the end of a message |
| `ThoughtChunk { message, text }`, `ThoughtEnd { message }` | reasoning |
| `Tool(ToolCall)` | a tool call announced or changed: a `None` field is unchanged; `content` and `locations` replace the list |
| `ToolChunk { id, content: ToolContent }` | content added to a tool call |
| `Terminal(Terminal)` | a terminal of the agent's own: `id`, `command`, `output` to add, `exit: Option<Option<i32>>` once it ended |
| `Plan(Vec<PlanEntry>)` | the plan, whole |
| `Permission(PermissionRequest)` | a call that needs leave |
| `Usage(Usage)` | tokens (`input`, `output`, `cache_read`, `cache_write`, `cumulative`), `cost_usd`, `context_used`, `context_size`, `elsewhere`, `spend: Option<Spend>` (the session's running totals; the connection fills the session id) |
| `Buckets(Vec<Bucket>)` | the account's usage allowances (`coqui_acp_api::usage::Bucket`): one replaces the earlier of its id and keeps what it leaves unknown; `observed_at` 0 is stamped when the connection hears it |
| `Commands(Vec<Command>)` | commands by name: `name`, `description`, `hint` |
| `Options(Vec<ConfigOption>)` | the session's choices as they stand now, after a change not a client's own |
| `Title(String)` | the session's title |
| `Notice { severity: Severity, title }` | something for the person outside the conversation |
| `Compaction { id, summary: Option<String> }` | the conversation was compacted |
| `Denied(Vec<String>)` | tools the harness refused by its own rules this turn |
| `TurnEnd(TurnEnd)` | `Stopped { stop, result }`, `Failed(String)` or `Limited(LimitHit)` (`coqui_acp_api::usage::LimitHit`: `bucket`, `resets_at`, `retry_after`, `source`, `text`, `observed_at`; 0 is stamped) |
| `Output(String)` | a line that was not protocol |
| `Exited(String)` | the harness ended, how, and what it wrote last |

`context_block(uri, text) -> String` writes a text resource as prompt text that it cannot close.

`Content`: `Text`, `Image { mime_type, data }` (base64), `ResourceLink { uri, name }`, `Resource { uri, mime_type, text, blob }`. `ToolCall` (`Default`): `id`, `name`, `title`, `kind: Option<ToolKind>`, `status: Option<ToolStatus>`, `input`, `output`, `locations: Option<Vec<Location>>`, `content: Option<Vec<ToolContent>>`; `ToolCall::new(id)`. `ToolContent`: `Text`, `Diff(Diff)`, `Terminal(id)`. `Diff`: `path`, `change: FileChange` (`Add`, `Modify`, `Delete`), `old_text`, `new_text`, `patch` (a git patch); `Diff::new(path, change)`. `ToolKind`, `ToolStatus`, `PlanStatus`, `Severity`, `Stop` and `OptionKind` have `as_str`, their names on the wire. Each `#[non_exhaustive]` struct has a `new` or `Default`.

```rust
pub struct PermissionRequest { pub tool: ToolCall, pub title: String, pub description: Option<String>,
    pub options: Vec<PermissionOption>, pub responder: Responder, pub details: Map<String, Value> }
pub fn PermissionRequest::new(tool, title, description, options, responder) -> PermissionRequest
pub fn details(self, details: Map<String, Value>) -> Self
pub struct PermissionOption { pub id: String, pub name: String, pub kind: OptionKind }
pub enum Outcome { Selected(String), Cancelled, Refused }
pub fn Responder::new(answer: impl FnOnce(Outcome) -> io::Result<()> + Send + 'static) -> Responder
pub fn answer(self, outcome: Outcome) -> io::Result<()>
```

A responder answers once; one dropped unanswered answers `Outcome::Cancelled`, so the harness is never left waiting. A tool call's input and output are the model's, untrusted.

## The ACP agent (`coqui_acp_harness::acp`)

```rust
pub trait Harness: Send + Sync + 'static {
    fn info(&self) -> AgentInfo;
    fn takes(&self) -> Takes;
    fn usage(&self) -> UsageSupport { UsageSupport::none() }     // the default is a true answer
    fn read_usage(&self) -> Option<Vec<Bucket>> { None }          // the account's usage with no session and no turn
    fn open(&self, request: OpenRequest, events: EventSink) -> Result<Box<dyn Conversation>, HarnessError>;
}
pub trait Conversation: Send + Sync {
    fn session_id(&self) -> String;
    fn options(&self) -> Vec<ConfigOption>;
    fn prompt(&self, content: Vec<Content>) -> Result<(), HarnessError>;
    fn interrupt(&self) -> Result<(), HarnessError>;
    fn set_option(&self, id: &str, value: &str) -> Result<Vec<ConfigOption>, HarnessError>;
    fn close(&self);
}
pub fn serve(harness: Arc<dyn Harness>, settings: Settings, input: impl Read, output: impl Write + Send + 'static) -> io::Result<()>
pub fn absolute_path(path: &str, what: &str) -> Result<PathBuf, String>
pub fn within_roots(path: &Path, roots: &[PathBuf]) -> bool                                 // links followed
pub fn path_admitted(path: &Path, roots: &[PathBuf], network_paths: bool) -> Result<(), String>   // the same rule as a session's directories
pub struct EventSink;  pub fn send(&self, event: Event) -> bool     // Clone; waits while the queue is full; false once the session is gone
pub type NoticeHook = Arc<dyn Fn(&str) + Send + Sync>;
pub use coqui_acp_api::LIMIT_META_KEY;     // "dev.boriken.coqui.acp/limit", a `LimitHit` as JSON
pub const PLAN_ID: &str = "plan";
```

`open` returns once the session's id is known; for `Open::Load` it sends the history to `events` first. A turn's events and its `TurnEnd` go to the sink. Any `Conversation` method may be called while another runs.

`OpenRequest` (`new(cwd)`): `cwd`, `open: Open` (`New`, `Resume(id)`, `Load(id)`), `mcp_servers: Vec<McpServer>` (`name`, `transport: McpTransport` of `Stdio { command, args, env }`, `Http { url, headers }`, `Sse { url, headers }`; `Debug` shows names, never values), `system_prompt`, `additional_directories`, `meta` (the request's `_meta`, the client's, untrusted). `Takes` (`new()` and one setter per field): `images`, `embedded_context`, `mcp_http`, `mcp_sse`, `load`, `additional_directories`, `system_prompt`. `ConfigOption::new(id, name, current, values)` with `category` and `description`; `OptionValue::new(id, name, description)`. `AgentInfo::new(name, title, version)`. `HarnessError`: `Refused(String)` is answered invalid params, `Failed(String)` internal error.

`serve` runs on the calling thread until the input ends, then closes every session it opened. Requests that take time run on threads of their own.

- A line that is not an object (a batch, a number) is answered `-32600` with id null, and one whose `method` is not text `-32600` with its id; an object with no method, result or error likewise.
- `initialize` is answered on a thread of its own, as asking a harness for `info` may take long. It answers version 2 to a client that asks for 2 or more, else version 1, up to `Settings::max_version`. Version 1: `agentCapabilities` with `loadSession`, `promptCapabilities`, `mcpCapabilities`, `sessionCapabilities` (`resume`, `close`, and `additionalDirectories` and `systemPrompt` when taken), `agentInfo`, `authMethods: []`. Version 2: `info` and `capabilities.session` (`prompt.image`, `prompt.embeddedContext`, `mcp.stdio`, `mcp.http`, `close`, `additionalDirectories`, `systemPrompt`). Any other request first is refused.
- `authenticate` answers `{}`. `session/new`, `session/load`, `session/resume` (version 2's `replayFrom: {"type": "start"}` replays, as `load`) check every part against `Takes` and `Settings` before the harness is asked: a `cwd`, MCP command or directory that is not absolute by this platform's rules (a drive or UNC path on Windows, `/…` elsewhere), a network or device path (`\\host\share`, `//host/share`, `\\?\…`, `\\.\…`) unless `network_paths`, a directory outside `working_roots` or holding `.` or `..` when roots are set, MCP servers when `client_mcp_servers` is off, two servers of one name, a transport not taken (version 2 has no SSE), a system prompt in a mode other than `append`, or a part not taken is invalid params. `new` answers `{sessionId, configOptions}` before the session's first events; `load` answers `{configOptions}` after its history; `resume` answers at once.
- A session being opened counts against `max_sessions` as an open one. `session/prompt` takes its turn on the thread that reads the client, so a `session/cancel` on the next line finds it; it takes text, images, resource links and resources as taken; any other block is invalid params, as is one larger than `max_prompt_bytes`. One turn at a time: a prompt while one runs is invalid request. Version 1 answers `{stopReason, usage}` at the turn's end; version 2 answers `{messageId}` at once, after a `user_message` and a `running` state update, and ends the turn with an `idle` state update carrying `stopReason` and `usage`. A failed turn is error `-32603` `Internal error` with the harness's words in `data.message` (version 1), or `idle` with `stopReason: "error"` and `error` (version 2). A limit is stop reason `refusal` with `_meta[LIMIT_META_KEY]` holding the `LimitHit`, and in version 2 a notice first.
- **Usage.** A harness whose `usage()` lists sources gets the usage extension (`coqui_acp_api` "Usage"): `initialize` advertises `dev.boriken.coqui.acp/usage` in `agentCapabilities._meta` (version 2: `capabilities._meta`) and reads the client's opt-in (`notify`, `account`) from `clientCapabilities._meta`; `_coqui_acp/usage` answers the connection's report with no session and no turn (a session's `spend` when `sessionId` names an open one, `-32602` for another); a harness that reports none answers it `-32601`. The report is built from `Event::Buckets`, `Event::Usage`'s `spend` and `TurnEnd::Limited`: a bucket replaces the earlier of its id and keeps what it leaves unknown, a change bumps `revision` and a repeat does not (only `observed_at` moves), a limit exhausts the bucket it names and is kept until its `resets_at` passes or a later bucket of that id shows room; `retry_after` is the harness's own word and is never worked out from `resets_at`. A client that set `notify` is sent `_coqui_acp/usage_update` `{sessionId, report}`, the whole report, on every change; `Settings::account` is the host's id for the account, over the client's. `Harness::read_usage` and `Conversation::read_usage` (default `None`) re-read the account with no turn, for a harness whose `UsageSupport::needs_turn` is false: each `_coqui_acp/usage` request asks one open session (the harness when none is open), a changed reading is a change (revision, notification) and an unchanged one is not, and the harness is read on a thread of its own once a session has opened and once each turn has ended. A harness with `needs_turn` is never asked, and one with `needs_session` (advertised as `"needs_session": true`) is asked only through an open session: with none open, `Harness::read_usage` is not called and the answer is what the connection has seen. `UsageTracker` (`new(support, account)`, `reports`, `notifies`, `client(notify, account)`, `snapshot(session, now)`, `buckets(incoming, now) -> bool`, `spend(session, spend) -> bool`, `limit(hit, now) -> bool`, `replace(prefix, incoming, now) -> bool`) is the connection's report with these rules, public for a host that reads buckets itself and merges them the same way; a `bool` answers whether the report changed other than in when it was read. `replace` takes a complete reading of the buckets whose ids start with `prefix`: each replaces the held bucket of its id outright (nothing of an earlier reading, such as a reset that has passed, is merged back), a held bucket of that prefix the reading leaves out is dropped, other ids are untouched.
- `session/cancel` calls `interrupt` once per turn, after the turn's prompt has reached the harness; the turn's end is answered `cancelled`. If it has not ended within `cancel_grace` it is answered `cancelled` anyway, its later end is dropped, and a prompt is refused until it comes.
- `session/set_config_option` (`configId`, `value` an id) calls `set_option` and answers `{configOptions}`: `id` in version 1, `configId` in version 2.
- `session/close` answers a running turn `cancelled`, closes the harness's session and lets go of it, and answers `{}`. Anything else with an id is method not found.
- A permission request is `session/request_permission`: version 1 `{sessionId, toolCall, options}`; version 2 `{sessionId, title, description, subject: {type: "tool_call", toolCall}, options}`, with `requires_action` before it and `running` after. An answer naming an option not offered, an outcome nobody knows, or an error is `Outcome::Refused` (this request alone is refused); the cancelled outcome, no answer within `permission_timeout`, or a client that has gone is `Outcome::Cancelled`; so is a question still open when its turn ends, is answered cancelled after `cancel_grace`, or its session closes, and the client's later answer is dropped.

What each event becomes:

| Event | Version 1 | Version 2 |
| --- | --- | --- |
| user, message, thought chunks | `user_message_chunk`, `agent_message_chunk`, `agent_thought_chunk` | the same, with `messageId` |
| their ends | nothing | `user_message`, `agent_message`, `agent_thought` with no `content`, since `[]` clears the message |
| `Tool` | `tool_call` first, then `tool_call_update`; a diff as `{path, oldText, newText}`; `cancelled` as `failed` | `tool_call_update` only, with `name`; a diff as `{changes, patch}` |
| `ToolChunk` | `tool_call_update` with the whole content so far | `tool_call_content_chunk` |
| `Terminal` | its output added to its tool call's content | `terminal_update`, output base64-encoded |
| `Plan` | `plan` | `plan_update` with `planId` `plan` |
| `Usage` with context | `usage_update` (`used`, `size`, `cost` in USD) | the same |
| `Commands` | `available_commands_update` | the same, each tagged `type: "command"` |
| `Options` | `config_option_update` | `configOptions`: every choice with its current value |
| `Title` | `session_info_update` | the same |
| `Notice`, `Denied` | nothing | `notice` |
| `Compaction` | nothing | `compaction_summary_chunk`, `compaction_update` |
| `Exited` | the running turn fails | the same |

`Settings` (`#[non_exhaustive]`, `new()` and `Default`; set its fields): `max_line` (64 MiB; a longer client line ends the connection), `max_sessions` (8), `max_requests` (64 at once), `max_prompt_bytes` (16 MiB), `max_version` (2), `cancel_grace` (10 s), `permission_timeout` (`None`: as long as the person takes), `max_tools` (4096 tool calls tracked per session for version 1; past it the oldest is forgotten, and a later report of it is a new `tool_call`), `max_queued_events` (4096 events queued for a client that has not read them; past it the harness's `EventSink::send` waits, so a client that does not read holds the harness back; the same many are kept of what a session says before its answer, and past it they are dropped and the client told how many), `max_open_questions` (64 open at once per session, each holding a thread; past it one is answered cancelled without being asked), `client_mcp_servers` (true), `working_roots` (empty: any; links followed; also on `OpenRequest`, whose `path_admitted` holds what a harness reads from the request's other parts to the same rule, and for a harness that finds a session by its id alone), `network_paths` (false, as written or where links lead), `max_tool_content_bytes` (1 MiB) and `max_tool_content_items` (256) (version 1 sends a tool call's content whole each time; past them the rest is left out and the client told), `on_notice: Option<NoticeHook>` (called with each notice and each harness exit, for a host that logs them), `on_usage: Option<UsageHook>` (called with the connection's report, as `_coqui_acp/usage` answers it with no session, each time it changes, with no lock held; none), `account: Option<String>` (the host's id for the account, echoed into usage reports; none).

## Modes (`coqui_acp_harness::mode`)

```rust
#[non_exhaustive] pub enum Mode { Manual, AcceptEdits, Auto, Plan, Bypass }   // Clone, Copy, Debug, PartialEq, Eq, Hash
pub const ALL: [Mode; 5]   // Mode::ALL, in the order a client cycles them
pub const fn id(self) -> &'static str          // manual, accept_edits, auto, plan, bypass
pub const fn name(self) -> &'static str;  pub const fn description(self) -> &'static str
pub fn from_id(id: &str) -> Option<Mode>
pub const OPTION_ID: &str = "mode"
pub fn option(current: Mode, offered: &[Mode]) -> ConfigOption          // category `mode`; values in `Mode::ALL` order; `current` is listed even when not offered
pub fn chosen(value: &str, offered: &[Mode]) -> Result<Mode, String>    // the reason, for `HarnessError::Refused`
pub const fn looseness(self) -> u8                                       // 0 for Plan up to 4 for Bypass, loosest last
pub fn up_to(loosest: Option<Mode>, allow_bypass: bool) -> Vec<Mode>    // the host's mode and every stricter one; None: Plan only; Bypass only if allowed
pub fn widened(base: &[Mode], named: &[Mode], allow_bypass: bool) -> Vec<Mode>   // `base` and the modes a host named, Bypass only if allowed
pub const CUSTOM: &str = "custom"
pub fn custom_option(label: &str, offered: &[Mode]) -> ConfigOption     // a state no mode names, shown as `Custom: label` and not choosable
```

How much a harness asks, named once so a client shows one control over every translator. A translator offers only the modes its tool has, and a mode in use is always listed. Each translator's table of what a mode sets is in its own header: `coqui-acp-claude` (`permissionMode`) and `coqui-acp-codex` (approval policy, sandbox, reviewer, collaboration mode).

## Antigravity (`coqui_acp_harness::antigravity`, feature `antigravity`)

```rust
pub struct AgyQuota;                     // new(ReadLimits); unmatched() -> u64; Debug
impl AgyQuota { pub fn read(&mut self, owners: &[u32], models: &[AcpModel]) -> Result<Vec<Bucket>, ReadError>; }
#[non_exhaustive] pub struct AcpModel { pub id: String, pub name: String }   // new(id, name)
#[non_exhaustive] pub struct ReadLimits { pub body: usize, pub connect: Duration, pub total: Duration, pub min_interval: Duration }
// Default: 256 KiB, 1 s, 3 s, none. new(), then body(), connect(), total(), min_interval().
#[non_exhaustive] pub enum ReadError { NoPort, NotOwned, Status(u16), Malformed, Io(io::ErrorKind) }   // Display, Error
pub fn loopback_listeners(owners: &[u32]) -> io::Result<Vec<SocketAddr>>   // ends the system's tool after 3 s
pub const BUCKET_PREFIX: &str = "agy:";   // pass to UsageTracker::replace
```

`agy acp` serves its language server on two random loopback ports (one plain HTTP, one HTTPS with a self-signed certificate) once a session is open, and answers `POST /exa.language_server_pb.LanguageServerService/GetUserStatus` with body `{}` and no token on both, with `Transfer-Encoding: chunked`. `read` lists the loopback listeners of `owners` (`loopback_listeners`: Windows `netstat.exe -ano` for `TCP` and `TCPv6`, ignoring the State column; Linux `/proc/<pid>/fd` joined with `/proc/net/tcp` and `tcp6`; macOS `/usr/sbin/lsof`; no `unsafe`), asks each by plain HTTP and then by HTTPS (certificate verification off, for a literal loopback address only, which `read` checks), and takes the first answer that reads as agy's status. `owners` are all the agent's processes: the reader does not find them. The port and scheme that answered are remembered and the owner is checked again before every reading; a failed reading forgets the port, so the next looks at every port again. Listing the ports counts against `limits.total` (the tool is ended at the deadline, and on Windows started with no console window).

No redirects, no proxy, the body counted as it is read against `limits.body` (an over-large body is `Malformed`), `limits.connect` for each connection and `limits.total` for the whole reading. An error never holds any of the answer. The answer is read into a struct with only each model's `label`, `modelId` and `quotaInfo`; the account's address, name and picture never become values. Non-200 is `Status`; an unparsable 200, an answer with no model list or a body past the limit is `Malformed`; no listener is `NoPort`, and a remembered port that no longer belongs to `owners` with nothing else answering is `NotOwned`.

Buckets, `reported`, unit `permille`, `limit` 1000, `used` unset: one for each distinct reset time (the Gemini 4 Argon models share seven days, the others five hours), id `agy:<first model's id>` (its `modelId`, else its label), `remaining` the least of its models' `round(remainingFraction * 1000)` (1000 only for exactly 1, 0 only for exactly 0, and 1 to 999 between), `exhausted` at 0 and `ok` otherwise (agy gives no warning). A fraction left out beside a valid reset is 0: the answer is protobuf JSON, which omits zero values (not yet seen from a real exhausted account). A model with no `quotaInfo`, a fraction that is present but not a number in `0..=1`, or a reset that is not RFC 3339 is left out. At fraction 1 the window has not started and agy reports a reset a whole window away that moves with every reading, so `resets_at` is `None` and the window length (reset less now, to the minute) is remembered for each of the group's models; a used bucket takes the length its models remember, so one that splits off a group keeps it, and carries the real `resets_at`. A reading is complete: pass its buckets to `UsageTracker::replace(BUCKET_PREFIX, ..)`, so a bucket a group no longer makes is dropped. The `label` is the length (`5 h`, `7 d`) when known. Coverage is `only` the ACP ids its models match, by `modelId` then by `label` against `AcpModel::name`, and `unknown` when any does not match (counted by `unmatched()`). `min_interval` answers a reading sooner than that after the last success with its buckets.

Not read: `planStatus` credits (the unit is unclear) and `userTier.name` (the host owns the account label). Recorded answer: `tests/fixtures/agy-1.5.3/getuserstatus.json`, with the account's address, name and picture replaced.

## Versions (`coqui_acp_harness::version`)

```rust
pub fn untested(harness: &str, found: &str, tested: &[&str]) -> Option<String>
```

The warning for a version a driver was not tested with, or `None`. Nothing is refused for it.

## Errors

`serve` fails with `io::Error` when reading the client failed, or `InvalidData` for a line over `max_line`. `HarnessError`'s `Display` is `refused: …` or `failed: …`.

## Gotchas

- Version 1 never gets a version 2 kind, and version 2 never gets `tool_call`.
- `serve` always answers a `Responder`; one dropped unanswered answers `Outcome::Cancelled`.
- A question that comes when no turn is running, or past `max_open_questions`, is answered cancelled and never asked.
- A cancel before the prompt reached the harness has the same grace: the turn is answered cancelled, and the prompt is interrupted if it arrives after all.
- Events sent before `open` returns are told after `session/new`'s answer, since a client drops updates of a session it does not know yet.
