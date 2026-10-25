# `ai6-agent-codex` — API header

Header-file-style reference for the `ai6-agent-codex` crate: every public item with its
signature, contract and gotchas, so a consuming agent skips the source. It ships inside
the package and is self-contained. Development documentation lives with the source at
<https://github.com/jose-pr/ai6>.

No features. MSRV: Rust 1.89. The crate depends on `ai6` and `serde_json`. It
re-exports `CodexHandler` and `CodexConfig` from the root. The crate contains no
`unsafe` code.

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

## Gotchas

The Codex executable must be available to the terminal environment. Tests use protocol
fixtures and a process-backed fake app-server; no real Codex account is required. The crate
builds both an `rlib` and its own Guatiao-loadable `cdylib`.
