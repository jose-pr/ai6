# ai6-agent-antigravity

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/jose-pr/ai6/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/jose-pr/ai6/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/jose-pr/ai6/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://jose-pr.github.io/ai6/rust/ai6_agent_antigravity/)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/ai6/test.yml)](https://github.com/jose-pr/ai6/actions/workflows/test.yml)

`ai6-agent-antigravity` implements AI6's Guatiao `AgentHandler` for the Antigravity CLI (`agy`). It
uses agy's stream-json interface over an AI6-owned terminal Session and keeps the conversation ID
agy reports as the provider Session ID. agy is closed source and updates itself: this crate is
written against what version 1.4.12 was measured to do, and the recorded lines its tests replay
are what would show a later version doing otherwise.

## Features

- Reads a Session's ID from its stream, and sends a resumed Session's message only once the stream
  has named the conversation asked for, since agy starts a new one for an ID it does not hold.
- Reports a quota limit as an `AgentEvent` of kind `limit`, in words a setting can change.
- Reports a tool the permission check or a hook refused as an event of kind `denied`, though agy's
  turn still says `SUCCESS` and exits 0.
- Gives each launch a directory of its own for what agy has no flag for: a custom agent that fixes
  the tools and carries the standing context as its system prompt, the MCP servers, and the green
  ink hook. Nothing is written to the user's files or the checkout.
- Holds `desk` clearance with no command tool, and a Session that only answers, or Moneypenny's,
  with reading and its own MCP tools; asks for more are refused by name.
- Refuses a working directory that carries agy hooks or MCP servers AI6 did not allow, which agy
  would otherwise load whether or not it trusts the directory.
- Turns agy's updater off during a Session, unless the Agent lets it run.

## Installation

This crate builds both the Rust library and the Guatiao-loadable dynamic library, under the handler
ID `antigravity`. The CLI's own login is a file under the `HOME` the Session's environment names, so
an Agent's `env` sets `HOME`.

## API overview

| Module | Purpose |
| --- | --- |
| `ai6_agent_antigravity` | `AntigravityHandler`, `AntigravityConfig` |

`AGENTS.md` beside this file has the settings, the flags and every measured fact.

## Development

Run `cargo fmt --all --check`, `cargo test -p ai6-agent-antigravity`,
`cargo clippy --workspace --all-targets --all-features -- -D warnings` from the AI6 workspace root.
Its tests need no CLI, account or network.

## License

MPL-2.0. See [LICENSE](https://github.com/jose-pr/ai6/blob/main/LICENSE).
