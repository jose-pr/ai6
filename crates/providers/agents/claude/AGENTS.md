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
instructions before the task prompt is written to the stream. Claude's provider Session
ID is returned from `session_id()` after the provider emits it and is supplied unchanged
to `resume`. Resume waits for Claude's stream initialization, verifies the reported ID,
and only then returns the resumed operation or sends its optional follow-up message.
End-of-file before a completion event emits one `failed` event; a read timeout returns
`None`.

A usage or rate limit is emitted as an event of kind `limit` (`ai6::LIMIT_EVENT`) with
`AgentEvent::limit` set, never as `completed`. Two stream shapes are recognised: a
`rate_limit_event` whose `rate_limit_info.status` is `rejected` (reset time from `resetsAt`,
window from `rateLimitType`), and an error `result` whose text names the limit
(`usage limit`, `limit reached` or `hit your limit`). For the latter the reset time is the
number after a trailing `|` (`... usage limit reached|1790000000`); a limit stated only as a
local clock time ("resets 3pm") leaves `resets_at` as `None` rather than guessing a date.
Seconds and milliseconds are both accepted. Other `rate_limit_event` statuses pass through
unchanged. These shapes come from recorded protocol fixtures, not from a live account.

A `result` event also carries `AgentEvent::usage`: its `usage` block (`input_tokens`,
`output_tokens`, `cache_creation_input_tokens` as cache writes, `cache_read_input_tokens`) and
`total_cost_usd` in micro-dollars, marked `cumulative`. Whether `usage` is per turn or per
process is not confirmed from a live account, so the daemon counts a decrease as a new
counter. An `assistant` message with `tool_use` blocks becomes kind `tool` with
`AgentEvent::tools` set to the block names. `AgentStart::max_cost_micro_usd` and
`AgentResume::max_cost_micro_usd` are passed as `--max-budget-usd`.

When AI6 supplies both `AI6_CABLE_MCP_URL` and `AI6_CABLE_MCP_TOKEN`, the handler adds
the authenticated Streamable HTTP Cable server through `--mcp-config`. The config contains
environment placeholders; the bearer token never appears in argv. If either variable is
absent, no AI6 MCP configuration is added.

When AI6 supplies `AI6_THEATER_MCP_URL` and `AI6_THEATER_MCP_TOKEN` the Session is in
remote-hands mode: its work is on a remote Theater, reached only through the Theater tools.
The handler then registers that server as `ai6-theater` (same placeholder scheme), allows
its tools (`--allowedTools mcp__ai6-theater`) and denies Claude's own shell and file tools
(`--disallowedTools Bash,PowerShell,Edit,MultiEdit,Write,NotebookEdit,Read,Glob,Grep,LS`) so
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

## Gotchas

The Claude executable must be available to the terminal environment. The gate is best
effort: a command that builds its arguments at run time, or a step taken through a tool
other than the shell, is not seen; leased credentials are the guarantee. Tests use protocol
fixtures and a process-backed fake CLI; no real Claude account is required.
