# ai6-transport-container

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/jose-pr/ai6/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/jose-pr/ai6/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/jose-pr/ai6/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://jose-pr.github.io/ai6/rust/ai6_transport_container/)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/ai6/test.yml)](https://github.com/jose-pr/ai6/actions/workflows/test.yml)

The `container` transport provider for [AI6](https://github.com/jose-pr/ai6): **the way into a
running Docker or Podman container** through the engine's command line. See the
[API header](https://github.com/jose-pr/ai6/blob/main/crates/providers/transports/container/AGENTS.md).

## Features

- `run` through `exec`, with the container's own working directory, environment and user.
- `files` built from shell commands run by `exec`, so the container needs a POSIX `sh` with
  `wc`, `tail`, `head`, `cat` and `mkdir` (BusyBox is enough).
- Docker and Podman alike: only the `engine` setting differs.

## Installation

Part of the AI6 workspace; built with it:

```bash
cargo build -p ai6 --features cli -p ai6-transport-container
```

## Quick start

```toml
[theater.sandbox]
access = { transport = { provider = "container", config = { container = "build", engine = "podman" } } }
agents = "remote-hands"
workdir = "/work"
```

## Development

```bash
cargo test -p ai6-transport-container
cargo clippy -p ai6-transport-container --all-targets -- -D warnings
```

A test against a real engine runs only when `AI6_TEST_CONTAINER` names a running container;
`AI6_TEST_CONTAINER_MEMORY_MB` (its `--memory` limit in MiB) also checks the container
reports its own memory limit.

## License

MPL-2.0. See [LICENSE](https://github.com/jose-pr/ai6/blob/main/LICENSE).
