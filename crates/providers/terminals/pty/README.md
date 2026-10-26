# ai6-terminal-pty

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/jose-pr/ai6/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/jose-pr/ai6/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/jose-pr/ai6/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://jose-pr.github.io/ai6/rust/ai6_terminal_pty/)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/ai6/test.yml)](https://github.com/jose-pr/ai6/actions/workflows/test.yml)

The `pty` terminal provider for [AI6](https://github.com/jose-pr/ai6), on
[`portable-pty`](https://crates.io/crates/portable-pty): **one provider for `openpty` on
Unix and ConPTY on Windows**. The session code is shared; the platform parts (owning the
child's process tree, translating input) live in `unix.rs` and `windows.rs`. See the
[API header](https://github.com/jose-pr/ai6/blob/main/crates/providers/terminals/pty/AGENTS.md).

## Features

- Interactive pseudo-terminal Sessions for agent harnesses, with resize and bounded reads.
- On Unix, no line editing or echo: an input line of any length reaches the harness whole.
- Kills the whole process tree: the process group on Unix, a Job Object on Windows.
- Builds as both a Rust library and the Guatiao provider library a Station loads.

## Installation

Part of the AI6 workspace; built with it:

```bash
cargo build -p ai6 --features cli -p ai6-terminal-pty
```

## Quick start

```bash
cargo run -p ai6 --features cli -- serve --plugin-path target/debug
```

## Development

```bash
cargo test -p ai6-terminal-pty
cargo clippy -p ai6-terminal-pty --all-targets -- -D warnings
```

## License

MPL-2.0. See [LICENSE](https://github.com/jose-pr/ai6/blob/main/LICENSE).
