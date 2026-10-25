# ai6ctl

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/jose-pr/ai6/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/jose-pr/ai6/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/jose-pr/ai6/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://jose-pr.github.io/ai6/)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/ai6/test.yml)](https://github.com/jose-pr/ai6/actions/workflows/test.yml)

The AI6 operator command. **`ai6ctl` reads and controls Stations, Agent roles, Missions
and Sessions through Universal Exports, AI6's authenticated public API** — never on a
Station directly. See the [command header](AGENTS.md).

## Features

- Reads health, Stations and the Headquarters Station, Operation-scoped Agent roles,
  Missions and provider Sessions.
- Cancels, retries and resumes Missions and interrupts Sessions; the Station holding the
  Mission decides and reports, and `ai6ctl` waits for its outcome.
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
ai6ctl session interrupt provider-session-007
ai6ctl mission resume mission-007 --message "The review is unblocked; carry on."
ai6ctl mission cancel mission-007
ai6ctl mission retry mission-007 --agent 008
```

## Command line

| Command | Does |
| --- | --- |
| `health` | `GET /health` |
| `stations`, `station <STATION_ID>` | Station health, HQS status, capacity, roles and authorized Operations |
| `agent-roles --operation <ID>` | Role purpose and policy visible with `agent.read` |
| `missions [--operation <ID>]` | Visible Missions and their Session counts |
| `mission <MISSION_ID> [--operation <ID>]` | One Mission and its provider Sessions; `--operation` disambiguates a repeated ID |
| `sessions [--operation <ID>] [--mission <ID>] [--state <STATE>]` | Authorized provider Sessions, filtered |
| `session <SESSION_ID>` | One provider Session |
| `mission cancel <MISSION_ID>` | Stops the live Session, if any, and abandons the Mission |
| `mission retry <MISSION_ID> [--agent <CODENAME>]` | Queues a stalled or finished Mission again as a new attempt |
| `mission resume <MISSION_ID> [--message <TEXT>]` | Resumes a parked, blocked or stalled Mission now |
| `session interrupt <SESSION_ID>` | Stops the Session's current turn; its Mission waits, blocked |

Every command accepts `--server <ORIGIN>`. The control commands need `mission.control`
on the Mission's Operation, take `--operation <ID>` (found from the board when omitted),
`--idempotency-key <KEY>` and `--wait <SECONDS>` (default 10; `0` returns once queued),
and print the control's `STATUS`, the Mission's `STATE` and any `REASON`.

## Environment variables

| Variable | Purpose | Default |
| --- | --- | --- |
| `AI6_EXPORTS_URL` | Universal Exports origin; overridden by `--server`. | `http://127.0.0.1:7082` |
| `AI6_ACCESS_TOKEN` | User bearer required by every command except `health`. | None |

## Exit status

`0` on success, `1` for a connection or API error, a missing item, or a control the
Station rejected or no Station took in time, `2` for invalid arguments.

## Development

```console
cargo test -p ai6ctl
cargo clippy -p ai6ctl --all-targets -- -D warnings
```

The integration test starts a loopback Universal Exports gateway in-process, plays a
Station that takes and answers controls, and runs the built binary against both.

## License

MPL-2.0. See [LICENSE](https://github.com/jose-pr/ai6/blob/main/LICENSE).
