# ai6-transport-ssh

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/jose-pr/ai6/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/jose-pr/ai6/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/jose-pr/ai6/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://jose-pr.github.io/ai6/rust/ai6_transport_ssh/)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/ai6/test.yml)](https://github.com/jose-pr/ai6/actions/workflows/test.yml)

The `ssh` transport provider for [AI6](https://github.com/jose-pr/ai6): **the way into a
remote host over SSH.** One pure-Rust client (`russh` with `ring`) on every operating
system, with no dependency on an installed `ssh` or on `ControlMaster`. See the
[API header](https://github.com/jose-pr/ai6/blob/main/crates/providers/transports/ssh/AGENTS.md).

## Features

- Commands (`run`), terminals on a pty, SFTP files and reverse forwards.
- **Host keys are pinned** (`host_key = "sha256:HEX"`); a host that presents any other key is
  refused. First contact without a pin is refused unless `trust_on_first_use = true`, which is
  reported as a weakening.
- **Keys never live in configuration**: only the path of a private key file does.
- Reconnects on its own when the connection drops, and has the host listen again for its
  open reverse forwards, on the same ports.

## Installation

Part of the AI6 workspace; built with it:

```bash
cargo build -p ai6 --features cli -p ai6-transport-ssh
```

## Quick start

```toml
[theater.build-box]
access = { ssh = { host = "build.example.net", user = "ci", host_key = "sha256:…" } }
```

Get the pin from the host with
`awk '{print $2}' /etc/ssh/ssh_host_ed25519_key.pub | base64 -d | sha256sum`.

## Development

```bash
cargo test -p ai6-transport-ssh
cargo clippy -p ai6-transport-ssh --all-targets -- -D warnings
```

## License

MPL-2.0. See [LICENSE](https://github.com/jose-pr/ai6/blob/main/LICENSE).
