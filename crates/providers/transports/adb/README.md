# ai6-transport-adb

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/jose-pr/ai6/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/jose-pr/ai6/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/jose-pr/ai6/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://jose-pr.github.io/ai6/rust/ai6_transport_adb/)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/ai6/test.yml)](https://github.com/jose-pr/ai6/actions/workflows/test.yml)

The `adb` transport provider for [AI6](https://github.com/jose-pr/ai6): **the way into an
Android device** through the `adb` command line. An Android device cannot run an agent
harness, so a Theater on it works in remote-hands mode: the harness runs on the Station and
acts on the device through this transport. See the
[API header](https://github.com/jose-pr/ai6/blob/main/crates/providers/transports/adb/AGENTS.md).

## Features

- `run` (through `adb shell`), `files` (built from shell commands, so no `push` or `pull`
  and no temporary files) and `forward` (`adb reverse`, to the Station's loopback).
- No terminal: an agent has no use for one on a device.
- Needs only the `adb` executable and a device that `adb devices` lists.

## Installation

Part of the AI6 workspace; built with it:

```bash
cargo build -p ai6 --features cli -p ai6-transport-adb
```

## Quick start

```toml
[theater.phone]
access = { transport = { provider = "adb", config = { serial = "emulator-5554" } } }
agents = "remote-hands"
workdir = "/data/local/tmp/work"
```

## Development

```bash
cargo test -p ai6-transport-adb
cargo clippy -p ai6-transport-adb --all-targets -- -D warnings
```

A test against a real device runs only when `AI6_TEST_ADB_SERIAL` names one.

## License

MPL-2.0. See [LICENSE](https://github.com/jose-pr/ai6/blob/main/LICENSE).
