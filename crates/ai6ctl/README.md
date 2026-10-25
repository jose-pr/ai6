# ai6ctl

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/jose-pr/ai6/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/jose-pr/ai6/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/jose-pr/ai6/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://jose-pr.github.io/ai6/)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/ai6/test.yml)](https://github.com/jose-pr/ai6/actions/workflows/test.yml)

The AI6 operator command. **`ai6ctl` reads Stations, Agent roles, Missions and
Sessions through Universal Exports, AI6's authenticated public API** — never from a
Station directly. See the [command header](AGENTS.md).

## Features

- Read-only: health, Stations and the Headquarters Station, Operation-scoped Agent
  roles, Missions and provider Sessions.
- Uses only a user bearer token; it never accepts M's credential or a
  terminal-attachment token.
- HTTPS everywhere except loopback HTTP.

## Installation

`ai6ctl` is part of the AI6 workspace and is not published separately:

```console
cargo install --path crates/ai6ctl
```

## Quick start

```console
export AI6_EXPORTS_URL=https://exports.example
export AI6_ACCESS_TOKEN=...   # from POST /v1/auth/login
ai6ctl health
ai6ctl stations
ai6ctl station station-007
ai6ctl agent-roles --operation G
ai6ctl missions --operation G
ai6ctl mission mission-007
ai6ctl sessions --state working
ai6ctl session provider-session-007
```

## Command line

| Command | Reads |
| --- | --- |
| `health` | `GET /health` |
| `stations`, `station <STATION_ID>` | Station health, HQS status, capacity, roles and authorized Operations |
| `agent-roles --operation <ID>` | Role purpose and policy visible with `agent.read` |
| `missions [--operation <ID>]` | Visible Missions and their Session counts |
| `mission <MISSION_ID> [--operation <ID>]` | One Mission and its provider Sessions; `--operation` disambiguates a repeated ID |
| `sessions [--operation <ID>] [--mission <ID>] [--state <STATE>]` | Authorized provider Sessions, filtered |
| `session <SESSION_ID>` | One provider Session |

Every command accepts `--server <ORIGIN>`.

## Environment variables

| Variable | Purpose | Default |
| --- | --- | --- |
| `AI6_EXPORTS_URL` | Universal Exports origin; overridden by `--server`. | `http://127.0.0.1:7082` |
| `AI6_ACCESS_TOKEN` | User bearer required by every command except `health`. | None |

## Exit status

`0` on success, `1` for a connection or API error or a missing item, `2` for invalid
arguments.

## Development

```console
cargo test -p ai6ctl
cargo clippy -p ai6ctl --all-targets -- -D warnings
```

The integration test starts a loopback Universal Exports gateway in-process and runs the
built binary against it.

## License

MPL-2.0. See [LICENSE](https://github.com/jose-pr/ai6/blob/main/LICENSE).
