# `coqui-acp-codex` — API header

Header-file-style reference for the `coqui-acp-codex` crate: every public item with its signature, contract and gotchas, so the crate can be used without reading its source. It ships inside the package and is self-contained. Development documentation lives with the source at <https://github.com/Boriken-Dev/coqui-acp>.

Cargo feature `history` (off by default): export and import of Codex's rollout in the standard history format. MSRV Rust 1.89. A default build depends on `coqui-acp-api`, `coqui-acp-harness` (with its `kind` feature), `serde_json` and, for the command, `clap`. Version `0.0.0-alpha.0`, not published; the API may change in any release before 1.0. Tested with Codex 0.164.0 and 0.166.0 (`TESTED_VERSIONS`).

Imports: everything is at the crate root, and `parse` is public. `coqui_acp_codex::harness` is `coqui-acp-harness`, for its events, processes and ACP server; `Launch`, `PlainSpawn`, `Spawn` and `Spawned` are re-exported from it.

No `unsafe` code (`#![forbid(unsafe_code)]`). No process-wide state.

## Launch (`CodexLaunch`)

```rust
#[non_exhaustive]                                       // Clone, PartialEq; Debug shows env names only
pub struct CodexLaunch {
    pub executable: OsString,                  // "codex"
    pub cwd: PathBuf,
    pub thread: ThreadStart,                   // New | Resume(id) | Load(id)
    pub model: Option<String>,
    pub effort: Option<String>,
    pub developer_instructions: Option<String>,
    pub mcp_servers: Vec<McpServer>,
    pub additional_directories: Vec<PathBuf>,
    pub working_roots: Vec<PathBuf>,           // a resumed thread's own directory must lie in one; empty takes any
    pub approval_policy: Option<String>,       // untrusted, on-failure, on-request, never
    pub sandbox: Option<String>,               // read-only, workspace-write, danger-full-access
    pub allow_bypass: bool,                    // false: the Bypass mode is not offered
    pub modes: Option<Vec<Mode>>,              // those named widen the host's mode and stricter ones (Bypass still needs allow_bypass)
    pub thread_config: Map<String, Value>,     // merged into the thread's config, as given
    pub config_overrides: Vec<(String, Value)>,
    pub extra_args: Vec<OsString>,             // the caller's own, after the overrides, unchecked
    pub client_info: ClientInfo,               // how it names itself in initialize; this crate by default
    pub env: BTreeMap<OsString, OsString>,
    pub clear_env: bool,
}
pub fn CodexLaunch::new(cwd: impl Into<PathBuf>) -> CodexLaunch
pub fn launch(&self) -> Launch                 // app-server and one --config=key=value per override
pub fn config(&self) -> Map<String, Value>     // mcp_servers, sandbox_workspace_write.writable_roots, thread_config
pub fn skill_roots(&self) -> Vec<String>       // the Agent Skills directory of each
pub fn has_server(&self, name: &str) -> bool   // in mcp_servers or thread_config
pub fn thread_params(&self) -> Value           // thread/start's, or thread/resume's with threadId and excludeTurns
#[non_exhaustive] pub enum ThreadStart { New, Resume(String), Load(String) }
#[non_exhaustive] pub struct ClientInfo { pub name: String, pub title: String, pub version: String }
pub fn ClientInfo::new(name, version) -> ClientInfo
pub fn valid_config_key(key: &str) -> bool;  pub fn toml_value(value: &Value) -> String
pub fn valid_thread_id(id: &str) -> bool       // letters, digits and dashes, 1 to 128
```

The command line holds only `app-server` and the overrides, each one argument `--config=<key>=<value>`: a key part that is not letters, digits, `_` and `-` is a quoted TOML string, and every value is TOML written here (strings with quotes, backslashes and control characters escaped, arrays, objects as inline tables, `null` as `""`), so no value can end its own or start another. MCP servers are `config.mcp_servers.<name>`: `{command, args, env}` for stdio, `{url, http_headers}` for HTTP; Codex has no SSE.

## Session (`CodexSession`)

```rust
pub fn CodexSession::start(launch: &CodexLaunch, spawn: &dyn Spawn, settings: CodexSettings,
    sink: impl Fn(Event) -> bool + Send + Sync + 'static) -> Result<CodexSession, Error>
pub fn thread(&self) -> &str
pub fn version(&self) -> Option<&str>
pub fn models(&self) -> Option<&[Model]>                        // None: not listed or could not be
pub fn choice(&self) -> (Option<String>, Option<String>)        // model and effort the next turn names
pub fn set_model(&self, model: Option<&str>);  pub fn set_effort(&self, effort: Option<&str>)
pub fn prompt(&self, content: &[Content]) -> Result<(), Error>
pub fn interrupt(&self) -> Result<(), Error>
pub fn steer(&self, turn: &str, content: &[Content]) -> Result<(), Error>   // turn/steer, its answer not awaited
pub fn request(&self, method: &str, params: Value, limit: Duration) -> Result<Value, Error>
pub fn read_usage(&self) -> Result<Vec<Bucket>, Error>      // account/rateLimits/read: needs no turn; the answer is the base later updates merge into
pub fn CodexSession::read_models(launch: &CodexLaunch, spawn: &dyn Spawn, settings: CodexSettings) -> Result<Option<Vec<Model>>, Error>   // starts app-server, initializes, lists models, ends it: no thread, no turn
pub fn CodexSession::read_account_usage(launch: &CodexLaunch, spawn: &dyn Spawn, settings: CodexSettings) -> Result<Vec<Bucket>, Error>   // starts app-server, initializes, reads, ends it: no thread, no turn
pub fn close(&self)                                              // also on drop
#[non_exhaustive] pub struct Model { pub id: String, pub name: String, pub description: Option<String>,
    pub efforts: Vec<String>, pub default_effort: Option<String>, pub is_default: bool }
pub fn models_of(result: &Value) -> Option<Vec<Model>>          // hidden models left out; None: no list at all
pub fn version_of(answer: &Value) -> Option<String>             // from initialize's userAgent
pub fn user_input(content: &[Content]) -> Result<Vec<Value>, Error>
```

`start` spawns the app server, sends `initialize` (as `coqui-acp-codex`) and `initialized`, lists `model/list` page by page, sends `skills/extraRoots/set` when there are directories, and `thread/start` or `thread/resume` with `thread_params`. A resumed thread other than the one asked for is `Error::Harness`, and the app server is ended. For `Load` the earlier turns of `thread/resume`'s answer go to `sink` as history, the person's messages included, the latest `max_history_turns` of them. A version not in `TESTED_VERSIONS` is an `Event::Notice`. Events reach `sink` from a thread of the session's own until it answers `false`; the output's end is one `Event::Exited`, saying how Codex ended and the last lines it wrote that were not protocol, standard error's included; none after `close`.

A `turn/start` that goes unanswered within `request_timeout` fails the prompt, and that turn is interrupted whenever it shows itself: its answer, its start and its end touch no later turn. `prompt` sends `turn/start` with the input (text; an image as a data URL; a link as `[@name](uri)`; a text resource in `<context ref="uri">…</context>`; an image resource as an image; any other resource is `Error::Refused`) and the chosen model and effort, and returns once Codex has answered. The first prompt's first line, cut to 80 bytes, is the `Event::Title`. `interrupt` sends `turn/interrupt` with the running turn's id and does not wait; with no turn running it does nothing.

`CodexSettings` (`new()`, `Default`; set its fields): `max_line` (64 MiB), `tail_lines` (8), `tail_line_bytes` (512), `initialize_timeout` (20 s), `request_timeout` (30 s), `exit_wait` (1 s), `max_tools` (4096 items kept), `max_model_pages` (10), `max_history_turns` (1000). Also: `parser` (`ParserLimits`: `max_title_bytes` 256, `max_told_bytes` 16 KiB, `max_streamed` 64), `list_models` (true; Codex's models failing to list stops nothing), `exit_poll` (100 ms), `close_wait` (2 s), `max_queued_writes` (1024), `title_bytes` (80), `version_wait` (10 s; 0 does not ask), `version_bytes` (4096), `on_line: Option<LineHook>`, `check_config: Option<AnswerCheck>` (asks `config/read` before the thread; a refusal fails the start), `check_thread: Option<AnswerCheck>` (the answer that opened the thread; the same). A resumed or loaded thread whose own directory lies outside `working_roots`, links followed, is refused.

## Parser (`coqui_acp_codex::parse`)

```rust
pub type Writer = Arc<dyn Fn(&Value) -> io::Result<()> + Send + Sync>;
pub type Answer = (String, Result<Value, String>);
pub const ALLOW_ONCE: &str = "accept";  pub const ALLOW_SESSION: &str = "accept_for_session";
pub const ALLOW_RULE: &str = "accept_with_rule";  pub const ALLOW_ALWAYS: &str = "accept_always";  pub const REJECT: &str = "decline";
pub const ALLOW_NETWORK_RULE: &str = "allow_network_rule";  pub const REJECT_NETWORK_RULE: &str = "reject_network_rule";
pub fn Parser::new(writer: Writer, max_tools: usize) -> Parser
pub fn Parser::with_limits(writer: Writer, max_tools: usize, limits: ParserLimits) -> Parser
pub fn kept_output(&self) -> usize
pub fn line(&mut self, message: &Value) -> Vec<Event>
pub fn history(&mut self, turn: &Value) -> Vec<Event>
pub fn turn(&self) -> Option<&str>;  pub fn started(&mut self, turn: &str);  pub fn interrupting(&mut self)
pub fn set_thread(&mut self, thread: &str);  pub fn prompting(&mut self);  pub fn abandon(&mut self, request: &str)
pub fn answers(&mut self) -> Vec<Answer>
pub fn id_text(id: &Value) -> String
pub fn git_patch(path: &str, change: FileChange, hunks: &str) -> String
```

| Message | Events |
| --- | --- |
| `item/agentMessage/delta`, `item/completed` `agentMessage` | `MessageChunk` by item id, `MessageEnd`; text not streamed is told whole |
| `item/reasoning/summaryTextDelta`, `textDelta`, `item/completed` `reasoning` | `ThoughtChunk`, `ThoughtEnd` |
| `commandExecution` items, `item/commandExecution/outputDelta` | `Tool` (`exec_command`, execute, its terminal) and `Terminal`: the command, output, exit code; `declined` fails |
| `fileChange` items | `Tool` (`apply_patch`, edit) with a diff per change, its `patch` a git patch of Codex's hunks |
| `mcpToolCall`, `dynamicToolCall` | `Tool` named `mcp__<server>__<tool>`, its result or error text as a `ToolChunk` |
| `webSearch`, `imageView` | `Tool` |
| `turn/plan/updated` | `Plan` |
| `thread/tokenUsage/updated` | `Usage` of `last`: cached input split out, not cumulative, no cost, context used and `modelContextWindow`; its `spend` is the thread's `total` (cached split out, reasoning kept, `cost_micro_usd` unset), which a resumed thread carries over from its earlier turns; another thread's has no `spend` |
| `account/rateLimits/updated` | merged into the last `account/rateLimits/read` (a sparse update: an object merges key by key, a null clears nothing), then `Buckets` for that limit when its buckets changed, none when they did not; the exhausted window kept for the limit error that follows |
| `account/rateLimits/read` answer | `Buckets` of every limit (`CodexSession::read_usage`, asked by the harness once the session has opened, after each turn and on each poll, through `Conversation::read_usage`; the answer is applied where it is read, so an update read after it is kept, and a used-up window in it names a later limit error): `rateLimitsByLimitId`, else `rateLimits`; each snapshot's window is `<limitId>:<five_hour|weekly|primary|secondary>` (300 min, 10080 min, else its position), `usedPercent` as permille of 1000, a fixed window with `windowDurationMins` as its length and `resetsAt`, `ok` below 100 and `exhausted` from 100; `limitName` the label; `normalModelSlug` makes the coverage `only {slug}`, else unknown; `individualLimit` is `<limitId>:spend_control` (permille `1000 - remainingPercent * 10`, exhausted when `spendControlReached` or none remains); `ordinaryUsageAllowed: false` holds every bucket exhausted until a later read says otherwise (Codex: recovery is not to be inferred from percentages); `credits.balance` is left unmapped, its unit being unknown |
| `turn/completed`, `turn/failed` | `TurnEnd`: a limit (`codexErrorInfo` naming a usage or rate limit, or a failed connection with HTTP 429; reset from `resetsAt` else the exhausted window) is `Limited`; `interrupted`, or `failed` after `interrupting`, is `Stopped(Cancelled)`; `failed` is `Failed` with the error's message; else `Stopped(EndTurn)` |
| `error` | kept for the running turn's end, unless Codex will retry or it names another turn |
| `thread/compacted`, `contextCompaction` items | `Compaction` |
| `warning`, `configWarning`, `deprecationNotice`, `guardianWarning`, an MCP server that failed to start | `Notice` |
| `thread/name/updated` | `Title` |
| an answer to our request | kept for `answers`, never an event |

Approvals are `Event::Permission`; their responders answer the server request by its own id:

| Request | Options | Answer |
| --- | --- | --- |
| `item/commandExecution/requestApproval` | `accept`, `accept_for_session`, `accept_with_rule` when Codex proposed a rule, `decline` | `accept`, `acceptForSession`, `{acceptWithExecpolicyAmendment}`, `decline`; nobody answered: `cancel` |
| `item/fileChange/requestApproval` | `accept`, `accept_for_session`, `decline` | the same words |
| `item/permissions/requestApproval` | `accept` (this turn), `accept_for_session`, `decline` | the permissions asked with `scope`, or none |
| `mcpServer/elicitation/request` of an MCP tool | `accept`, `accept_for_session` and `accept_always` as Codex's `persist` allows, `decline` | `{action: "accept", _meta: {persist}}`, `decline`; nobody answered: `cancel` |
| `execCommandApproval`, `applyPatchApproval` | `accept`, `accept_for_session`, `decline` | `approved`, `approved_for_session`, `{denied}`; nobody answered: `abort` |

Any other server request is answered `-32601` at once.

### Export and import (feature `history`, `coqui_acp_codex::history`)

Cargo feature `history` (off by default) depends on `coqui-acp-history` and adds the module, the provider's `import_session` and `export_session` (without it they answer `GUATIAO_ERR_UNSUPPORTED`) and the `history` subcommand of the program.

```rust
pub const HARNESS: &str = "codex";  pub const CONTENT_DROPPED: &str = "content_dropped";  pub const DEFAULT_MAX_BYTES: u64 = 256 << 20;
pub fn codex_home(env: &BTreeMap<OsString, OsString>, clear_env: bool) -> Option<PathBuf>   // CODEX_HOME, else .codex in the home directory
pub fn rollout(home: &Path, id: &str) -> Result<PathBuf, Error>                      // the one sessions/Y/M/D/rollout-*-<id>.jsonl, not through a link
pub fn export(text: &str, id: &str) -> Result<Document, Error>
pub fn import(doc: &Document, cwd: &str, id: &str, options: &ImportOptions) -> Result<Imported, Error>
pub fn export_file(home: &Path, id: &str, max_bytes: u64) -> Result<Document, Error>
pub fn import_file(home: &Path, cwd: &Path, id: &str, doc: &Document, options: &ImportOptions) -> Result<Imported, Error>
#[non_exhaustive] pub struct ImportOptions { pub use_summary: bool /* true */, pub version, model_provider, started: Option<String> }   // Default; builders of the same names
#[non_exhaustive] pub struct Imported { pub text: String, pub report: LossReport, pub seeded_from: Option<SeedPoint>, pub closed_calls: u64, pub closed_turn: bool, pub path: String }
```

`coqui-acp-codex history export [--config DIR] ID` writes the history of a thread to standard output, as it is; `history import [--config DIR] [--no-summary] [--model-provider ID] CWD ID` reads one from standard input, writes the rollout (Codex resumes a thread only under the model provider it names, `openai` unless told: a provider with another id needs the same one here) and prints what it lost on standard error. `--config` is a Codex home, by default `CODEX_HOME` else `.codex`. A rollout of that thread is never replaced.

**Export** reads `response_item`, `compacted` and the turn events of a rollout, in either of its forms (older rollouts, and 0.166's paginated ones). What Codex adds to a thread by itself is not part of the history: the base instructions, developer messages, `additional_tools`, `<environment_context>`, AGENTS.md and permission instructions, `world_state`, `turn_context` settings (only the model and effort go to the header), token counts and the other events. Codex adds its own again on the next turn, so nothing of it is carried or reported. A user message is a prompt unless Codex's own `content_item_kinds` (or, in older rollouts, its tag) says it is context; an assistant message is `agent_message_chunk`; reasoning is `agent_thought_chunk` (summary text, with the summary, id, raw content and encrypted content in `_meta`); `function_call` (`rawInput` the parsed arguments), `custom_tool_call` (`rawInput` `{"input": text}`: `exec`'s script, `apply_patch`'s patch), `local_shell_call` and `web_search_call` are `tool_call`s with the name Codex gave (ACP version 2's `name`), a `kind` from the tool table (else `execute` for shells and `exec`, `edit` for `apply_patch` with its paths as `locations`, `think` for `update_plan`) and their outputs `tool_call_update`s with `rawOutput` exactly as Codex has them (`failed` for `success: false` and a script that says `Script failed`). `task_complete` ends a turn `end_turn`, `turn_aborted` `cancelled`, and an `error` event makes the turn `error`. A `compacted` item is a `history/compaction` whose summary is its message without Codex's fixed lead-in (the whole message in `_meta`). `_meta` (`harness` `codex`): `item`, `arguments`, `itemId`, `namespace`, `summary`, `id`, `content`, `encrypted`, `message`.

**Import** writes the older (legacy) form of a rollout, which every Codex version reads and which shows the turns on `thread/resume`: `session_meta` with the host's UUID as the thread id and the host's directory, `task_started`, the `response_item`s with the matching `user_message`, `agent_message` and `agent_reasoning` events, `task_complete` or `turn_aborted`. The file is `sessions/<year>/<month>/<day>/rollout-<time>-<UUID>.jsonl` from the history's `createdAt` (the time of the import without one); Codex's own instructions, environment and tools are not written, and Codex adds them after the history on its next turn (measured on 0.166.0 against the stand-in model). With `use_summary` the latest compaction summary becomes a `compacted` item holding it in Codex's slot; without, none is written. A tool call is written as Codex's own when the origin is Codex (the arguments text it had, while it says the same), and otherwise through the normalised tool table: `shell`/`shell_command` as a `shell_command` function call, and Write and Edit as an `apply_patch` custom call (a whole file added, or one run of lines replaced); a call the table does not name (Read, Grep, Glob, WebFetch, an MCP tool) is written as a text note of the call and its result and counted `tool_as_note`. Reasoning is written only for a history from Codex, with the encrypted content it had, and otherwise dropped and counted `thinking`. Content blocks are counted `attachment_by_digest` or `content_dropped` as for Claude Code. The hostile-history rules are kept as for Claude Code: calls without outputs get an output saying they did not finish and an open turn ends `turn_aborted`; no permission, approval or execution-policy rule is written; the file's name and place come from the host's UUID, directory and `createdAt` (checked as RFC 3339), never from a name in the history.

An empty history seeds nothing on disk either: Codex gives a thread an id of its own (`thread/start` takes none), so the provider keeps the host's id (`CodexAgent::seeded_sessions`, shared with the agents it starts), the first open of it by `session/resume` or `session/load` starts a thread and keeps which, later opens reach that thread (a thread nothing was said in is not kept by Codex, so it starts over), and every session id the agent reports is the host's. Measured on Codex 0.166.0: a thread started and never prompted cannot be resumed (`no rollout found`); a prompted one resumes under its own id.

The provider behaves as Claude Code's does, an unknown history version being `UNSUPPORTED` too (a local spawner; the grant's `state_dir` is the Codex home for a login kind).

## ACP agent (`CodexAgent`)

```rust
pub fn CodexAgent::new() -> CodexAgent                    // also Default; codex from PATH, PlainSpawn
pub fn defaults(self, defaults: CodexLaunch) -> Self
pub fn spawn(self, spawn: Arc<dyn Spawn>) -> Self
pub fn settings(self, settings: CodexSettings) -> Self
pub fn cli_version(&self) -> Option<String>               // codex --version, once, within CodexSettings::version_wait
impl Harness for CodexAgent
pub const DEFAULT: &str = "default";
```

`agentInfo` is `coqui-acp-codex`, titled `Codex <version>`, at this crate's version. It takes images, embedded context, MCP servers over stdio and HTTP, load, more directories and an appended system prompt. A session's parts are added to `defaults`: its `cwd`, `systemPrompt` after the defaults' developer instructions, `mcpServers`, and `additionalDirectories`, each a skill root (the Agent Skills directory inside it) and writable in a `workspace-write` sandbox. An SSE server or a thread id `valid_thread_id` refuses is refused. The session's id is the thread's.

The mode option (`mode`, category `mode`; ids from `coqui_acp_harness::mode`; `coqui_acp_codex::mode` holds the table), measured on Codex 0.166.0 from `app-server generate-json-schema` and a `thread/start` with no turn. Approval policy is `untrusted`, `on-request` or `never`; the sandbox `read-only`, `workspace-write` or `danger-full-access`; the approvals reviewer `user`, `auto_review` or `guardian_subagent`; the collaboration modes (`collaborationMode/list`) `plan` and `default`.

| Mode | `approvalPolicy` | `sandboxPolicy.type` | `approvalsReviewer` | `collaborationMode.mode` |
| --- | --- | --- | --- | --- |
| `manual` | `untrusted` | `workspaceWrite` | `user` | `default` |
| `accept_edits` | `on-request` | `workspaceWrite` | `user` | `default` |
| `auto` | `on-request` | `workspaceWrite` | `auto_review` | `default` |
| `plan` | `on-request` | `readOnly` | `user` | `plan` |
| `bypass` | `never` | `dangerFullAccess` | `user` | `default` |

What a client may choose is capped by the host: the mode the launch's `approval_policy` and `sandbox` amount to (`coqui_acp_codex::mode::configured`; the thread's real state when the launch sets neither) and every stricter one, in the order Plan, Manual, Accept edits, Auto, Bypass. `CodexLaunch::modes` names more; Bypass is offered only with `allow_bypass`, whatever is named. A host whose policy and sandbox amount to no mode offers Plan only.

The current mode at the start is read from `thread/start`'s `approvalPolicy`, `approvalsReviewer` and `sandbox`, exactly: it is a mode only when all three are what the mode sets (Plan never reads back: a thread has no collaboration mode to report, so its `on-request`, `readOnly`, `user` is shown as it is (`custom`) until a client chooses Plan). Any other pair shows as `custom` with its real values (`Custom: untrusted, readOnly`), never as a mode, and cannot be chosen. Choosing the mode shown still applies it from the next turn. Until a client chooses a mode no turn names any of the four settings, and the thread's own configuration holds; once one has, each `turn/start` carries all four (`sandboxPolicy` keeps the thread's `sandbox_workspace_write` roots and network setting), so a switch holds from the next turn, not the one running. `collaborationMode.settings` names the model (the turn's, else the thread's, else Codex's default; with none the turn carries the approval and sandbox only and a warning), the session's effort (else the thread's `reasoningEffort`) and the launch's developer instructions, each only when there is one: Codex's schema says a collaboration mode takes precedence over the thread's model, effort and developer instructions, and that `null` instructions mean the mode's built-in ones, so nothing is sent as `null`. A session with developer instructions has them replace Plan's built-in ones in a Plan turn. A `workspace-write` sandbox falls back to `readOnly` in a directory the Codex home does not trust, as `thread/start` answers show.

Config options: `model` (category `model`): `default` and the models Codex lists; `effort` (category `thought_level`): `default` and the current model's efforts. Setting one not offered is refused; a choice applies from the next turn.

## Provider (`CodexProvider`, id `codex`)

```rust
#[derive(Provider)] pub struct CodexProvider;                        // an `acp_harness` provider; Debug
pub fn CodexProvider::new(settings: ProviderSettings) -> CodexProvider    // also From<ProviderSettings>
pub struct ProviderSettings {                                        // ToValue, FromValue, Schema, Default; every field defaults
    pub executable: String,              // "codex": a path, or a name looked up where the spawner runs it
    pub model: Option<String>, pub effort: Option<String>,
    pub approval_policy: Option<String>, pub sandbox: Option<String>,
    pub allow_bypass: bool,              // false
    pub modes: Option<Vec<Mode>>,        // None; ids `manual`, `accept_edits`, `auto`, `plan`, `bypass`: widen what approval_policy and sandbox amount to and every stricter mode (Bypass still needs allow_bypass, and is left out without it)
    pub max_acp_version: u64,            // 2 (1 or 2)
    pub max_sessions: u64,               // 8
    pub max_requests: u64,               // 32: requests worked on at once; a request past it is refused
    pub max_message_bytes: u64,          // 64 MiB: one byte more ends the connection GUATIAO_ERR_BAD_VALUE
    pub client_mcp_servers: bool,        // true
    pub renew_poll_ms: u64,              // 30000: how often a connection on a login kept in a file looks at the file; 0 looks only at a call and at close
    pub command_timeout_ms: u64,         // 60000: the longest `codex login` may take to save a key for an account
}
pub mod accounts { pub fn catalog() -> AccountCatalog;  pub const CHATGPT, API_KEY, ACCESS_TOKEN: &str;
    pub const CHATGPT_METHOD, DEVICE_METHOD, API_KEY_METHOD, ACCESS_TOKEN_METHOD: &str;  pub const AUTH_FILE, FILE_KEY: &str; }
pub fn CodexSession::refresh_account(launch: &CodexLaunch, spawn: &dyn Spawn, settings: CodexSettings) -> Result<Value, Error>   // starts app-server, initializes, asks account/read with refreshToken: true, ends it: no thread, no turn
```

Register it with `Registry::register_local`, or build it into a library with `guatiao::providers!` (`examples/codex_provider.rs`, built by `cargo test`). Each connection serves a `CodexAgent` over the channel with `coqui_acp_harness::provider::Serving`; it starts `codex app-server` through the host's spawner. `examples/fake_codex.rs` is a stand-in for the app server that behaves as the contract agent does (it needs `CODEX_HOME` or the suite's home, keeps threads and the five-hour window there, answers `login` as recorded and `account/read`, rotating the refresh token when asked to refresh, and logs its arguments and environment names (never values) to `COQUI_ACP_FAKE_CODEX_LOG` and to `fake-codex.log` of the home; `fake-refresh` in the home holding `refuse-blank` or `refuse-delete` scripts a refused refresh, `fake-login` holding `refuse` a refused login); the suite runs against it as `acp-harness-check --library <codex_provider> --provider codex --settings '{"executable": "<fake_codex>"}' --program-agent --no-imports --holds none`; `--program-agent` runs the checker in a home of its own, which the stand-in uses when `CODEX_HOME` is not set.

- `describe`: title `Codex`, ACP versions 1 and 2, what `CodexAgent` takes and `session/resume`, `resumes_wider`, usage `reported` and `inferred` with spend, readable with no turn, and the cache hint: `cache_lifetime` says `ignored`, since Codex's app server has no setting for how long its prompt cache lives (`app-server generate-json-schema` of 0.166.0 names none).
- `refusal`, `start`: holds no requirement; each is `GUATIAO_ERR_UNSUPPORTED` with its name. `read_only` is not the read-only sandbox alone: an approval policy that asks lets Codex escalate, Bypass is still offered unless `allow_bypass` is off, Plan mode applies `on-request` again, and the MCP servers of the person's own configuration start outside the sandbox. What would make it enforceable: Bypass off and never offered, the harness declining file-change and escalation approvals itself instead of asking the person, and the configuration's MCP servers disabled with an override measured on the Codex in use (`-c mcp_servers={}` is the candidate; unmeasured).
- `accounts`: `harness-login` and the kinds measured on Codex 0.166.0, with every credential variable. All three keep their credential in `auth.json` of Codex's home (`state_dir`), and the provider writes `config.toml` there (`cli_auth_credentials_store = "file"`) when it is absent. `codex-chatgpt` (credential `auth_file`, the file's text; rotating; `renews`; setting `chatgpt_base_url`), `codex-api-key` (`api_key`; `openai_base_url`), `codex-access-token` (`access_token`; `chatgpt_base_url`; **taken not to rotate**, since its success path was not run and Codex checks the token offline with no refresh credential: a catalog fact to revisit once measured). A key or token is saved by `codex login --with-api-key|--with-access-token` in the home with the secret on standard input, never in an argument; a key already in `auth.json` is not saved again. Login methods: `chatgpt` (`codex login`, step `open_url`, not remote: the callback returns to this machine), `chatgpt-device` (`--device-auth`, `open_url` with a `user_code`, remote), `api-key` and `access-token` (`ask`).
- Applying a grant (`start`, `models`, `usage`): every credential variable is removed from what Codex starts with, and from the request's environment, `CODEX_HOME` is the `state_dir`, endpoint settings go as `--config` overrides, and the file of `codex-chatgpt` is written, watched and reported as in `coqui_acp_harness::account` (a changed `auth.json` is `renewed`, a missing or blank one `lost`). `BAD_VALUE` and `UNSUPPORTED` as for Claude Code. **What is removed** is `accounts::VARIABLES`, also `credential_variables`: the credentials, `CODEX_REFRESH_TOKEN_URL_OVERRIDE`, `CODEX_APP_SERVER_LOGIN_ISSUER`, `CODEX_REVOKE_TOKEN_URL_OVERRIDE` and `CODEX_SQLITE_HOME` (measured in the binary's strings) and `OPENAI_BASE_URL` (not measured), from the request's environment and the inherited one; the endpoint reaches Codex only as a `--config` override from the grant. A `codex-chatgpt` renewal reports `expires_at` from the `exp` of its access token, and one whose `tokens.account_id` differs from the grant's is refused as lost and logged. A token is saved with `codex login --with-access-token` once for a home while its `auth.json` is there (not on every `usage` poll).
- `models`: starts the app server through the spawner, lists `model/list` and ends it (no thread, no turn). `usage`: reads `account/rateLimits/read` the same way and merges it with what connections saw (`Reports`); a login with no allowances reports nothing.
- `renew`: for `codex-chatgpt`, the app server is asked `account/read` with `refreshToken: true` and the file is read: changed is `credentials`, unchanged `not_renewed_because`, missing or blank (or no account in the answer) `GONE`. Other kinds: `UNSUPPORTED`.
- `login`: `chatgpt` and `chatgpt-device` run `codex login [--device-auth]` in the login's home, relay the page (and the code) with `open_url`, and wait for the command to end, then read `auth.json` and check it with `codex login status`; `api-key` and `access-token` ask for the secret and save it on standard input (`BAD_VALUE` `login refused: ...` with the command's last line when it refuses). Cancelling or `timeout_ms` ends the command's tree and answers `GONE` within a moment. `import_session`, `export_session`: with the `history` feature the rollout's, as above; without it `GUATIAO_ERR_UNSUPPORTED` with a reason (after checking the account). `session_record`, `non_defaults`: the kind's defaults.

## Errors

`Error` (`#[non_exhaustive]`): `Spawn(io::Error)`, `Refused(String)`, `Timeout { what, limit }`, `Harness(String)` in Codex's words or saying how it ended. Into `HarnessError`: `Refused` stays refused, the rest is `Failed`.

## Command line

`coqui-acp-codex` serves `CodexAgent` on standard input and output. Flags: `--executable` (`COQUI_ACP_CODEX_EXECUTABLE`, default `codex`), `--model`, `--effort`, `--approval-policy`, `--sandbox`, `--allow-bypass`, `--max-acp-version` (1 or 2, default 2), `--max-sessions` (8), `--working-root` (repeated; a session's `cwd` and more directories must lie in one), `--no-client-mcp-servers`, `--allow-network-paths`. Notices and exits go to standard error. Exit status 0 when the client closes, 1 when reading it failed, 2 for a wrong invocation.

## Environment variables

`COQUI_ACP_CODEX_EXECUTABLE`, as `--executable`. Codex reads its own (`CODEX_HOME`, `OPENAI_API_KEY` and others) from the environment the agent runs in, or `CodexLaunch::env`.

## Gotchas

- `turn/interrupt` needs the turn's id, which `turn/start`'s answer and `turn/started` give.
- `availableDecisions` is what a command's approval offers; `decline` is always offered, since 0.164.0 and 0.166.0 take it where they list only `cancel`, mark the command `declined` and go on with the turn (each one's recording `turn`). A version after those is untested.
- A tool call is named as ACP's clients expect, `web_search` and `mcp__<server>__<tool>`; the server and tool are in the name.
- An `error` with `willRetry` is no limit: Codex may yet succeed. The turn's end decides, with the window an earlier report said was used up.
- `describe` says `usage.needs_turn` false: with no session open, a poll starts an app server for the read (`CodexAgent` as a `Harness`), so how often is the host's cost; with one open it asks that session's. A login with no allowances answers the read with an error, and nothing is reported. Measured on Codex 0.166.0 with no turn: one `codex` limit with a five-hour and a weekly window, `credits` and reset credits (`rateLimitResetCredits`, unmapped); `tests/fixtures/codex-0.166.0/rate_limits_read.stdout` is that answer with its account id and credit ids replaced. Every recorded `updated` line has null windows: it changes nothing.
- Another thread's usage comes as `Usage` with `elsewhere`, as spent and no part of this conversation's context; nothing else of it does.
- `--config` keys are not TOML: Codex splits the argument at its first `=` and the key at each `.`, and a quote is part of a name. A key holding `=` cannot be written and is refused. A JSON `null` value is the empty string, which is not unset.
- A login file needs `tokens.refresh_token` to be read, or Codex answers every turn `401`. Empty, a second client cannot rotate the person's login, and the access token goes on working until it expires.
- With `additionalDirectories` from a client, `workspace-write` writes where the client says, `/` included, with no approval. A host that does not trust its client gives `--working-root`; `--no-client-mcp-servers` keeps a client from starting programs.
- Whether Codex sends another thread's messages on this connection is not recorded. A notification of another thread is dropped, and a question of another thread says so.
- A question Codex settles itself (`serverRequest/resolved`) stays open at the client until the turn ends; a late answer is harmless.
- An MCP tool call asks through `mcpServer/elicitation/request` with `_meta.codex_approval_kind` `mcp_tool_call`, not a tool approval.
- Codex merges a thread's `config.mcp_servers` with the person's own configuration; a server of the same name in it wins.
- A fresh `CODEX_HOME` makes Codex set the machine up again; give it the login, or use the person's own.
- Everything the app server writes is the model's or Codex's: untrusted.
