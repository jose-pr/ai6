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

When AI6 supplies both `AI6_CABLE_MCP_URL` and `AI6_CABLE_MCP_TOKEN`, the handler adds
the authenticated Streamable HTTP Cable server through `--mcp-config`. The config contains
environment placeholders; the bearer token never appears in argv. If either variable is
absent, no AI6 MCP configuration is added.

## Gotchas

The Claude executable must be available to the terminal environment. Tests use protocol
fixtures and a process-backed fake CLI; no real Claude account is required.
