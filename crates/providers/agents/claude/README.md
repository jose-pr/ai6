# ai6-agent-claude

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/jose-pr/ai6/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/jose-pr/ai6/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/jose-pr/ai6/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://jose-pr.github.io/ai6/rust/ai6_agent_claude/)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/ai6/test.yml)](https://github.com/jose-pr/ai6/actions/workflows/test.yml)

`ai6-agent-claude` implements AI6's Guatiao `AgentHandler` for Claude Code. It uses
Claude's stream-json interface over an AI6-owned terminal Session, preserving Claude's
provider-issued Session ID for resume.

## Features

- Establishes initial context as provider system instructions before sending the Mission prompt.
- Resumes an existing Claude Session and accepts later Cable/message text.
- Connects to the AI6 per-Session Cable MCP endpoint using environment-substituted
  Streamable HTTP configuration; the bearer token is not included in process arguments.
- Converts provider stream events into AI6 `AgentEvent` values.
- Reports a usage or rate limit, with its reset time when Claude gives one, as an `AgentEvent`
  of kind `limit` so AI6 can park the Session instead of failing it.
- Reports the tokens and cost of a finished turn, names the tools an assistant message calls,
  and passes a Mission spend ceiling to Claude Code as `--max-budget-usd`.

## Installation

This crate builds both the Rust library and the Guatiao-loadable dynamic library. The
Guatiao provider entry point and `ClaudeHandler` implementation ship together.

## API overview

| Module | Purpose |
| --- | --- |
| `ai6_agent_claude` | `ClaudeHandler`, `ClaudeConfig` and stream-json Session implementation |

## Development

Run `cargo fmt --all --check`, `cargo test -p ai6-agent-claude`,
`cargo clippy --workspace --all-targets --all-features -- -D warnings`, and
`cargo build -p ai6-agent-claude` from the AI6 workspace root. Workspace-wide Clippy keeps
the AI6 provider dependency's feature-gated library code in its intended build context.

## License

MPL-2.0. See [LICENSE](https://github.com/jose-pr/ai6/blob/main/LICENSE).
