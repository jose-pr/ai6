# `ai6-agent-codex` — API header

Header-file-style reference for the `ai6-agent-codex` crate: every public item with its
signature, contract and gotchas, so a consuming agent skips the source. It ships inside
the package and is self-contained. Development documentation lives with the source at
<https://github.com/jose-pr/ai6>.

No features. MSRV: Rust 1.89. The crate depends on `ai6` (feature `gate`) and `serde_json`. It
re-exports `CodexHandler` and `CodexConfig` from the root, beside the tool check's
`TOOL_CHECK_ENV` and `KEPT_FEATURES`. The crate contains no `unsafe` code.

## `CodexHandler`

`CodexHandler` and `CodexConfig` implement `ai6::directorates::section_00::agent::AgentHandler`. The config
selects the Codex executable. It starts Codex in app-server mode using the AI6-supplied
terminal. The AI6 Session ID is Codex's resumable thread ID; the handler passes it to
`thread/resume`. The separate Codex session-tree ID is not used as the resume handle.
The handler sends the Mission working directory and verifies the returned thread ID
before accepting the resumed Session. A missing or different thread ID closes the
app-server process and fails resume.
A usage limit is emitted as an event of kind `limit` (`ai6::LIMIT_EVENT`) with
`AgentEvent::limit` set, instead of `completed` or `failed`. It is recognised from an
`error` notification, a `turn/failed` notification or a failed `turn/completed` whose error
carries a `codexErrorInfo` naming a usage or rate limit. The reset time is the error's
`resetsAt` (Unix seconds) when present, otherwise the reset of the quota window the account
last reported as used up (`account/rateLimits/updated` with `usedPercent` of 100 or more;
a 300-minute window is `five_hour`, a 10,080-minute one `weekly`). These shapes come from
recorded protocol fixtures, not from a live account.
Every `account/rateLimits/updated` notification also sets `AgentEvent::quota` to one
`QuotaReport` per window present (`primary`, `secondary`): `window` named as above, else
by its position; `used_permille` from `usedPercent` (times ten, clamped to 0-1000);
`resets_at` from `resetsAt`; no `status`.
A `thread/tokenUsage/updated` notification carries `AgentEvent::usage` from
`params.tokenUsage.last` (the latest model request; the thread total survives a resume, so it
is not used): `inputTokens` minus `cachedInputTokens` as input, the cached ones as cache reads,
`outputTokens` as output, `cumulative` false. Codex reports no cost, so `cost_micro_usd` is
0 and Codex Sessions are limited by tokens only. An `item/started` notification for a web
search (`WebSearch`) or an MCP or dynamic tool call (the tool's name) sets
`AgentEvent::tools`. These shapes follow the app-server protocol schema, not a live account.
Terminal end-of-file is reported as a provider error so AI6 can fail and finalize the
Mission instead of polling an exited process indefinitely.
When AI6 supplies `AI6_CABLE_MCP_URL` and `AI6_CABLE_MCP_TOKEN`, app-server receives
per-process config overrides for the Cable endpoint and token environment-variable name.
The bearer token itself is not placed in process arguments.
When AI6 supplies `AI6_THEATER_MCP_URL` and `AI6_THEATER_MCP_TOKEN` the Session is in
remote-hands mode: its work is on a remote Theater, reached only through the Theater tools.
App-server then also receives overrides for an `ai6-theater` server, and `thread/start` and
`thread/resume` carry `sandbox = "read-only"` and `approvalPolicy = "never"`, because Codex's
shell tool cannot be switched off: the sandbox keeps anything from being written on the
Station. The two parameter names follow the app-server protocol and are not exercised against
a live Codex.
The handler answers every `item/commandExecution/requestApproval` server request (params
`threadId`, `turnId`, `command`, `cwd`, `kind`) with the green ink gate
(`ai6::directorates::section_00::gate::gate`, asking the Station through the Cable endpoint):
`{"decision": "accept"}` unless the gate holds the command, then `{"decision": "decline"}`
followed by `turn/steer {threadId, expectedTurnId: turnId, input: [{type: "text", text:
REASON}]}` so the Agent learns what to ask for. A `writeStdin` approval and one without a
command are declined the same way: the gate cannot see what they run. A Session without a
Cable endpoint cannot ask for green ink, so it has no gate and everything is accepted, as with
the Claude handler. With a gate, `thread/start` and `thread/resume` carry
`approvalPolicy = "untrusted"`, so Codex asks before every command outside its own list of
safe reads; in remote-hands mode it stays `never`, and the Station gates `theater_run`. These shapes follow the app-server protocol schema, not a live Codex.
When `AgentStart::project_settings` (or `AgentResume::project_settings`) is `false`, the
default, app-server also receives `-c projects={"<working directory>"={trust_level="untrusted"}}`:
an untrusted project skips its `.codex/` layer (project config, hooks and rules), while the
user and system layers still load (Codex configuration reference). The table is one inline
value because a path holds dots; it replaces the user's `projects` table for that process
only. `true` leaves trust to the user's own configuration. The key is the working
directory as AI6 passes it as `cwd`; should Codex key the project by another spelling of the
path (its repository root, a canonical form), that project has no entry at all in the
replaced table: it gets Codex's default for an unlisted project, never a trust the user
granted it. That the default is untrusted for an app-server session is not confirmed from
the Codex documentation. Not exercised against a live Codex.
When `AgentStart::answer_only` (or `AgentResume::answer_only`) is `true`, which AI6 sets for
a Chief of Staff review, app-server receives `-c mcp_servers={}`, `-c` for
`features.shell_tool=false`, `tools.view_image=false`, `web_search="disabled"`,
`features.multi_agent=false`, `agents.enabled=false`, `features.apps=false` and
`skills.max_context_tokens=1`, and the untrusted-project override and none of AI6's servers;
the Session has no gate, and `thread/start` and `thread/resume` carry `sandbox =
"read-only"`, `approvalPolicy = "never"` and `config = {"web_search": "disabled"}`. An
assistant (`AgentStart::assistant`) gets the same, with `mcp_servers` holding exactly its
servers.

Both are restricted threads, which start or resume only after the tool check:

```rust
pub const TOOL_CHECK_ENV: &str = "AI6_CODEX_TOOL_CHECK"; // strict (default, also unset) | warn; other values refuse
pub const KEPT_FEATURES: [&str; 4] = ["secret_auth_storage", "enable_request_compression",
                                      "system_proxy_fallback", "unbounded_connection_retries"];
```

Through the terminal AI6 supplies, with the app server's `-c` settings, environment and
working directory, the handler runs `codex --version`, then `codex features list` and adds
`-c features.<name>=false` for every enabled feature not in `KEPT_FEATURES`, then `codex
debug prompt-input` with all of those and `-c model="<model>"` when the request names one
(what Codex offers depends on the model). It reads the offered tools from the
`additional_tools` items (`namespace.tool` inside a namespace); an item of any other type
than `message` and `additional_tools`, or output that is not that JSON, is unreadable. The
thread passes when every offered tool is `mcp__<server>.<tool>` for one of its servers and
each server offers at least one (answer-only: no tool). On a failure `strict` refuses with
`GUATIAO_ERR_UNSUPPORTED` naming what was found, before any app server starts; `warn`
starts it, prints `ai6: weakening: …` on standard error and queues an event of kind
`weakened` with that text first. The feature switches apply in both. A pass is cached in
the process for 15 minutes by executable, `--version` output, settings (without the
working directory's `projects` entry), model, servers and mode; a refusal is not cached.
With Codex 0.164.0 an answer-only thread passes with no tool; an assistant does not, because
that release offers its models MCP tools only inside code mode's `functions.exec` (fixtures
in `tests/fixtures/codex-0.164.0/`). `codex debug prompt-input` is trusted to render what a
thread is sent; with every feature off `codex exec` (0.164.0) was seen to send
`functions.exec`, `functions.wait` and `functions.request_user_input` where it renders
none, and the app server's case is not established.

The terminal is opened at `STREAM_COLS` (32767) columns and read with
`ai6::directorates::q::terminal::JsonLines`: ConPTY's controls, its echo of the requests
written and lines it breaks are read as the stream Codex wrote. A line that is not JSON is
skipped while waiting for a reply, and is an event of kind `output` otherwise. Checked against the app-server protocol source
(`ThreadStartParams`: `sandbox`, `approvalPolicy`, `config`; the `web_search` key and its
`disabled` value): those for `thread/start`. Inferred: that `thread/resume` takes `config`
as well, that the read-only sandbox has no network, and that the `mcp_servers` override
replaces the user's table rather than merging into it.

## Gotchas

The Codex executable must be available to the terminal environment. Tests use protocol
fixtures and a process-backed fake app-server; no real Codex account is required. The crate
builds both an `rlib` and its own Guatiao-loadable `cdylib`.
