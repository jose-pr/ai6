# `coqui-acp-claude` — API header

Header-file-style reference for the `coqui-acp-claude` crate: every public item with its signature, contract and gotchas, so the crate can be used without reading its source. It ships inside the package and is self-contained. Development documentation lives with the source at <https://github.com/Boriken-Dev/coqui-acp>.

Cargo feature `history` (off by default): export and import of Claude Code's transcript in the standard history format. MSRV Rust 1.89. A default build depends on `coqui-acp-api`, `coqui-acp-harness` (with its `kind` feature), `serde_json`, `uuid` and `clap` (the `cli` arguments). Version `0.0.0-alpha.0`, not published; the API may change in any release before 1.0. Tested with Claude Code 2.1.318 (`TESTED_VERSIONS`).

Imports: everything is at the crate root, and `parse` is public. `coqui_acp_claude::harness` is `coqui-acp-harness`, for its events, processes and ACP server; `Launch`, `PlainSpawn`, `Spawn` and `Spawned` are re-exported from it.

No `unsafe` code (`#![forbid(unsafe_code)]`). No process-wide state.

## Launch (`ClaudeLaunch`)

```rust
#[non_exhaustive]                                           // Clone, PartialEq; Debug shows env names only
pub struct ClaudeLaunch {
    pub executable: OsString,              // "claude"
    pub cwd: PathBuf,
    pub session: SessionStart,             // New(uuid) | Chosen | Resume(id)
    pub model: Option<String>,
    pub effort: Option<String>,            // low, medium, high, xhigh, max
    pub append_system_prompt: Option<String>,
    pub mcp_servers: Vec<McpServer>,
    pub mcp_config_file: Option<PathBuf>,  // the file --mcp-config names
    pub strict_mcp_config: bool,           // false
    pub add_dirs: Vec<PathBuf>,
    pub plugin_dirs: Vec<PathBuf>,
    pub permission_prompts: bool,          // true: --permission-prompts=host --permission-prompt-tool=stdio
    pub permission_mode: Option<String>,
    pub access: Option<Access>,            // None; with it, `effective_permission_mode()` (plan for read-only, else default/acceptEdits/bypassPermissions by approvals; `permission_mode` wins) and `access_settings()` (`--settings` JSON: a workspace sandbox sets sandbox.enabled and no unsandboxed retry; network off sets sandbox.network.strictAllowlist, with sandbox.filesystem.disabled when there is no sandbox)
    pub allow_bypass: bool,                // false: no `--allow-dangerously-skip-permissions`, and no Bypass mode offered
    pub modes: Option<Vec<Mode>>,          // those named widen the launch's mode and stricter ones (Bypass still needs allow_bypass)
    pub setting_sources: Option<Vec<String>>,   // None: Claude Code's default; Some(empty): none
    pub max_budget_micro_usd: Option<u64>,
    pub partial_messages: bool,            // true
    pub env: BTreeMap<OsString, OsString>,
    pub clear_env: bool,
    pub extra_args: Vec<OsString>,         // the caller's own, after the rest, unchecked
}
pub fn ClaudeLaunch::new(cwd: impl Into<PathBuf>) -> ClaudeLaunch    // a new session, its id a new UUID
pub fn session_id(&self) -> Option<&str>               // None when Claude Code is to choose it
pub fn launch(&self) -> Launch
#[non_exhaustive] pub enum SessionStart { New(String), Chosen, Resume(String) }
pub fn mcp_config(servers: &[McpServer]) -> Value        // {"mcpServers": {name: {type, command, args, env} | {type, url, headers}}}
pub fn valid_session_id(id: &str) -> bool                // letters, digits and dashes, 1 to 128
```

`launch` gives `-p --input-format=stream-json --output-format=stream-json --verbose --replay-user-messages --permission-prompts=host --permission-prompt-tool=stdio`, `--include-partial-messages` when set, `--session-id=` or `--resume=`, then `--model=`, `--effort=`, `--append-system-prompt=`, `--permission-mode=`, `--setting-sources=` (comma-separated), `--max-budget-usd=` (dollars, six decimals), `--mcp-config=<mcp_config_file>` when set (MCP servers never go on the command line, since their environment and headers may be credentials), `--strict-mcp-config`, one `--add-dir=` and one `--plugin-dir=` per directory, and `extra_args`. Every value is one `--flag=value` argument, so a value that starts with `-` is never read as an option.

## Session (`ClaudeSession`)

```rust
pub fn ClaudeSession::start(launch: &ClaudeLaunch, spawn: &dyn Spawn, settings: ClaudeSettings,
    sink: impl Fn(Event) -> bool + Send + Sync + 'static) -> Result<ClaudeSession, Error>
pub fn id(&self) -> Option<String>                       // the one asked for, or the one it chose once said
pub fn version(&self) -> Option<String>
pub fn models(&self) -> Option<Vec<Model>>               // None: it listed none at all, not an empty list
pub fn prompt(&self, content: &[Content]) -> Result<(), Error>
pub fn interrupt(&self) -> Result<(), Error>
pub fn set_model(&self, model: &str) -> Result<(), Error>
pub fn set_effort(&self, effort: Option<&str>) -> Result<(), Error>      // None: Claude Code's own
pub fn permission_mode(&self) -> Option<String>                          // `current_permission_mode` of its initialize answer
pub fn set_permission_mode(&self, mode: &str) -> Result<(), Error>       // `set_permission_mode`; takes effect at once, in a running turn too
pub fn on_permission_mode(&self, hook: impl Fn(&str) + Send + Sync + 'static)   // told each mode Claude Code reports or an "allow always" switches it to
pub fn close(&self)                                                      // also on drop
#[non_exhaustive] pub struct Model { pub id: String, pub name: String, pub description: Option<String>, pub efforts: Vec<String>,
    pub resolved: Option<String>, pub version: Option<String> }
pub fn Model::label(&self) -> String       // "Opus 5.5" for the family name "Opus"; "Default (Opus 5.5)" for `default`; else `name`
pub fn models_of(answer: &Value) -> Option<Vec<Model>>
pub fn version_of(resolved: &str) -> Option<String>      // "Opus 5.5" for claude-opus-5-5: the words and short numbers up to a date; None with no number
pub fn user_blocks(content: &[Content]) -> Result<Vec<Value>, Error>
```

`start` writes `mcp_config` of the launch's MCP servers to a new file in the system's temporary directory, readable only by this user on Unix and on Windows taking that directory's access, names it as `mcp_config_file` unless the launch names one, and removes it on `close` and drop; it then spawns Claude Code, writes an `initialize` control request, which Claude Code answers with no model turn and before any message, and waits for it up to `initialize_timeout`; it then sends `Event::Commands` with the commands of that answer, and an `Event::Notice` when the version is not one of `TESTED_VERSIONS`. Events reach `sink` from a thread of the session's own; one that answers `false` is not called again. The output's end is one `Event::Exited`, saying how Claude Code ended (`exited with code N`, `was ended by signal N`) and the last lines it wrote that were not protocol, standard error's included; none after `close`. A session that reports an id other than its own is killed and ends with `Event::Exited` saying so; nothing of the other session is told.

`prompt` writes one `user` message: text; an image as a base64 source; a resource link as `[@name](uri)`; a text resource as `<context ref="uri">…</context>`, the address's `&"<>` escaped and every `</context` of the text written `&lt;/context`, so it cannot close the block; an image resource as an image. Any other resource is `Error::Refused`. The first prompt's first line, cut to 80 bytes, is the `Event::Title`. `interrupt` writes an `interrupt` control request and does not wait; the turn's error result then ends it as `Stop::Cancelled`. `set_model` sends `set_model`, `set_effort` sends `apply_flag_settings` with `effortLevel`, each waiting up to `control_timeout`. Each line is one write.

`ClaudeSettings` (`new()`, `Default`; set its fields): `max_line` (64 MiB; a longer output line ends the session), `tail_lines` (8) and `tail_line_bytes` (512) quoted by an exit, `initialize_timeout` (30 s), `control_timeout` (30 s), `exit_wait` (1 s for the exit status), `max_tools` (4096 tool calls kept for their results), `max_transcript_bytes` (64 MiB replayed). Also: `parser` (`ParserLimits`: `max_title_bytes` 256, `max_streamed` 64, `max_tasks` 1024), `exit_poll` (100 ms; an exit is noticed even with the output held open), `close_wait` (2 s), `max_queued_writes` (1024; past it a write fails rather than waits), `title_bytes` (80), `version_wait` (10 s; 0 does not ask), `version_bytes` (4096), `on_line: Option<LineHook>` (each line before anything reads it; `false` ends Claude Code).

`models_of` reads `models[]` of an `initialize` answer: `value` as the id, `displayName`, `description` as it stands, `resolvedModel` as `resolved` and its `version`, and `supportedEffortLevels` when `supportsEffort`. The answer's `account` is never read.

## Parser (`coqui_acp_claude::parse`)

```rust
pub type Writer = Arc<dyn Fn(&Value) -> io::Result<()> + Send + Sync>;
pub type ControlAnswer = (String, Result<Value, String>);
pub const ALLOW_ONCE: &str = "allow";  pub const ALLOW_ALWAYS: &str = "allow_always";  pub const REJECT: &str = "reject";
pub fn Parser::new(writer: Writer, max_tools: usize) -> Parser
pub fn Parser::with_limits(writer: Writer, max_tools: usize, limits: ParserLimits) -> Parser
pub fn line(&mut self, value: &Value) -> Vec<Event>
pub fn history(&mut self, value: &Value) -> Vec<Event>     // a transcript line: the person's messages told too
pub fn prompting(&mut self);  pub fn interrupting(&mut self);  pub fn end_of_output(&mut self) -> Option<TurnEnd>
pub fn answers(&mut self) -> Vec<ControlAnswer>            // our control requests' answers since the last call
pub fn version(&self) -> Option<&str>;  pub fn session(&self) -> Option<&str>
```

| Line | Events |
| --- | --- |
| any with `session_id` | `SessionId`, when it changes |
| `system` `init` | its `claude_code_version` is kept |
| `system` `compact_boundary`, then a synthetic message of the person's | `Compaction` with the summary |
| `system` `informational`, `notification` | `Notice` |
| `stream_event` text and thinking deltas | `MessageChunk`, `ThoughtChunk`, then `MessageEnd`, `ThoughtEnd`; message ids `<message id>:<block index>` |
| `assistant` | text and thinking not already streamed (an API error message's text is left to the result); each `tool_use` as `Tool` with title, kind, input, locations, and a diff for `Edit` and `Write`; `Bash` and `PowerShell` also `Terminal` with the command; `TodoWrite` as `Plan` instead; another `*_tool_use` block as `Tool` named `TYPE:NAME` |
| `assistant` and `user` lines with `parent_tool_use_id` | a subagent's own messages, thinking and tool calls as `InChild` (its partial `stream_event`s are dropped, and its plan, usage and title are the parent's alone); `history` does the same and leaves out a sidechain line that names no parent |
| `Agent` (and `Task`) call | `Tool` and `Child` `Starting` (title from `description`, task from `prompt`); its return is `Child` `Completed` with the text as the result, `Failed` when it errored, and nothing when it says `Async agent launched` (a background launch reports its end later) |
| `system` `task_started`, `task_progress`, `task_updated`, `task_notification` of a subagent (`subagent_type` or `task_type` `local_agent`) | `Child` `Working`, then `Working` with `usage` (`total_tokens`, `tool_uses`, `duration_ms`), the `patch.status`, and the end with the summary as result; `task_updated` names only the task id, kept against the call that started it. Measured on Claude Code 2.1.311 (`tests/fixtures/2.1.311/`); a parent's `result.usage` leaves its children's tokens out, `modelUsage` has them in |
| `user` tool results | `Tool` completed or failed: `Edit` and `Write` with a diff whose `patch` is made from `structuredPatch`, located at each hunk; a command's output as `Terminal` with exit 0, or the code its failure names; other output as `ToolChunk` text (not `Read`'s); the Task tools' results update the `Plan` |
| `result` | `Usage` (`total_cost_usd`, `contextWindow` of `modelUsage` as the size, the last request's tokens as context used) with the session's `Spend`: `modelUsage` summed, one `ModelSpend` row per model with its `costUSD`, `thinkingTokens` as `reasoning`, `total_cost_usd` as `cost_micro_usd` rounded to the nearest (unset when absent, never 0); the `usage` beside them is the turn's own and stands in only when `modelUsage` is absent, and then the cost stays unset (a partial pair), `Denied` with `permission_denials` named once each, then `TurnEnd` |
| `rate_limit_event` | `Buckets`, one per `unifiedWindows` entry (else the `rateLimitType`, else `unknown`), its id the window's name: `permille` of `utilization` taken only as a fraction, a fixed window ending at `resetsAt` whose length comes from the name (`five_hour` 5 h, `seven_day` and `seven_day_*` 7 d, any other none), coverage `only {opus}` for `seven_day_opus` and `only {sonnet}` for `seven_day_sonnet` and unknown for every other name, status `allowed`, `allowed_warning`, `rejected` as `ok`, `warning`, `exhausted`; a rejected one kept for the turn it came in |
| `error` | `TurnEnd`: `Limited` when its words or a rejected event say so, else `Failed`; a `result` after it is no second end |
| `control_request` `can_use_tool` | `Permission`; any other subtype is answered with an error at once |
| `control_response` | kept for `answers`, never an event |

`TurnEnd`: a usage limit (`rate_limit_event` `rejected`, or an error result naming `usage limit`, `limit reached`, `hit your limit`, or with `api_error_status` 429; a reset after a trailing `|`, in seconds or milliseconds; the bucket `five_hour` or `seven_day` from the text, source `inferred`; a hit a rejected event backs, in the same turn or alone, is `reported`, and that event's bucket and reset win over the text's; `retry_after` is never set, Claude Code gives none) is `Limited` with a `LimitHit`; an error result after `interrupting`, or with a `terminal_reason` starting `aborted`, is `Stopped(Cancelled)`; `error_max_turns` is `Stopped(MaxTurnRequests)`; another error is `Failed` with its `result` or `errors`; otherwise `Stopped` by `stop_reason` (`max_tokens`, `refusal`, else `EndTurn`).

A permission request offers `allow` (Allow once), `allow_always` when one of Claude Code's `permission_suggestions` grants something (a mode, an allow rule, a directory), named for every suggestion it applies and where each is kept, such as `Allow, and from now on: accept edits (acceptEdits) for this session` or `Allow, and from now on: allow mcp__echo__echo in local settings`, and `reject`. Its responder answers `{"behavior": "allow", "updatedInput": <input>}`, with `updatedPermissions` the suggestions for `allow_always`; `reject` is `{"behavior": "deny", "message": "The person refused this."}`; `Outcome::Cancelled` also sets `"interrupt": true`. The title is Claude Code's, else `<display name>: <tool title>`; the description its `decision_reason` or `description`.

## History

```rust
pub fn config_dir(env: &BTreeMap<OsString, OsString>, clear_env: bool) -> Option<PathBuf>
pub fn project_directory(cwd: &Path) -> String          // every character but an ASCII letter or digit as `-`
pub fn transcript(config: &Path, cwd: &Path, id: &str) -> Result<PathBuf, Error>
pub fn replay(transcript: &Path, max_bytes: u64, max_tools: usize, sink: &dyn Fn(Event) -> bool) -> Result<(), Error>
```

`config_dir` is `CLAUDE_CONFIG_DIR`, else `.claude` in `USERPROFILE` (Windows) or `HOME`, from the launch's environment over this process's. `transcript` is `projects/<project_directory(cwd)>/<id>.jsonl`, only that one and not through a link, for an id `valid_session_id` takes: `--resume` finds a session only from the directory that recorded it. `replay` sends the person's messages (not command output, interruptions or meta messages), the agent's text and thinking, and tool calls with their results; past `max_bytes` the rest is left out and a `Notice` says so.

### Export and import (feature `history`, `coqui_acp_claude::history`)

Cargo feature `history` (off by default) depends on `coqui-acp-history` and adds the module, the provider's `import_session` and `export_session` (without it they answer `GUATIAO_ERR_UNSUPPORTED`) and the `history` subcommand of the program.

```rust
pub const HARNESS: &str = "claude";  pub const CONTENT_DROPPED: &str = "content_dropped";  pub const DEFAULT_MAX_BYTES: u64 = 256 << 20;
pub fn export(text: &str, id: &str) -> Result<Document, Error>                       // a transcript's text; `id` the host's UUID
pub fn import(doc: &Document, cwd: &str, id: &str, options: &ImportOptions) -> Result<Imported, Error>   // the transcript's text
pub fn export_file(config: &Path, cwd: &Path, id: &str, max_bytes: u64) -> Result<Document, Error>        // projects/<dir>/<id>.jsonl
pub fn import_file(config: &Path, cwd: &Path, id: &str, doc: &Document, options: &ImportOptions) -> Result<Imported, Error>
#[non_exhaustive] pub struct ImportOptions { pub use_summary: bool /* true */, pub version: Option<String> }   // Default; .use_summary(bool), .version(..)
#[non_exhaustive] pub struct Imported { pub text: String, pub report: LossReport, pub seeded_from: Option<SeedPoint>, pub closed_calls: u64, pub closed_turn: bool }
```

`history export [--config DIR] CWD ID` (`cli::History`, run by `cli::run_history`) writes the history of a session to standard output, as it is; `history import [--config DIR] [--no-summary] CWD ID` reads one from standard input, writes the transcript and prints what it lost on standard error. `--config` is a configuration directory, by default `CLAUDE_CONFIG_DIR` else `.claude`. A transcript is never replaced.

**Export** reads the conversation: the chain of messages from the last one back through `parentUuid` (a compaction boundary continues through `logicalParentUuid`), or, when a link is missing from the file, every message in file order; `isSidechain` messages and every line that is no message or boundary (file-history snapshots, titles, queue operations, attachments) are not carried, and a line that is not JSON is skipped. A person's message with text and image blocks is a `session/prompt`; one that is `isMeta`, a command echo (`<command-...`, `<local-command-...`) is a `user_message_chunk` with `_meta` `isMeta`; `[Request interrupted...` is a `session/cancel`. Assistant text is `agent_message_chunk`, thinking `agent_thought_chunk` (its signature, or a redacted block's data, in `_meta`), each `tool_use` a `tool_call` (ACP version 2's `name` the Claude Code tool, `kind`, `title`, `locations` and `rawInput` exactly as Claude Code's, a diff for Edit and Write) and each `tool_result` a `tool_call_update` (`rawOutput` the content as the model was given it, `failed` for `is_error`); TodoWrite also tells its plan. A turn ends where the next prompt begins or the file ends: `end_turn`, `max_tokens` or `refusal` by the last `stop_reason`, `cancelled` after an interruption or with a call unanswered, `error` for an API error message; a file that ends with the agent owing an answer leaves the turn open. A compact boundary and the summary message after it are one `history/compaction` with the summary. The header has the origin version and session id, `cwd`, `createdAt`, the last model, effort and permission mode, and a digest. Server-side tool blocks and the thinking of a subagent are not carried. `_meta` (`dev.boriken.history/native`, `harness` `claude`): `messageId`, `model`, `signature`, `redacted`, `isMeta`.

**Import** writes one transcript line per block, chained by `parentUuid`, with ids made from the host's UUID (the same history gives the same transcript), the host's directory and the version (the newest tested one unless told). It seeds with `Conversation::of`: with `use_summary` the latest compaction that has a summary becomes a `compact_boundary` and the summary message in its own slot, and nothing before it is written; a boundary without a summary is `summary_missing`. A tool call is written as Claude Code's own when the origin is Claude Code, and otherwise through the normalised tool table: Read, Write, Edit, Grep, Glob and WebFetch for their table rows, Bash for `shell`/`shell_command` (a command vector joined into a line), and a Codex `apply_patch` as Write when it adds one file and as Edit when it replaces one run of lines in one file; a call the table does not name (Codex's `exec`, a patch of several files, an MCP tool) is written as a text note of the call and its result and counted `tool_as_note`. A call id Claude Code would not take is made one that it does. Thinking is written only with the signature it had, and otherwise dropped and counted `thinking`; a block the history left out by digest, or of a kind Claude Code does not take, is counted `attachment_by_digest` or `content_dropped`. The rules for hostile histories: a call with no result gets one that says it did not finish, and a turn left open ends with the interruption line (rule a); a permission request or answer is not written and no rule or `updatedPermissions` is (b); no snapshot is written (c); nothing a history names is opened, and the file is `projects/<directory of the host's cwd>/<host's UUID>.jsonl` whatever the history says (d); the summary goes only to the summary slot (e).

The provider imports and exports when its spawner runs on this machine (`SpawnerInfo::local`; else `UNSUPPORTED`): the directory is the grant's `state_dir` for a login kind, else the configuration directory of the request's environment. An id that is not a UUID or already names a session is `BAD_VALUE`, a session with no transcript is `NOT_FOUND`, and an unknown history version is `UNSUPPORTED`. An empty history seeds nothing on disk: the provider keeps the id (`ClaudeAgent::fresh_sessions`, shared with the agents it starts), and the first open of it by `session/resume` or `session/load`, while it has no transcript, starts a new Claude Code session under that id (`--session-id`); export of it before that is a history with nothing in it. Export pages are `coqui_acp_history::paging`'s.

## ACP agent (`ClaudeAgent`)

```rust
pub fn ClaudeAgent::new() -> ClaudeAgent                     // also Default; claude from PATH, PlainSpawn
pub fn defaults(self, defaults: ClaudeLaunch) -> Self
pub fn spawn(self, spawn: Arc<dyn Spawn>) -> Self
pub fn settings(self, settings: ClaudeSettings) -> Self
pub const fn client_plugins(self, allowed: bool) -> Self   // default false; each path held to the working roots and network rule
pub fn cli_version(&self) -> Option<String>                 // claude --version, once, within ClaudeSettings::version_wait
impl Harness for ClaudeAgent
pub const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
pub const DEFAULT: &str = "default";
```

`agentInfo` is `coqui-acp-claude`, titled `Claude Code <version>`, at this crate's version. It takes images, embedded context, MCP servers over stdio, HTTP and SSE (version 1), load, more directories and an appended system prompt. A session's parts are added to `defaults`: its `cwd`, a new UUID or the id resumed or loaded, `systemPrompt` appended after the defaults' own, `mcpServers`, `additionalDirectories` as `--add-dir`, and `_meta.claudeCode.options.plugins` (`[{"type": "local", "path": absolute}]`, as the Claude Agent SDK's `plugins` option) as `--plugin-dir`, whose skills are `<plugin>:<skill>`. A plugin of another type, a relative path, or any plugin when `client_plugins(false)`, is refused, as is an MCP server named as one of the defaults'. Load replays the transcript, then resumes.

The mode option (`mode`, category `mode`; ids from `coqui_acp_harness::mode`), measured on Claude Code 2.1.318 with `initialize` and `set_permission_mode`, no prompt sent:

| Mode | `permissionMode` | Offered |
| --- | --- | --- |
| `manual` | `default` (`manual` on the command line is `default`) | the launch's mode and every stricter one |
| `accept_edits` | `acceptEdits` | likewise; Claude Code takes the switch |
| `auto` | `auto` | likewise; whether the account may use it shows at the first turn |
| `plan` | `plan` | always |
| `bypass` | `bypassPermissions` | only with `allow_bypass`, whatever the launch's mode: Claude Code refuses the switch otherwise (`bypass_not_launched`), and a launch in it with `allow_bypass` off shows Bypass and cannot be taken back to it |

What a client may choose is capped by the launch: the launch's own `permission_mode` (default `default`, so Manual) and every stricter one, in the order Plan, Manual, Accept edits, Auto, Bypass; `ClaudeLaunch::modes` names more (a host's widening, still with no Bypass unless `allow_bypass`). A launch in a mode no mode names (`dontAsk`) offers Plan only, and shows it as `custom`. The current mode at the start is `current_permission_mode` of the `initialize` answer. A switch is a `set_permission_mode` control request and holds for the turn running and every turn after it. The mode shown is the one Claude Code is really in: the session tells it when Claude Code reports a `permissionMode` (`system` lines, `init` included) and when a person's "allow always" forwards a `setMode` suggestion (the last one wins; `setMode` is never stripped, since the person chose it), and the conversation sends a `config_option_update` for each change. `ClaudeSession::on_permission_mode` is that hook. A `setMode` an "allow always" would apply is left out of what the answer applies when it names a mode outside the offered list (`ClaudeSession::permit_modes`), so a person's "always" cannot loosen what a host capped; an option that would then grant nothing is not offered. The hook is told after the parser's lock is released, never under it. Claude Code's own list of modes is the `--permission-mode` choices (`acceptEdits`, `auto`, `bypassPermissions`, `manual`, `dontAsk`, `plan`) and the error text of an unknown mode (`acceptEdits, auto, bypassPermissions, default, dontAsk, plan`); its `initialize` answer lists none.

Leaving Plan, measured on Claude Code 2.1.311 (one real turn, `claude-haiku-5-5`, started in `plan`): allowing the `ExitPlanMode` permission once is answered with `behavior: allow` and no `setMode`, and the CLI writes, right after the answer, one `{"type":"system","subtype":"status","status":null,"permissionMode":"default","uuid":...,"session_id":...}` line, the mode the session was in before Plan. So the mode shown follows that line like any other, and the translator adds nothing of its own; the `COQUI_ACP_CLAUDE_MODE_LINE` stand-in writes that shape.

Config options: `model` (category `model`): the models Claude Code lists, and the current one; `effort` (category `thought_level`): `default` and the current model's efforts, else `EFFORTS`. Setting one not offered is refused; setting one sends `set_model` or `apply_flag_settings` for the turns that follow.

## Provider (`ClaudeProvider`, id `claude`)

```rust
#[derive(Provider)] pub struct ClaudeProvider;                       // an `acp_harness` provider; Debug
pub fn ClaudeProvider::new(settings: ProviderSettings) -> ClaudeProvider  // also From<ProviderSettings>
pub struct ProviderSettings {                                        // ToValue, FromValue, Schema, Default; every field defaults
    pub executable: String,              // "claude": a path, or a name looked up where the spawner runs it
    pub model: Option<String>, pub effort: Option<String>, pub permission_mode: Option<String>,
    pub access: Option<Access>,          // None
    pub allow_client_access: bool,       // false: a session's `_meta` access is ignored unless true (then it replaces `access`)
    pub allow_bypass: bool,              // false
    pub modes: Option<Vec<Mode>>,        // None; ids `manual`, `accept_edits`, `auto`, `plan`, `bypass`: widen the permission mode's own and every stricter one (Bypass still needs allow_bypass, and is left out without it)
    pub setting_sources: Option<Vec<SettingSource>>,   // None: Claude Code's default; `user`, `project`, `local`; an empty list loads none (`--setting-sources=`)
    pub max_acp_version: u64,            // 2 (1 or 2)
    pub max_sessions: u64,               // 8
    pub max_requests: u64,               // 32: requests worked on at once; a request past it is refused
    pub max_message_bytes: u64,          // 64 MiB: one byte more ends the connection GUATIAO_ERR_BAD_VALUE
    pub allow_client_plugins: bool,      // false
    pub client_mcp_servers: bool,        // true
}
#[non_exhaustive] pub enum SettingSource { User, Project, Local }   // ToValue, FromValue, Schema; written `user`, `project`, `local`; `as_str()` is the name `--setting-sources` takes
pub mod accounts { pub fn catalog() -> AccountCatalog;  pub fn offer() -> AccountOffer;  pub fn apply(grant: &AccountGrant, launch: &mut ClaudeLaunch, files: &Files) -> Result<Applied, ProviderError>;
    pub struct Applied;  /* .dir() -> Option<&str>, .keeps_files() -> bool, .hold(self, &Arc<Accounts>, &Files) -> Result<Option<Hold>, ProviderError> */  pub const LOGIN, CONSOLE, OAUTH_TOKEN, API_KEY, AUTH_TOKEN: &str;
    pub const SUBSCRIPTION, CONSOLE_METHOD, API_KEY_METHOD, AUTH_TOKEN_METHOD, LONG_LIVED_TOKEN: &str;  pub const CREDENTIALS_FILE, FILE_KEY: &str; }
```

Register it with `Registry::register_local`, or build it into a library with `guatiao::providers!` (`examples/claude_provider.rs` is that library, built by `cargo test`). Each connection serves a `ClaudeAgent` over the channel with `coqui_acp_harness::provider::Serving`; it starts `claude` through the host's spawner. `examples/fake_claude.rs` is a stand-in for Claude Code in its stream-json dialect that behaves as the contract agent does (it needs `CLAUDE_CONFIG_DIR` or the suite's home, keeps sessions as Claude Code does, answers `auth login` with the recorded prompts and `auth status` as the kinds need, rotates a login in its last five minutes, and logs its arguments, environment names (never values) and the two cache variables to `COQUI_ACP_FAKE_CLAUDE_LOG` and to `fake-claude.log` of the configuration directory; `fake-refresh` in that directory holding `refuse-blank` or `refuse-delete` scripts a refused refresh, `fake-login` holding `refuse` a refused code); the suite runs against it as `check harness --library <claude_provider> --provider claude --settings '{"executable": "<fake_claude>"}' --program-agent --no-imports --holds no_project_mcp`; `--program-agent` runs the checker in a home of its own, which the stand-in uses when `CLAUDE_CONFIG_DIR` is not set.

- `describe`: title `Claude Code`, ACP versions 1 and 2 (to `max_acp_version`), what `ClaudeAgent` takes and `session/resume`, `resumes_wider`, usage `reported` and `inferred` with spend and `needs_turn`, and the cache hint: `cache_lifetime` short sets `CLAUDE_CODE_PROMPT_CACHE_TTL` and `CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL` to `5m`, long to `1h`, off sets `DISABLE_PROMPT_CACHING` to `1` (a gate Claude Code 2.1.311 reads before any request), default leaves all as the environment has them. `cache_choices` lists Off, "5 minutes" and "1 hour".
- `refusal`, `start`: holds `no_project_mcp` as `--strict-mcp-config` (which starts only the servers `--mcp-config` names, so none of the user's own either; whether it also keeps out the MCP servers of plugins that a project's `.claude/settings.json` enables is unmeasured); every other requirement is `GUATIAO_ERR_UNSUPPORTED` with its name (the flags do not leave instruction files loaded while they drop a directory's settings, and what the rest take away is not measured). The environment is the host's, over nothing; the session ids are Claude Code's UUIDs.
- `accounts`: `harness-login` and the kinds measured on Claude Code 2.1.311, with every credential variable, and no login method: the harness signs no one in and every kind says `renews` false (the account provider below does both). A login kind (`claude-login`, `claude-console`) keeps its login in `.credentials.json` of a configuration directory (credential `credentials_file`, rotating); `claude-oauth-token`, `anthropic-api-key` and `anthropic-auth-token` are one variable each (`token`, `api_key`, `auth_token`), with the optional setting `base_url` (`ANTHROPIC_BASE_URL`). The login methods are the account provider's (`accounts::offer()`).
- Standards: `claude-login`, `claude-console`, `claude-oauth-token` and `harness-login` provide `claude-code`; `anthropic-api-key` and `anthropic-auth-token` provide `anthropic-api`. It accepts, in this order, `claude-code` (`credentials_file` as `claude-login`: `.credentials.json` in the grant's `state_dir`, `CLAUDE_CONFIG_DIR`; `token` as `claude-oauth-token`: `CLAUDE_CODE_OAUTH_TOKEN`) and `anthropic-api` (`api_key` as `anthropic-api-key`: `ANTHROPIC_API_KEY`; `auth_token` as `anthropic-auth-token`: `ANTHROPIC_AUTH_TOKEN`), each with `base_url` as `ANTHROPIC_BASE_URL`. An account made elsewhere that provides one of them is applied as that kind; any other is `BAD_VALUE` naming `claude-code, anthropic-api`.
- Applying a grant (`start`, `models`; `usage` and the session calls only check it): every credential variable is removed from what Claude Code starts with, and from the host's environment the request carried, then the kind's is set (`CLAUDE_CONFIG_DIR` the grant's `state_dir` for a login kind). A login kind's file is written into that directory through the spawner under the rules in `coqui_acp_harness::account`; **nothing watches it and the keeper handed to a call is destroyed unused**: a login Claude Code renews while it runs is told by the account provider's `watch`, which a host holds for as long as a connection runs. `BAD_VALUE`: an id that is not one, an unlisted kind, a setting the kind lacks or a `base_url` that is not an `http(s)` address, a credential missing or not the kind's, a login kind with no `state_dir`; `UNSUPPORTED`: a login kind on a spawner with no files. **What is removed** is `accounts::VARIABLES`, also the catalog's `credential_variables` (its `secret_variables` are the keys, tokens, passwords and private keys among them, not the endpoints, switches, profiles, projects or file paths): the credentials, the provider switches (`CLAUDE_CODE_USE_BEDROCK`, `_VERTEX`, `_FOUNDRY`, `_ANTHROPIC_AWS`, `_MANTLE`), the six `*_BASE_URL` endpoints and the cloud credentials (`AWS_*`, `GOOGLE_APPLICATION_CREDENTIALS`, the Vertex and Foundry settings), from the request's environment and from the inherited one (names compared without case on Windows); the endpoint is set only from the grant's `base_url`. Measured: the credentials, `CLAUDE_CODE_USE_BEDROCK` and the six `*_BASE_URL` of the documentation; not run on 2.1.311: the other switches and the cloud credentials. An environment kind (no directory of its own) also starts with `--setting-sources=project,local`, so the person's `~/.claude` settings (`apiKeyHelper`, an `env` block) do not outrank the credential, unless the entry's `setting_sources` says otherwise (its notices say so).
- **`accounts::apply(grant, launch, files)`** is what `start` and `models` apply a grant with. It removes every credential variable from `launch.env` and sets `launch.remove_env` to them, sets the kind's variable (and `ANTHROPIC_BASE_URL` from the grant's `base_url`), and for an environment kind sets `--setting-sources=project,local` unless `launch.setting_sources` is already set. It writes no file: a login the grant carries is placed and watched by `Applied::hold(&Arc<Accounts>, &Files)`, which is `None` for a kind that keeps none (`start` calls both). A login kind whose grant carries **no** `credentials_file` and has a `state_dir` is applied **in place**: Claude Code runs on that directory as it is (`CLAUDE_CONFIG_DIR`, the person's settings kept), nothing is written, `hold` is `None`; `start` still refuses such a grant. `files` is looked at only for a login to place (`UNSUPPORTED` when its spawner serves none). The endpoint variables are cleared with the rest: a host that keeps a person's own endpoint puts it back (they are in `credential_variables` and not in `secret_variables`). `Applied`'s `Debug` shows no credential.
- `models`: starts `claude` through the spawner, reads its `initialize` answer and ends it (no turn); unknown, with the reason, when it could not be asked.
- `usage`: what the connections last saw for the account (`Reports`), as Claude Code learns usage only in a turn. An account's plan limits are the account provider's `usage`.
- The harness has no `login` or `renew` and takes no keeper: the account provider below signs in, renews and watches.
- `import_session`, `export_session`: with the `history` feature the transcript's, as above; without it `GUATIAO_ERR_UNSUPPORTED` with a reason (after checking the account). `session_record`, `non_defaults`: the kind's defaults.

## Account provider (`ClaudeAccounts`, id `claude_accounts`)

```rust
#[derive(Provider)] pub struct ClaudeAccounts;                        // an `account_provider` provider; Debug
pub fn ClaudeAccounts::new(settings: AccountSettings) -> ClaudeAccounts  // also From<AccountSettings>
pub struct AccountSettings {                                          // ToValue, FromValue, Schema, Default; every field defaults
    pub executable: String,              // "claude": what signs in and renews
    pub renew_poll_ms: u64,              // 30000: how often a watch looks at the login's file; 0 looks only when it begins and ends
    pub command_timeout_ms: u64,         // 60000: the longest `claude auth status` may take
    pub usage_endpoint: Option<String>,  // None: https://api.anthropic.com (the OAuth API; also the model API for a key with no base_url)
    pub usage_timeout_ms: u64,           // 10000: the longest a plan-limits read may take in all
}
```

The same library file (`examples/claude_provider.rs`) offers it beside `ClaudeProvider`; `coqui_acp_api::contract::AccountContract` holds it to the kind (`tests/account_provider.rs`, with `.program(true)` so the suite starts `fake_claude` for real). It runs no agent and keeps no state but what it learned of each account's files and the last usage it told.

- `describe`, `offers`: title `Claude accounts`; `accounts::offer()`: the five kinds (not `harness-login`, which nothing signs in to) with `renews` true for the two login kinds, the four sign-in methods (`subscription`, `console` as `claude auth login --claudeai|--console` with steps `open_url` and `paste_code`, remote, input `email`; `api-key`, `auth-token`: `ask`, checked with `claude auth status`; `long-lived-token` (`claude setup-token`) answers `UNSUPPORTED`: it needs a terminal and its prompts were not recorded), and `usage` for `claude-login`, `claude-oauth-token`, `anthropic-api-key` and `anthropic-auth-token`. Every call but `describe` and `offers` refuses `BAD_VALUE` an id that is not one and a kind it does not list, **including a kind of another provider that provides the same standard**.
- `login`: runs `claude auth login ...` in the login's directory (every credential variable removed), relays the page it prints with `open_url`, asks for the code the page shows (`ask`, a secret) and writes it to the command's standard input; a code the command refuses is `BAD_VALUE` `login refused: ...`. A typed key or token is set in the environment of `claude auth status` and never on a command line. Cancelling or `timeout_ms` ends the command's tree and answers `GONE` within a moment, whatever the call is waiting on.
- `renew`: for a login kind, `claude auth status` in the account's directory, which refreshes a login within five minutes of `expiresAt` (a login further from its end is answered `not_renewed_because` with no command run); a login in the directory that differs from the grant's is answered as the renewal, whatever the window. The directory is **looked at, never filled from the grant**: a missing, blank or refused login is `GONE`, so a host that wants the grant's copy there lets a harness start on the account first (or writes `credentials_file` itself). Other kinds: `UNSUPPORTED` with a reason.
- `watch`: for `claude-login` and `claude-console`, looks at `.credentials.json` every `renew_poll_ms` through the spawner and tells the keeper `renewed` (whole credentials, `from_revision` the grant's, `expires_at` from `expiresAt`) or `lost` (a file missing, blank or with both tokens blank), once each, with a sequence every live watch of the account in the instance hears. It reports what it finds when it begins and never writes a file. Other kinds: `UNSUPPORTED`.
- `usage`: `account_usage::read_limits`, as `AccountUsage` (a login's windows, a key's rate limits; a Console login and any login without the profile scope `not offered`); `revision` grows when what is read changes. The query's `environment` is not used: no program is started.

## Plan limits of an account (`coqui_acp_claude::account_usage`)

```rust
pub fn read_limits(grant: &AccountGrant, options: &ReadOptions) -> Result<UsageReport, ProviderError>
pub struct ReadOptions { pub endpoint: Option<String>, pub timeout: Duration /* 10 s */, pub now: Option<i64> }   // #[non_exhaustive]; ReadOptions::new().endpoint(..).timeout(..).now(..); `endpoint` also moves the read of an API key or gateway token whose account names no `base_url`
pub const DEFAULT_ENDPOINT: &str = "https://api.anthropic.com"; pub const USAGE_PATH: &str = "/api/oauth/usage";
```

The account's side, with no harness, session or program: `GET <endpoint>/api/oauth/usage` with the login's access token as a bearer token and `anthropic-beta: oauth-2025-04-20`, the request Claude Code's own `/usage` view makes (read from its code; the OAuth API address is fixed there and the account's `base_url`, the model endpoint, never moves it). The report is `supported` with a reported bucket per window the document names (`five_hour`, `seven_day`, `seven_day_opus`, `seven_day_sonnet`, `seven_day_oauth_apps`, each `utilization` percent as permille of 1000 and `resets_at` as a fixed window) and per `model_scoped` entry (`seven_day_model_<name>`, covering that model). `claude-login` reads (its `scopes` must hold `user:profile`, else not offered, and its access token must not have expired: `GONE`, never refreshed here since a refresh rotates the file's refresh token); `claude-oauth-token` is tried and a 403 is not offered; `claude-console`, `anthropic-api-key`, `anthropic-auth-token`, `harness-login` and a document with no window answer `UsageReport::unsupported` with the reason. 401 is `GONE`, anything else that fails `INTERNAL`; no message holds the token or the answer. Per-session token counts are not here: they stay with ACP's usage updates. `ClaudeProvider::usage` is unchanged.

## Errors

`Error` (`#[non_exhaustive]`): `Spawn(io::Error)`, `Refused(String)`, `Timeout { what, limit }`, `Harness(String)` in Claude Code's words or saying how it ended. Into `HarnessError`: `Refused` stays refused, the rest is `Failed`.

## Arguments

`cli::Args` (a `clap::Args` type) and `cli::run(&Args) -> Result<u8, String>` serve `ClaudeAgent` on standard input and output. The crate runs no program of its own: `examples/serve_claude.rs` is a command line over them, and a host runs `run`. Flags: `--executable` (`COQUI_ACP_CLAUDE_EXECUTABLE`, default `claude`), `--model`, `--effort`, `--permission-mode`, `--allow-bypass`, `--setting-sources` (comma-separated, empty for none), `--strict-mcp-config`, `--max-acp-version` (1 or 2, default 2), `--max-sessions` (8), `--no-permission-prompts`, `--allow-client-plugins`, `--working-root` (repeated; a session's `cwd` and more directories must lie in one), `--no-client-mcp-servers`, `--allow-network-paths`. Notices and exits go to standard error. Exit status 0 when the client closes, 1 when reading it failed, 2 for a wrong invocation.

## Environment variables

`COQUI_ACP_CLAUDE_EXECUTABLE`, as `--executable`. Claude Code reads its own (`CLAUDE_CONFIG_DIR`, `ANTHROPIC_API_KEY`, `CLAUDE_CODE_OAUTH_TOKEN` and others) from the environment the agent runs in, or `ClaudeLaunch::env`.

## Gotchas

- `--permission-prompts host` alone does not route prompts to standard output on Claude Code 2.1.318: the call is refused as not granted. `--permission-prompt-tool=stdio` does, and the launch passes both unless `permission_prompts` is off.
- `dontAsk` decides before those flags, on Windows and on Linux: a call nothing granted is refused where it is made, no question comes, and one allowed runs (recording `dont_ask`). With no prompt flags and no mode, the same refusal (recording `no_prompts`).
- Claude Code learns usage only during a turn: `describe` says `usage.needs_turn` and a read with no turn answers what the connection last saw. It writes `rate_limit_event` only for a subscription login. Its `result` `usage` is the turn's own while `modelUsage` and `total_cost_usd` are running totals (measured: 11 input in `usage` beside 88 in `modelUsage` after two turns), so `Spend` takes the latter; `Usage::cumulative` still marks the former.
- A `rate_limit_event` is the only place a limit's reset is a number; the `result` after it says a local clock time. A rejected one becomes the limit of a turn that then fails or whose output ends; one whose turn is answered anyway is no failure (recording `rate_limit`). Claude Code writes these only with a subscription's login.
- No recording holds a `type: error` line: nothing a stand-in did drew one from 2.1.318. It is handled as a failed turn, its words read from `result`, `error.message`, the first block of `message`, `message` or `error`.
- Claude Code's output does not outlive it: on Windows and on Linux, with a background `Bash` still running, killing Claude Code alone ended its output at once, and closing standard input made it wait for that `Bash`, then exit and end its output together. What can hold the output open past an exit is a `Spawn` or a wrapper; the watcher is for that.
- `close` ends what Claude Code started on Windows, its job object taking a background `Bash` with it. On Linux that `Bash` runs in a session of its own, outside Claude Code's process group, and outlives `close` until it ends by itself.
- A prompt's blocks keep the order the client gave them: a host that wants images first, as the model's guidance has it, puts them first.
- `partial_messages` is on since ACP streams; a host that wants whole messages turns it off.
- `--resume` finds a session only from the directory that recorded it.
- With stream-json input Claude Code writes nothing, not even `init`, until it reads a line; `initialize` is answered at once.
- Claude Code 2.1.318 in print mode offers neither `TodoWrite` nor the Task tools (`No such tool available`, `CLAUDE_CODE_ENABLE_TASKS` either way); a plan the model writes with them is still told.
- A permission answered `reject` lets the turn go on; only `Outcome::Cancelled` interrupts it.
- Everything Claude Code writes is the model's or the harness's: untrusted.
- A plugin's hooks and MCP servers run with no permission asked, whatever `--strict-mcp-config` and `--setting-sources` say: they are off until a host says `client_plugins(true)`, or the arguments say `--allow-client-plugins`.
- A diagnostic line on standard error longer than `max_line` is noted and the rest of standard error read and dropped, so Claude Code never writes to a closed pipe.
- An appended system prompt goes on the command line: past the platform's limit (128 KiB for one argument on Linux, 32 767 characters for a command line on Windows) Claude Code does not start.
