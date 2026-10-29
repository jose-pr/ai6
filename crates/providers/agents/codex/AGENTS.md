# `ai6-agent-codex` — API header

Header-file-style reference for the `ai6-agent-codex` crate: every public item with its
signature, contract and gotchas, so a consuming agent skips the source. It ships inside
the package and is self-contained. Development documentation lives with the source at
<https://github.com/jose-pr/ai6>.

No features. MSRV: Rust 1.89. The crate depends on `ai6` (feature `gate`), `serde_json`,
`tempfile`, and `libc` (Unix) or `windows-sys` (Windows). It re-exports `CodexHandler` and
`CodexConfig` from the root, beside the tool check's `TOOL_CHECK_ENV` and `KEPT_FEATURES`. Its
only `unsafe` code kills the sandbox probe's process tree: the terminal providers'
`crates/providers/terminals/process_tree/{unix,windows}.rs`, included with `#[path]`, and, on
Windows, putting the probe in its Job Object before it runs.

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
The end of the app server's output is the terminal's end-of-file, or its exit while the
terminal keeps the output open (a read that times out asks the terminal's `exit_status`).
After every line written before it, `next_event` then returns one `failed` event whose text
names the exit (`exited with code N`, `was ended by signal N`, or `closed its terminal` when
the terminal cannot tell) and quotes the last eight lines it wrote that were not JSON (512
bytes each), and `None` after that; a pending request (`initialize`, `thread/start`,
`thread/resume`) fails with the same text. `close` kills the terminal and waits up to 2 s for
its exit status, so the working directory is free when it returns.
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
command are declined the same way: the gate cannot see what they run. With a gate, two more
are declined without asking it, and also printed on standard error and queued as an event of
kind `declined`: a request whose line was `rebuilt` (`StreamLine::Json::rebuilt`: rejoined
from a line the terminal broke, or with a control other than SGR removed from inside it),
and one whose `itemId` names no earlier `item/started` commandExecution item with the same
`command` (so a subcommand approval sharing a parent item is declined). That the app server
starts the item before asking is read from its protocol, not seen in a live turn. A Session
without a
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
`features.shell_tool=false`, `web_search="disabled"`, `features.multi_agent=false`,
`agents.enabled=false`, `features.apps=false`, `skills.max_context_tokens=1` and
`tools.experimental_request_user_input={enabled=false}` (which removes `request_user_input`
and `request_user_input_async`; no feature does), and the untrusted-project override and none
of AI6's servers; the Session has no gate, and `thread/start` and `thread/resume` carry
`sandbox = "read-only"`, `approvalPolicy = "never"` and `config = {"web_search":
"disabled"}`. An assistant (`AgentStart::assistant`) gets the same, with `mcp_servers`
holding exactly its servers, each with `default_tools_approval_mode="approve"` (under approval
`never` Codex 0.164.0 fails every MCP call it would ask about), and `-c
project_doc_max_bytes=0`: no `AGENTS.md` is read, and the mode's `context_file` (at most 1 MiB;
unreadable or larger refuses the start) is appended to the developer instructions on start,
after `initial_context`. A mode with skills is refused (`GUATIAO_ERR_UNSUPPORTED`: Codex keeps
no list of allowed skills), and `assistant_skills` says so, so a Station installs none.

With interpreters the thread runs scripts. Sandboxed (not `unsandboxed`), the command tools
stay on (`features.shell_tool=false` is left out), `thread/start` and `thread/resume` carry no
`sandbox` (Codex does not combine one with a permissions profile) and the app server gets:

```text
-c windows.sandbox="mxc"                       (Windows only: the implementation that denies reads)
-c permissions.ai6_assistant={filesystem={":root"="deny",":minimal"="read","<cwd>"="read",
     "<dir>"="read" (codex executable's, each interpreter's), "<cwd>/<files dir>"="write",
     "<station path>"="deny" (each, below)}}
-c default_permissions="ai6_assistant"
-c shell_environment_policy.include_only=[ai6::SCRIPT_ENVIRONMENT]
```

An executable or interpreter is found on the environment's `PATH` (Windows `PATHEXT`) with
links followed, and its install directory is readable
(`ai6::directorates::section_00::programs::program_dirs`: the directory above `bin`, so
`/opt/py/bin/python3` opens `/opt/py`); that directory is left out when it is `/`, is or holds
the home directory (`HOME`/`USERPROFILE`), the working directory, its parent or a credential
directory (`ai6::credential_dir_var`), or lies inside a credential directory.
`:minimal` re-opens all of `/etc`, `/usr` (`/usr/local/etc` and `/usr/etc` with it), `/dev`,
`/proc` and `/run` on Linux, and `C:\Windows`, `C:\Program Files` and `C:\ProgramData` on
Windows (measured with Codex 0.164.0, `codex sandbox -P`), and a `deny` inside a readable
directory wins whatever the order. So the Station's paths are denied again: `/etc/ai6`,
`/usr/local/etc` and `/usr/etc` (not on Windows), `ai6::station::safehouse::state_dir()`,
`AI6_HOME`, and the directory of the program hosting the handler (`current_exe`), or only that
program when its directory holds or lies in a readable install directory. A path that is or
holds the working directory or a readable install directory is not denied. A deny of a path that
does not exist is accepted. The profile has no network:
measured with Codex 0.164.0, app-server threads ran no proxy for a profile's network, so a
script reached unlisted hosts and host loopback; scripts get none under every policy, which a
developer-instructions line tells her (when the policy names hosts) and standard error logs.
Unsandboxed, `sandbox = "danger-full-access"` and no profile. Without interpreters the thread
stays read-only and, with `files`, is told it has no file tools (nothing offers one outside
`exec`). Every command execution item a scripts thread starts is reported as tool
`exec_command`. `assistant_scripts` runs `<executable> [-c windows.sandbox="mxc"] -c
permissions.ai6_assistant_probe={…} sandbox -P ai6_assistant_probe -- cmd.exe /d /c echo …`
(`/bin/sh -c 'echo …'` elsewhere) from a fresh temporary directory (`tempfile`, removed after)
with exactly the given environment (`std::process`, 60 s), and answers `None` only when it exits
successfully having printed the marker. The probe leads a process group of its own (Unix) or
starts suspended inside a kill-on-close Job Object (Windows), and that whole tree is killed once
it exits or times out, so nothing it started outlives it or holds its output open; the answer is cached 15 minutes per executable and configuration
environment. Codex merges the `mcp_servers` table into the user's own MCP servers rather
than replacing them (0.164.0: a user's `node_repl` started in an answer-only thread), so the
tool check below also switches off each of the user's servers by name. Checked against the
app-server protocol source (`ThreadStartParams`: `sandbox`, `approvalPolicy`, `config`; the
`web_search` key and its `disabled` value): those for `thread/start`. Inferred: that
`thread/resume` takes `config` as well, and that the read-only sandbox has no network.

A restricted thread that uses a tool anyway is killed, ends with a `failed` event naming
it, and nothing read after that becomes an event: any server request but
`account/chatgptAuthTokens/refresh` and `attestation/generate`, or an `item/started` /
`item/completed` whose item type is not `userMessage`, `agentMessage`, `reasoning`, `plan`,
`contextCompaction`, `enteredReviewMode` or `exitedReviewMode`, an `mcpToolCall` to one of the
thread's servers, and, for an assistant with scripts, `commandExecution` and (with `files`, or
unsandboxed) `fileChange` (an unknown item type counts as a tool; item types from `codex
app-server generate-json-schema`, 0.164.0).

Both are restricted threads, which start or resume only after the tool check:

```rust
pub const TOOL_CHECK_ENV: &str = "AI6_CODEX_TOOL_CHECK"; // strict (default, also unset) | warn; other values refuse
pub const KEPT_FEATURES: [&str; 5] = ["secret_auth_storage", "enable_request_compression",
                                      "system_proxy_fallback", "unbounded_connection_retries",
                                      "incremental_tools"];
```

`incremental_tools` is kept because without it `codex debug prompt-input` renders no tool
while the app server still sends the tools of the model's catalog entry (0.164.0); with it
kept, `prompt-input` matched the app server's request in every case compared.

Through the terminal AI6 supplies, with the app server's `-c` settings, environment and
working directory, the handler runs `codex --version`, then `codex features list` and adds
`-c features.<name>=false` for every enabled feature not in `KEPT_FEATURES` (with scripts,
nor `code_mode_host`, `shell_tool` or `unified_exec`) (a non-blank line
that is not `name stage state` fails the check), then `codex mcp list --json` with those and
adds `-c mcp_servers.<name>.enabled=false` for every listed server that is not one of the
thread's (output that is not one JSON array of objects with a string `name`, with nothing
after it, fails the check, as does a name that is not a bare TOML key: Codex splits a `-c`
key at every dot, so such a server cannot be switched off; only the names are kept, since
the output also holds each server's environment values), then `codex debug prompt-input`
with all of those and `-c model="<model>"` when the request names one (what Codex offers
depends on the model). It reads the offered tools from the `additional_tools` items
(`namespace.tool` inside a namespace) of the one JSON array that opens `[`, `{`, `"type"`;
output with no such array, anything but whitespace after it, no `message` item, or an item
of another type is unreadable. The thread passes when every offered tool is
`mcp__<server>.<tool>` for one of its servers and each server offers at least one
(answer-only: no tool); with scripts `functions.exec` and `functions.wait` are allowed too and
a server need not offer a tool directly (code mode nests them in `exec`), and the start is
reported as a weakening in either mode (`Codex gives Moneypenny's thread functions.exec and
functions.wait (code mode): her scripts may run any command inside Codex's sandbox, not only
<interpreters>`, before any weakening the check found). On a failure `strict` refuses with `GUATIAO_ERR_UNSUPPORTED` naming
what was found, before any app server starts; `warn` starts it, prints `ai6: weakening: …` on
standard error and queues an event of kind `weakened` with that text first. The feature and
server switches apply in both. The feature list and the server list are cached together in
the process for 15 minutes by executable, `--version` output, settings (without the working
directory's `projects` entry and the scripts profile), mode, scripts and the values of `CODEX_HOME`, `HOME`, `USERPROFILE`,
`APPDATA`, `LOCALAPPDATA`, `XDG_CONFIG_HOME` and `PATH`; a failed read is not cached.
`prompt-input` runs on every start and resume, but does not render the user's MCP servers
(they are deferred tools inside `functions.exec`): a server added to the user's configuration
within the 15 minutes is switched off only once the cached list expires.
With Codex 0.164.0 every model in the catalog is sent `functions.exec` and `functions.wait`
(code mode; `tool_mode = "code_mode_only"`), and no setting this handler passes removes them,
so in `strict` answer-only threads and assistants without scripts are refused on that
release; `warn` starts them as a weakening (fixtures in `tests/fixtures/codex-0.164.0/`). An
MCP server's tools are offered only inside `functions.exec` there, so an assistant without
scripts also finds its server missing; one with scripts is offered exactly
`functions.exec` and `functions.wait` (`prompt-input-scripts.json`) and passes. A server whose name collides with one of the user's is merged with it: with a stdio
server of that name Codex refuses its configuration and the check fails.

`stdio()` answers `HarnessStdio::Json`, so the Station starts the app server in a terminal
provider without a terminal (`pipe`), where its stream arrives as written, and refuses one
that gives it a terminal.

The terminal is opened at `STREAM_COLS` (32767) columns and read with
`ai6::directorates::q::terminal::JsonLines`: ConPTY's controls, its echo of the requests
written and lines it breaks are read as the stream Codex wrote. A line that is not JSON is
skipped while waiting for a reply, and is an event of kind `output` otherwise.

## Gotchas

The Codex executable must be available to the terminal environment. Tests use protocol
fixtures and a process-backed fake app-server; no real Codex account is required. The crate
builds both an `rlib` and its own Guatiao-loadable `cdylib`.
