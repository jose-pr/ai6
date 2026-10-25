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

When AI6 supplies both `AI6_CABLE_MCP_URL` and `AI6_CABLE_MCP_TOKEN`, the handler adds
the authenticated Streamable HTTP Cable server through `--mcp-config`. The config contains
environment placeholders; the bearer token never appears in argv. If either variable is
absent, no AI6 MCP configuration is added.

## Gotchas

The Claude executable must be available to the terminal environment. Tests use protocol
fixtures and a process-backed fake CLI; no real Claude account is required.
