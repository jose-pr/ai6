# ai6-agent-claude

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/jose-pr/ai6/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/jose-pr/ai6/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/jose-pr/ai6/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://jose-pr.github.io/ai6/rust/ai6_agent_claude/)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/ai6/test.yml)](https://github.com/jose-pr/ai6/actions/workflows/test.yml)

`ai6-agent-claude` implements AI6's Guatiao `AgentHandler` for Claude Code. It uses
Claude's stream-json interface over an AI6-owned terminal Run, preserving Claude's
provider ID for resume.

## Features

- Establishes initial context as provider system instructions before sending the Mission prompt.
- Resumes an existing Claude Run and accepts later Cable/message text.
- Connects to the AI6 per-Run Cable MCP endpoint using environment-substituted
  Streamable HTTP configuration; the bearer token is not included in process arguments.
- Converts provider stream events into AI6 `AgentEvent` values.
- Reports a usage or rate limit, with its reset time when Claude gives one, as an `AgentEvent`
  of kind `limit` so AI6 can park the Run instead of failing it.
- Reports the tokens and cost of a finished turn, names the tools an assistant message calls,
  and passes a Mission spend ceiling to Claude Code as `--max-budget-usd`.
- Holds a shell command that pushes to a protected ref, merges or releases until green ink
  covers it, through a `PreToolUse` hook (`ai6 green-ink gate`) passed with `--settings`.
- Runs a Mission under `--permission-mode dontAsk` with an explicit list of tools: reading,
  editing inside the working directory, AI6's MCP servers and, at `field` and `00` clearance,
  commands. An Agent's `handler_settings` (`allowed_tools`, `denied_tools`) change it, and a turn's
  refused calls are reported as an event of kind `denied`.
- Leaves out the working directory's own settings and `.mcp.json` unless AI6 allows them
  (`--setting-sources user --strict-mcp-config`), so a checkout cannot add hooks or MCP
  servers to an Agent.
- Starts an answer-only Run (`AgentStart::answer_only`, for a Chief of Staff review) with
  `--restricted`, no built-in tools and no MCP servers, so it can read and answer and do
  nothing else.

## Installation

This crate builds both the Rust library and the Guatiao-loadable dynamic library. The
Guatiao provider entry point and `ClaudeHandler` implementation ship together.

## API overview

| Module | Purpose |
| --- | --- |
| `ai6_agent_claude` | `ClaudeHandler`, `ClaudeConfig` and stream-json Run implementation |

## Development

Run `cargo fmt --all --check`, `cargo test -p ai6-agent-claude`,
`cargo clippy --workspace --all-targets --all-features -- -D warnings`, and
`cargo build -p ai6-agent-claude` from the AI6 workspace root. Workspace-wide Clippy keeps
the AI6 provider dependency's feature-gated library code in its intended build context.

## License

MPL-2.0. See [LICENSE](https://github.com/jose-pr/ai6/blob/main/LICENSE).
