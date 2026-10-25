# ai6-agent-codex

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/jose-pr/ai6/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/jose-pr/ai6/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/jose-pr/ai6/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://jose-pr.github.io/ai6/rust/ai6_agent_codex/)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/ai6/test.yml)](https://github.com/jose-pr/ai6/actions/workflows/test.yml)

`ai6-agent-codex` implements AI6's Guatiao `AgentHandler` for the Codex app-server
JSON-RPC protocol. The daemon supplies a terminal Session; this handler owns Codex
initialization, thread start/resume, turn submission and event decoding.

## Features

- Starts a Codex app-server thread with Mission context and prompt.
- Resumes using the provider-issued thread identifier, verifies the returned ID, and
  preserves the Mission working directory.
- Sends later messages as turns and emits provider events as AI6 `AgentEvent` values.
- Reports a usage-limit error, with the reset time of the exhausted quota window, as an
  `AgentEvent` of kind `limit` so AI6 can park the Session instead of failing it.
- Reports token usage per model request (Codex reports no cost) and names the web-search and
  tool calls a turn makes.
- Uses per-process app-server configuration to connect to the AI6 Cable MCP endpoint;
  the bearer token is read from the launch environment.

## Installation

The provider library contains its Guatiao entry point and builds as both an ordinary
Rust library and a dynamically loadable provider. Build it with
`cargo build -p ai6-agent-codex` from the AI6 workspace root.

## API overview

| Module | Purpose |
| --- | --- |
| `ai6_agent_codex` | `CodexHandler`, `CodexConfig` and app-server JSON-RPC Session implementation |

## Development

Run `cargo fmt --all --check`, `cargo test -p ai6-agent-codex`, and
`cargo clippy --workspace --all-targets --all-features -- -D warnings` from the AI6
workspace root. Workspace-wide Clippy keeps the AI6 provider dependency's feature-gated
library code in its intended build context.

## License

MPL-2.0. See [LICENSE](https://github.com/jose-pr/ai6/blob/main/LICENSE).
