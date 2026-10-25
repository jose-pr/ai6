# ai6-theater-container

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/jose-pr/ai6/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/jose-pr/ai6/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/jose-pr/ai6/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://jose-pr.github.io/ai6/rust/ai6_theater_container/)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/ai6/test.yml)](https://github.com/jose-pr/ai6/actions/workflows/test.yml)

The `ephemeral-container` Theater provider for [AI6](https://github.com/jose-pr/ai6): **a fresh
Docker or Podman container for each Mission**, made from an image, reached by the `container`
transport, and removed once the Mission is done. See the
[API header](https://github.com/jose-pr/ai6/blob/main/crates/providers/theaters/container/AGENTS.md).

## Features

- One container per Mission, labelled with its template and Mission, so a Station that
  restarts finds and adopts it again.
- Docker and Podman alike: only the `engine` setting differs.
- Extra `run` options (a network mode, a volume) from the template.

## Installation

Part of the AI6 workspace; built with it, beside the `container` transport:

```bash
cargo build -p ai6 --features cli -p ai6-transport-container -p ai6-theater-container
```

## Quick start

```toml
[theater.scratch]
access = { ephemeral = { provider = "ephemeral-container", config = { image = "fedora:44", engine = "podman" } } }
agents = "remote-hands"
workdir = "/work"
```

## Development

```bash
cargo test -p ai6-theater-container
cargo clippy -p ai6-theater-container --all-targets -- -D warnings
```

A test against a real engine runs only when `AI6_TEST_THEATER_IMAGE` names an image already
present (`AI6_TEST_ENGINE` the engine).

## License

MPL-2.0. See [LICENSE](https://github.com/jose-pr/ai6/blob/main/LICENSE).
