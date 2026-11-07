# `ai6-agent-codex` — API header

Header-file-style reference for the `ai6-agent-codex` crate: every public item with its
signature, contract and gotchas, so a consuming agent skips the source. It ships inside
the package and is self-contained. Development documentation lives with the source at
<https://github.com/jose-pr/ai6>.

No features. MSRV: Rust 1.89. The crate depends on `ai6` (feature `gate`), `serde_json`,
`tempfile`, `ai6-process-tree` (`crates/process-tree/`, which kills the sandbox probe's process
tree), and `libc` (Unix) or `windows-sys` (Windows). It re-exports `CodexHandler` and
`CodexConfig` from the root, beside the tool check's `TOOL_CHECK_ENV` and `KEPT_FEATURES`. Its
only `unsafe` code, on Windows, puts the probe in its Job Object before it runs.

## `CodexConfig`

Every key is optional; each default is the value the handler used before it was a key. Counts,
bytes and characters are plain numbers (a large number takes the bound away); `_ms` is
milliseconds and `_secs` seconds.

| Key | Default | What |
| --- | --- | --- |
| `executable` | `codex` | Executable visible to the terminal environment |
| `json_line_bytes` | 67108864 | Bytes of one line of the app server's output held before what there is is handed over |
| `csi_bytes` | 64 | Bytes of one terminal control sequence before it is dropped, in the app server's output and in the tool check's commands |
| `context_file_bytes` | 1048576 | Bytes an assistant's context file may hold |
| `tail_lines` | 8 | Non-JSON lines a failed Session quotes |
| `tail_line_bytes` | 512 | Bytes of each of those lines |
| `close_wait_ms` | 2000 | How long `close` waits for the app server to have exited |
| `exit_wait_ms` | 1000 | How long the end of the output waits for the exit status |
| `start_timeout_ms` | 20000 | How long the app server may take to answer `initialize` |
| `request_timeout_ms` | 30000 | How long it may take to answer `thread/start`, `thread/resume` and `config/read` |
| `sandbox_probe_timeout_ms` | 60000 | How long the probe of the scripts sandbox may run |
| `sandbox_probe_lifetime_secs` | 900 | How long the probe's answer stands per executable and configuration environment |
| `sandbox_error_chars` | 300 | Characters of the probe's last line quoted when the sandbox did not run |
| `tree_scans` | 32 | Unix: times the process table is read again when the probe's process tree is killed; Windows ignores it |
| `tool_check_lifetime_secs` | 900 | How long the tool check's feature list stands |
| `tool_command_timeout_ms` | 120000 | How long one of the tool check's commands may run |

A Station sets them in `[providers.codex]` of its `station` record.

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
per-process config overrides for the Cable endpoint and token environment-variable name,
with `default_tools_approval_mode="approve"`.
The bearer token itself is not placed in process arguments.
Without that key, Codex 0.164.0 asks about each MCP call (measured on Windows and Linux through
a loopback stand-in for the model): under approval `never` the call fails with "MCP tool call
requires approval, but approval policy is never", and under `untrusted` it sends
`mcpServer/elicitation/request`, which this handler does not answer. With it the call completes
under both.
When AI6 supplies `AI6_THEATER_MCP_URL` and `AI6_THEATER_MCP_TOKEN` the Session is in
remote-hands mode: its work is on a remote Theater, reached only through the Theater tools.
App-server then also receives overrides for an `ai6-theater` server, its tools approved the
same way, and `-c features.shell_tool=false`, and `thread/start` and `thread/resume` carry
`sandbox = "read-only"` and `approvalPolicy = "never"`. Codex 0.164.0 reports both back from
`thread/start` (same measurement). Measured through a loopback stand-in on Windows and on Linux with
bubblewrap: at those settings without the shell setting, `exec_command` is offered, and on Linux a
command reads any file the Station's user can (no write and no network), while on Windows every
command tried is `blocked by policy`; with it, `exec_command` and `write_stdin` leave `functions.exec`
on both, `apply_patch` is refused by the sandbox, and the Theater's tools still work. Codex's other
tools stay (`view_image`, `request_user_input`, its goal and collaboration tools), since a thread at
`field` or `00` gets no tool check.
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
`command` (so a subcommand approval sharing a parent item is declined). A Session without a
Cable endpoint cannot ask for green ink, so it has no gate and everything is accepted, as with
the Claude handler. `item/fileChange/requestApproval` (params `threadId`, `turnId`, `itemId`,
`startedAtMs`, `reason`, `grantRoot`; the paths are on the `item/started` fileChange item it
follows) is answered by the rule the Claude handler's gate keeps, which holds only shell tools:
an edit inside the working directory is `accept`ed. Codex 0.164.0 applies an accepted patch
wherever it points, even from a thread that reports a read-only sandbox (measured against a
loopback stand-in), so a gated thread is declined, the same way as a command, a path outside the
working directory (a link out of it counts), a `grantRoot`, a `rebuilt` line and an item not
started. With a gate, `thread/start` and `thread/resume` carry
`approvalPolicy = "untrusted"`, so Codex asks before every command outside its own list of
safe reads; in remote-hands mode it stays `never`, and the Station gates `theater_run`.
Codex's documentation calls `untrusted` unsupported, but Codex 0.164.0 honours it (same
measurement, app server only): `thread/start` answers `approvalPolicy: "untrusted"`; a command
started through `exec_command` announces `item/started` first, then asks with that item's id and
the same `command`, which is the shell wrapper Codex runs (`/bin/bash -lc '…'`, `"…\\pwsh.exe"
-Command '…'`); `decline` ends the item as `declined` and nothing runs; `accept` runs it, with
writes in the working directory, though the thread reports a `readOnly` sandbox on Windows and
`workspaceWrite` on Linux when the user's configuration names none; and a `turn/steer` after a
decline reaches the model's next request. `git status`, `git log -1`, `ls`, `dir`, `cat` and
`echo` all asked, so in that release none of them is a safe read through `exec_command`.
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
project_doc_max_bytes=0`: no `AGENTS.md` is read, and the mode's `context_file` (at most `context_file_bytes`, 1 MiB by default;
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
`/usr/local/etc` and `/usr/etc` (not on Windows), `ai6::bootstrap::platform_state_dir()`,
the Station's own directories in `AssistantMode::station_paths`, and the directory of the program hosting the handler (`current_exe`), or only that
program when its directory holds or lies in a readable install directory. A path that is or
holds the working directory or a readable install directory is not denied. A deny of a path that
does not exist is accepted.
Codex merges the user's configuration into these settings rather than replacing them (0.164.0,
measured through a loopback stand-in for the model): a user's `[permissions.ai6_assistant]`
entries join the profile (`network.enabled = true` there let scripts reach loopback and the
internet), and `shell_environment_policy.set` reaches the variables kept; a user's
`sandbox_mode` or `default_permissions` does not win over the `-c` settings, and a legacy
`profile = "…"` key fails the app server's start. So a sandboxed scripts thread sends
`config/read {cwd}` after `initialize` and starts or resumes only when the effective
`default_permissions` is `ai6_assistant`, `permissions.ai6_assistant` holds exactly the
filesystem rules passed and nothing else non-null, and `shell_environment_policy` has only
`include_only` (as passed); then `thread/start` or `thread/resume` must answer with
`activePermissionProfile.id = "ai6_assistant"` and no `sandbox.networkAccess = true`. Otherwise
the app server is closed and the start or resume fails with `GUATIAO_ERR_UNSUPPORTED` naming
what differs (`Codex's configuration changes the sandbox Moneypenny's scripts run in (…)`), in
both tool-check modes. The profile has no network:
measured with Codex 0.164.0, app-server threads ran no proxy for a profile's network, so a
script reached unlisted hosts and host loopback; scripts get none under every policy, which a
developer-instructions line tells her (when the policy names hosts) and standard error logs.
Unsandboxed, `sandbox = "danger-full-access"` and no profile. Without interpreters the thread
stays read-only and, with `files`, is told it has no file tools (nothing offers one outside
`exec`). Every command execution item a scripts thread starts is reported as tool
`exec_command`; a command code mode's `exec` runs through its nested `exec_command` is announced
as such an item (`source: "unifiedExecStartup"`), and one whose sandbox fails to start is not
announced and does not run (0.164.0, measured through a loopback stand-in on Linux and Windows).
A link a script plants in a writable directory pointing outside does not let a script or
`apply_patch` write there: Codex runs `apply_patch` inside the same sandbox (0.164.0, measured:
bubblewrap and MXC; MXC refused to create a junction), so no mode is refused for it. `assistant_scripts` runs `<executable> [-c windows.sandbox="mxc"] -c
permissions.ai6_assistant_probe={…} sandbox -P ai6_assistant_probe -- cmd.exe /d /c echo …`
(`/bin/sh -c 'echo …'` elsewhere) from a fresh temporary directory (`tempfile`, removed after)
with exactly the given environment (`std::process`, 60 s), and answers `None` only when it exits
successfully having printed the marker. The probe leads a process group of its own (Unix) or
starts suspended inside a kill-on-close Job Object (Windows), and that whole tree is killed once
it exits or times out, so nothing it started outlives it or holds its output open; the answer is cached `sandbox_probe_lifetime_secs` (15 minutes) per executable and configuration
environment. Codex merges the `mcp_servers` table into the user's own MCP servers rather
than replacing them (0.164.0: a user's `node_repl` started in an answer-only thread), so the
tool check below also switches off each of the user's servers by name. Checked against the
app-server protocol source (`ThreadStartParams`: `sandbox`, `approvalPolicy`, `config`; the
`web_search` key and its `disabled` value): those for `thread/start`. Inferred: that
`thread/resume` takes `config` as well, and that the read-only sandbox has no network.

A Mission's thread at `desk`, or a clearance that is not one of the three
(`ai6::clearance_runs_commands`; `AgentStart::clearance`, `AgentResume::clearance`), an empty one
included, gets what a field Session gets (the AI6 servers its environment names, approved the same
way, and the project override `project_settings` decides) and `-c` for `features.shell_tool=false`,
`web_search="disabled"`, `features.multi_agent=false`, `agents.enabled=false`, `features.apps=false`,
`skills.max_context_tokens=1` and `tools.experimental_request_user_input={enabled=false}`;
`thread/start` and `thread/resume` carry `sandbox = "read-only"` and `approvalPolicy = "untrusted"`.
`features.shell_tool=false` takes `exec_command` and `write_stdin` out of `functions.exec`, and a direct
call of either answers `unsupported call`; `exec`'s JavaScript has no `require`, `process`, `fetch` or
dynamic `import` (Codex 0.164.0, Windows, through a loopback stand-in, `prompt-input-desk.json`).
Nothing is left that reads a file, since Codex reads through commands: such a thread has its prompt and
the `AGENTS.md` Codex loads. Under those settings Codex asks about each `apply_patch`, an accepted one
inside the working directory is applied, a thread resumed with them after one started without them
loses the command tools though its history still lists them (same measurement), and the handler answers
`item/fileChange/requestApproval` as for a gated thread, gate or no gate: outside the working
directory, a `grantRoot`, a `rebuilt` line and an item not started are declined. In remote-hands mode the
thread stays read-only with approval `never`, so `apply_patch` is refused without asking, and `-c
mcp_servers.ai6-theater.disabled_tools=["theater_run","theater_write"]` takes those two out of `exec`
(same measurement). `field` and `00` get a field Session as above, with no tool check.

An overlay's skill (`AgentStart::overlays`) is a routing line naming its `SKILL.md`, after
`initial_context` in the developer instructions: Codex 0.164.0 lists the skills of `CODEX_HOME/skills`,
the project's own skill directories and what it ships, took no other root through
`skills.config`, `skills.paths`, `skills.roots`, `skills.extra_roots` or `skills/list`'s
`perCwdExtraUserRoots` (measured with `codex debug prompt-input` and the app server, no model), and
`CODEX_HOME` is shared by every Session of the Agent. A `field` thread reads the file with its command
tools; a `desk` thread has no tool that reads a file, so there the line names what it cannot open.

A restricted thread that uses a tool anyway is killed, ends with a `failed` event naming
it, and nothing read after that becomes an event: any server request but
`account/chatgptAuthTokens/refresh` and `attestation/generate` (and, on a `desk` thread,
`item/fileChange/requestApproval`), or an `item/started` /
`item/completed` whose item type is not `userMessage`, `agentMessage`, `reasoning`, `plan`,
`contextCompaction`, `enteredReviewMode` or `exitedReviewMode`, an `mcpToolCall` to one of the
thread's servers, for an assistant with scripts, `commandExecution` and (with `files`, or
unsandboxed) `fileChange`, and for a `desk` thread on the Station's disk, `fileChange` (an unknown item
type counts as a tool; item types from `codex app-server generate-json-schema`, 0.164.0).

All three are restricted threads, which start or resume only after the tool check:

```rust
pub const TOOL_CHECK_ENV: &str = "AI6_CODEX_TOOL_CHECK"; // strict (default, also unset) | warn; other values refuse
pub const KEPT_FEATURES: [&str; 5] = ["secret_auth_storage", "enable_request_compression",
                                      "system_proxy_fallback", "unbounded_connection_retries",
                                      "incremental_tools"];
```

`incremental_tools` is kept because without it `codex debug prompt-input` renders no tool
while the app server still sends the tools of the model's catalog entry (0.164.0); with it
kept, `prompt-input` matched the app server's request in every case compared.

Through the terminal AI6 supplies, out of the Session's sight (`AgentTerminal::spawn_private`: the
listing holds each MCP server's environment values, and no viewer reads any of it), with the app
server's `-c` settings, environment and working directory, the handler runs `codex --version`, then `codex features list` and adds
`-c features.<name>=false` for every enabled feature not in `KEPT_FEATURES` (with scripts,
nor `code_mode_host`, `shell_tool` or `unified_exec`; at `desk`, nor `code_mode_host`) (a non-blank line
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
a server need not offer a tool directly (code mode nests them in `exec`), and every tool
`exec`'s description declares (`declare const tools: { NAME(`, one per `exec tool declaration:`
heading; a count that differs, or none, is unreadable and fails the check) must be
`exec_command` or `write_stdin`, `apply_patch` when the thread may change files (`files`, or
unsandboxed), `list_mcp_resources`, `list_mcp_resource_templates` or `read_mcp_resource` when it
has servers, `mcp__<server>__<tool>` of one of its servers (the server's name with every
character but letters and digits as `_`), or `clock__curr_time`, which only reads the time and
which Codex 0.164.0 offers with no setting to remove it. That release declares exactly
`apply_patch`, `exec_command`, `list_mcp_resource_templates`, `list_mcp_resources`,
`read_mcp_resource`, `write_stdin` and `clock__curr_time`, with or without `files`: in `strict`
a sandboxed scripts thread without `files` is refused (`apply_patch`). A `desk` thread passes on the
same terms with code mode allowed, `exec_command` and `write_stdin` forbidden and `apply_patch`
allowed; that release, with `features.shell_tool=false`, declares the rest and neither of those, and
a release that declares either is refused in `strict`. Deferred tools, an MCP
server's among them, are left out of the description and not seen. The start is
reported as a weakening in either mode (`Codex gives Moneypenny's thread functions.exec and
functions.wait (code mode): her scripts may run any command inside Codex's sandbox, not only
<interpreters>`, before any weakening the check found). On a failure `strict` refuses with `GUATIAO_ERR_UNSUPPORTED` naming
what was found, before any app server starts; `warn` starts it, prints `ai6: weakening: …` on
standard error and queues an event of kind `weakened` with that text first. The feature and
server switches apply in both. The feature list and the server list are cached together in
the process for `tool_check_lifetime_secs` (15 minutes) by executable, `--version` output, settings (without the working
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
`functions.exec` and `functions.wait` (`prompt-input-scripts.json`, `exec`'s description whole)
and passes when it has `files`. A server whose name collides with one of the user's is merged with it: with a stdio
server of that name Codex refuses its configuration and the check fails.

`program()` answers `executable`, which the Station looks up on the Session's own `PATH` before it
starts it. `stdio()` answers `HarnessStdio::Json`, so the Station starts the app server in a terminal
provider without a terminal (`pipe`), where its stream arrives as written, and refuses one
that gives it a terminal.

The terminal is opened at `STREAM_COLS` (32767) columns and read with
`ai6::directorates::q::terminal::JsonLines`: ConPTY's controls, its echo of the requests
written and lines it breaks are read as the stream Codex wrote. A line that is not JSON is
skipped while waiting for a reply, and is an event of kind `output` otherwise.

## Listing models

`CodexHandler::models(environment)` starts `codex app-server` with exactly that environment, answers
`initialize`, sends `initialized` and asks `model/list`: `result.data[]` holds `id`, `model`,
`displayName`, `description` and `hidden`; a hidden model is left out, as Codex does not offer it. No
model turn is involved. Measured with Codex 0.164.0 on Windows with an empty `CODEX_HOME`, no login
and an unreachable proxy: it answered from the catalog it ships with (`gpt-6.1-sol` default,
`gpt-6-astra`, and others). With a login it may refresh from the account; that was not measured. The
wait is `start_timeout_ms` plus `request_timeout_ms`; no answer, or one without `data`, is `unknown`
with the reason. `cargo test -p ai6-agent-codex -- --ignored live_` with `AI6_TEST_LIVE_HARNESSES=1`
runs it against the installed program.

## A login kept in the Registry

A `harness-login` pool account with `files = [".codex/auth.json"]`. The home needs
`cli_auth_credentials_store = "file"` in `.codex/config.toml` (add it to `files`), or `auto` may use the
OS keyring. Measured on 0.164.0 with a fake login and a loopback stand-in, never a real one, and from the
source at tag `rust-v0.164.0`:

- It refreshes by itself when the access token's JWT `exp` is within 5 minutes, else when `last_refresh`
  is over 8 days old (source). On a 401 it reloads the file, then refreshes (source).
- `codex login status` never renews. `codex app-server`, asked `account/read` with `refreshToken: true`,
  renews even a day from the end, and with `false`, or `model/list`, within the window. It exits as soon
  as its input ends, before answering: a `renew_command` must hold its input open
  (`sh -c '{ printf ...; sleep 10; } | codex app-server'`).
- A refused refresh leaves the file as it was and reports no account; a code of `refresh_token_reused`,
  `refresh_token_expired` or `refresh_token_invalidated` is permanent (source).
- Unmeasured: whether the provider rotates the refresh token, whether an old access token stays good
  after a renewal, and how long an unused refresh token lasts. Keep `max_concurrent = 1`.

## Gotchas

The Codex executable must be available to the terminal environment. Tests use protocol
fixtures and a process-backed fake app-server; no real Codex account is required. The crate
builds both an `rlib` and its own Guatiao-loadable `cdylib`.
