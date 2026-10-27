# `ai6-agent-claude` — API header

Header-file-style reference for the `ai6-agent-claude` crate: every public item with its
signature, contract and gotchas, so a consuming agent skips the source. It ships inside
the package and is self-contained. Development documentation lives with the source at
<https://github.com/jose-pr/ai6>.

No features. MSRV: Rust 1.89. The crate depends on `ai6`, `serde` and `serde_json`.
It exposes `ClaudeHandler` and `ClaudeConfig` from the root. The crate contains no
`unsafe` code.

## `ClaudeHandler`

`ClaudeHandler` and `ClaudeConfig` implement the `ai6::directorates::section_00::agent::AgentHandler` Guatiao
kind. `ClaudeConfig` selects the executable; `ClaudeHandler` launches Claude Code through
the terminal instance supplied by AI6. Initial context becomes appended system
instructions (`--append-system-prompt`) before the task prompt is written to the stream.
Claude's provider Session ID is returned from `session_id()` after the provider emits it
and is supplied unchanged to `resume`. Claude Code 2.1.310 records the system prompt with
the Session (`--system-prompt-snapshot`, on by default) and reuses it on resume until the
Session is compacted; after that it builds the prompt from the resuming launch, so resume
passes `AgentResume::initial_context` as `--append-system-prompt` too when it is not empty.
A restricted Session (`answer_only`, or `assistant`) whose `assistant` message calls a tool
not named `mcp__<server>__<tool>` for one of its servers (nor, for an assistant with `files`,
one of its five file tools) is killed, ends with a `failed`
event naming the tools, and nothing read after that becomes an event. A call is any
`tool_use` content block (named by its `name`) and any other block whose type ends in
`_tool_use` (named `TYPE:NAME`, such as `server_tool_use:advisor` or `mcp_tool_use:echo`):
those the Messages API runs itself, outside every MCP server, so a restricted Session never
may. Whether Claude Code's stream carries such blocks is not documented; the block types
are the Messages API's. A restricted Session also runs with
`CLAUDE_CODE_DISABLE_ADVISOR_TOOL=1` in its environment, overriding the Agent's: Claude Code
2.1.310 otherwise attaches its server-side `advisor` tool to model requests (seen with
`claude-opus-5-5`), and neither `--restricted`, `--tools=` nor a deny rule removes it. That
variable is the only documented switch (code.claude.com/docs/en/advisor and
`/docs/en/env-vars`); no setting, flag or permission rule turns server-side tools off as a
class. `--resume <id>` finds a Session only from the directory that recorded
it (Claude Code 2.1.310: from another one it answers "No conversation found with session
ID"), so resume must be given that working directory. With `--input-format stream-json`
Claude Code writes nothing, not even its stream's initialization, until it has read a
message, so resume sends its optional follow-up message right after starting the process and
returns without waiting for output. `session_id()` is `None` until Claude reports the
Session; a reported ID other than the one resumed makes `next_event` fail ("Claude resumed a
different Session than requested"), and a Session Claude cannot find ends with a `failed`
event quoting its message. The end of Claude's output is the terminal's
end-of-file, or Claude's exit while the terminal keeps the output open (a read that times
out asks the terminal's `exit_status`). After every line written before it, that emits one
`failed` event whose text names the exit (`exited with code N`, `was ended by signal N`, or
`closed its terminal` when the terminal cannot tell) and quotes the last eight lines Claude
wrote that were not JSON (512 bytes each); a read timeout otherwise returns `None`. `close`
kills the terminal and waits up to 2 s for its exit status, so the working directory is free
when it returns.

`stdio()` answers `HarnessStdio::Json`, so the Station starts Claude Code in a terminal
provider without a terminal (`pipe`) and refuses one that gives it a terminal: with `-p`,
Claude Code 2.1.310 exits at once when its standard input is a terminal ("Input must be
provided either through stdin or as a prompt argument when using --print").

`interrupt` writes one stream-json control request,
`{"type":"control_request","request_id":"ai6-interrupt-N","request":{"subtype":"interrupt"}}`
(`N` counts from 1 per Session), the shape the Agent SDK sends; no `^C` byte is written,
since without a terminal it would reach Claude as data. Claude answers with a
`control_response` line, which `next_event` reads and drops, never an event; one whose
`response.subtype` is `error` is logged to standard error with its `error`. Claude Code
2.1.310 answers an interrupt with `success` and `{"still_queued":[]}` even when no turn is
running, and stays running (checked against that version over pipes, with no message sent).
Each message and request is written in a single write, so an interrupt sent while a message
is being written cannot land inside it.

The terminal is opened at `STREAM_COLS` (32767) columns and read with
`ai6::directorates::q::terminal::JsonLines`, so the controls ConPTY writes on Windows and a
line it breaks are read as the stream Claude wrote; a line that is not JSON becomes an event
of kind `output` with the line as text.

A usage or rate limit is emitted as an event of kind `limit` (`ai6::LIMIT_EVENT`) with
`AgentEvent::limit` set, never as `completed`. Two stream shapes are recognised: a
`rate_limit_event` whose `rate_limit_info.status` is `rejected` (reset time from `resetsAt`,
window from `rateLimitType`), and an error `result` whose text names the limit
(`usage limit`, `limit reached` or `hit your limit`). For the latter the reset time is the
number after a trailing `|` (`... usage limit reached|1790000000`); a limit stated only as a
local clock time ("resets 3pm") leaves `resets_at` as `None` rather than guessing a date.
Seconds and milliseconds are both accepted. Other `rate_limit_event` statuses pass through
unchanged. These shapes come from recorded protocol fixtures, not from a live account.

Every `rate_limit_event`, whatever its status, also sets `AgentEvent::quota` to one
`QuotaReport`: `window` from `rateLimitType` (`unknown` when absent), `status`, `resets_at`
from `resetsAt`, and `used_permille` from `utilization` only when it is a fraction between 0
and 1. `utilization` is in the published `SDKRateLimitInfo` type of
`@anthropic-ai/claude-agent-sdk` (0.3.310) without a documented scale and has not been seen
from a live account, so another scale leaves `used_permille` as `None`. Other events carry
no quota.

A `result` event also carries `AgentEvent::usage`: its `usage` block (`input_tokens`,
`output_tokens`, `cache_creation_input_tokens` as cache writes, `cache_read_input_tokens`) and
`total_cost_usd` in micro-dollars, marked `cumulative`. Whether `usage` is per turn or per
process is not confirmed from a live account, so the daemon counts a decrease as a new
counter. An `assistant` message with tool-call blocks (above) becomes kind `tool` with
`AgentEvent::tools` set to their names. `AgentStart::max_cost_micro_usd` and
`AgentResume::max_cost_micro_usd` are passed as `--max-budget-usd`.

When AI6 supplies both `AI6_CABLE_MCP_URL` and `AI6_CABLE_MCP_TOKEN`, the handler adds
the authenticated Streamable HTTP Cable server through `--mcp-config`. The config contains
environment placeholders; the bearer token never appears in argv. If either variable is
absent, no AI6 MCP configuration is added.

When AI6 supplies `AI6_THEATER_MCP_URL` and `AI6_THEATER_MCP_TOKEN` the Session is in
remote-hands mode: its work is on a remote Theater, reached only through the Theater tools.
The handler then registers that server as `ai6-theater` (same placeholder scheme), allows
its tools (`--allowedTools mcp__ai6-theater`) and denies Claude's own shell and file tools
(`--disallowedTools Bash,PowerShell,Edit,Write,NotebookEdit,Read,Glob,Grep`) so
nothing it does lands on the Station. Both flags are variadic, so each is followed by another
flag. This applies to resume as well.

With the Cable endpoint present, start and resume also pass `--settings` with the green ink
gate: `{"disableAllHooks": false, "hooks": {"PreToolUse": [{"matcher": "Bash|PowerShell",
"hooks": [{"type": "command", "command": PROGRAM, "args": ["green-ink", "gate",
"--cable-url", URL], "timeout": 60, "onFailure": "block"}]}]}}`. `URL` is the Session's
`AI6_CABLE_MCP_URL`, pinned in the hook so a project's settings (`env`) cannot point the gate
elsewhere; the token stays in the environment, out of every command line (a redirected token
only makes the gate hold). `PROGRAM` is `AI6_GREEN_INK_GATE` from the Session environment
(which only the Agent's roster can set) when not blank, else the executable hosting the
handler (`ai6` on a Station or field runner). The hook asks the Station whether a live green ink approval covers
each governed step of a shell command and denies the call otherwise
(`ai6::directorates::section_00::gate`). `onFailure` needs Claude Code 2.1.295 or later; a
hook that cannot start or times out then blocks the call. The settings follow the Claude
Code hooks reference and are not exercised against a live Claude Code.

When `AgentStart::project_settings` (or `AgentResume::project_settings`) is `false`, the
default AI6 decides for an Operation, start and resume add `--setting-sources user`: Claude
Code loads its user setting source only. The `project` source is what loads the working
directory's `.claude/settings.json` and its hooks, project `CLAUDE.md` and `.claude/rules`,
and project skills, commands and subagents, and `local` loads `.claude/settings.local.json`
and `CLAUDE.local.md`, so none of those load (Agent SDK reference, "Control filesystem
settings with settingSources", which the CLI flag sets). Project instructions in `CLAUDE.md`
are therefore not read either. `--strict-mcp-config` is added unless both
`project_settings` and `project_mcp` are `true`, so `.mcp.json` servers connect only when
the Operation's pinned manifest holds that file or it loads project settings outright.
`--settings` (the green ink hook) sits above the setting files and applies in every mode.
These follow the Claude Code documentation; they are not exercised against a live Claude
Code.

When `AgentStart::answer_only` (or `AgentResume::answer_only`) is `true`, which AI6 sets for
a Chief of Staff review, start and resume add, in place of the project settings, MCP and gate
flags: `--restricted`, `--tools=`, `--disallowedTools
Bash,PowerShell,Edit,Write,NotebookEdit,Read,Glob,Grep,WebFetch,WebSearch,Task,Agent,TodoWrite,mcp__*`,
`--setting-sources user`, `--strict-mcp-config` and `--mcp-config {"mcpServers":{}}`. An
assistant Session's list is the same with `Skill` in place of `mcp__*`. An assistant whose
mode names `files` instead gets `--tools Read,Write,Edit,Glob,Grep`, `--permission-mode
dontAsk`, `--disallowedTools Bash,PowerShell,NotebookEdit,WebFetch,WebSearch,Task,Agent,
TodoWrite,Skill,Edit(**/.*),Edit(**/.*/**)` (no path with a name starting with `.`) and `--allowedTools mcp__<server>,…,Read,Glob,Grep,Edit(/<dir>/**),…` (Edit
rules cover Write; `/` is the working directory), and its tripwire lets those five tools
through. Measured with 2.1.310 against a loopback stand-in for the API (no model reached):
Write and Edit under `personal/` ran; Write to another directory, to the working directory's
own `CLAUDE.md`, to `personal/.git/hooks` and to `personal/../x` were denied; `--restricted`
refused Read outside the working directory. With the dot-path rules, Write and Edit to
`.git/config`, `.GIT/config`, `.git./config`, `PERSONAL\.git\config`, `.gitattributes`,
`notes/.hidden` and `scratch/.x` were refused on Windows, and so were writes through 8.3
short names (`GIT~1\config`, `GITATT~1`) even after reading them. Under `--restricted` the working directory's
`CLAUDE.md` does not load, with or without `--setting-sources user,project`, so a mode's
`context_file` goes in with `--append-system-prompt-file`, which reached the request's
system prompt. Every assistant Session also has `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1`.
`MultiEdit` and `LS`
are not denied: Claude Code 2.1.310 warns that such a rule matches no known tool. `Task` is
the former name of `Agent`, which Claude Code still accepts. Taken
from `claude --help` (2.1.310) and the CLI reference: `--tools ""` turns every built-in tool
off but not MCP tools, hence the `mcp__*` denial and the empty strict MCP config;
`--restricted` (2.1.248 or later) removes the command and code tools and WebFetch and loads
only managed settings besides AI6's. Inferred, not checked against a live Claude Code: that
`--tools=` parses as the empty list, and that a stored login or `ANTHROPIC_API_KEY` still
authenticates under `--restricted`. A Claude Code that does not know a flag refuses to start,
which ends the review without an answer.

## Gotchas

The Claude executable must be available to the terminal environment. The gate is best
effort: a command that builds its arguments at run time, or a step taken through a tool
other than the shell, is not seen; leased credentials are the guarantee. Tests use protocol
fixtures and a process-backed fake CLI; no real Claude account is required.
