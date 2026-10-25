# ai6-transport-qemu

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/jose-pr/ai6/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/jose-pr/ai6/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/jose-pr/ai6/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://jose-pr.github.io/ai6/rust/ai6_transport_qemu/)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/ai6/test.yml)](https://github.com/jose-pr/ai6/actions/workflows/test.yml)

The `qemu` transport provider for [AI6](https://github.com/jose-pr/ai6): **the way into a QEMU
guest** through its guest agent (`qemu-ga`), which needs no network inside the guest. See the
[API header](https://github.com/jose-pr/ai6/blob/main/crates/providers/transports/qemu/AGENTS.md).

## Features

- `run` through the agent's `guest-exec`, with environment and standard input.
- `files` built from shell commands run the same way, so a POSIX guest needs an `sh`.
- Reaches the agent over a TCP chardev on any host, or a Unix socket chardev on Unix.

## Installation

Part of the AI6 workspace; built with it:

```bash
cargo build -p ai6 --features cli -p ai6-transport-qemu
```

## Quick start

Start the guest with a socket for the agent, for example
`-chardev socket,id=qga0,host=127.0.0.1,port=4555,server=on,wait=off -device virtio-serial
-device virtserialport,chardev=qga0,name=org.qemu.guest_agent.0`, then:

```toml
[theater.vm]
access = { transport = { provider = "qemu", config = { address = "127.0.0.1:4555" } } }
agents = "remote-hands"
workdir = "/root/work"
```

## Development

```bash
cargo test -p ai6-transport-qemu
cargo clippy -p ai6-transport-qemu --all-targets -- -D warnings
```

## License

MPL-2.0. See [LICENSE](https://github.com/jose-pr/ai6/blob/main/LICENSE).
