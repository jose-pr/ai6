# ai6-terminal-pipe

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/jose-pr/ai6/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/jose-pr/ai6/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/jose-pr/ai6/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://jose-pr.github.io/ai6/rust/ai6_terminal_pipe/)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/ai6/test.yml)](https://github.com/jose-pr/ai6/actions/workflows/test.yml)

The `pipe` terminal provider for [AI6](https://github.com/jose-pr/ai6): **a program on plain
pipes, with no terminal**. Its standard input is a pipe, and its standard output, with standard
error merged in, is the output stream, byte for byte. A Station gives it to harnesses whose
handler speaks JSON over standard input and output (Claude Code, Codex), which a terminal would
render or refuse. See the
[API header](https://github.com/jose-pr/ai6/blob/main/crates/providers/terminals/pipe/AGENTS.md).

## Features

- Output exactly as the program wrote it: no line-break translation, no control sequences.
- Input of any length reaches the program whole; nothing is echoed.
- Standard error merged into the output (the default) or dropped.
- Kills the whole process tree: a process group on Unix, a Job Object on Windows.
- Builds as both a Rust library and the Guatiao provider library a Station loads.

## Installation

Part of the AI6 workspace; built with it:

```bash
cargo build -p ai6 --features cli -p ai6-terminal-pipe
```

## Quick start

```bash
cargo run -p ai6 --features cli -- serve --plugin-path target/debug
```

## Development

```bash
cargo test -p ai6-terminal-pipe
cargo clippy -p ai6-terminal-pipe --all-targets -- -D warnings
```

## License

MPL-2.0. See [LICENSE](https://github.com/jose-pr/ai6/blob/main/LICENSE).
