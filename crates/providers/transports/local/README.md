# ai6-transport-local

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/jose-pr/ai6/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/jose-pr/ai6/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/jose-pr/ai6/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://jose-pr.github.io/ai6/rust/ai6_transport_local/)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/ai6/test.yml)](https://github.com/jose-pr/ai6/actions/workflows/test.yml)

The `local` transport provider for [AI6](https://github.com/jose-pr/ai6): **the way into the
Station's own host.** It runs commands and reads, writes and lists files there, so the same
Theater tools work on the Station's host as on any remote one. See the
[API header](https://github.com/jose-pr/ai6/blob/main/crates/providers/transports/local/AGENTS.md).

## Features

- `run` with environment, working directory, standard input, a timeout and an output limit.
- `read_file`, `write_file` and `list_dir` on the host's filesystem.
- No terminal and no forward: a Station opens terminals on its own host with its Terminal
  provider (`pty`).

## Installation

Part of the AI6 workspace; built with it:

```bash
cargo build -p ai6 --features cli -p ai6-transport-local
```

## Quick start

The `local` Theater is always reachable. To drive it through the transport (for example to
test remote-hands mode without a second machine), declare it:

```toml
[theater.local]
access = "local"
agents = "remote-hands"
```

## Development

```bash
cargo test -p ai6-transport-local
cargo clippy -p ai6-transport-local --all-targets -- -D warnings
```

## License

MPL-2.0. See [LICENSE](https://github.com/jose-pr/ai6/blob/main/LICENSE).
