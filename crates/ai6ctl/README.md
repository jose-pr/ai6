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
- Signs green ink: lists the steps Agents wait to take, approves, rejects or revokes them,
  and lists or revokes class authorisations.
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
export UNEX_URL=https://exports.example   # or [unex] url in ai6.cfg
export UNEX_ACCESS_TOKEN=...              # from POST /v1/auth/login
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
ai6ctl green-ink list --operation G
ai6ctl green-ink approve green-ink-5c1e... --operation G --expires-in 1800 --note "main only"
ai6ctl green-ink reject green-ink-9a02... --operation G --reason "not before the audit"
ai6ctl class-authorizations list --operation G
ai6ctl held-briefs list --operation G
ai6ctl held-briefs release source_jira--OPS-12 --operation G
```

## Command line

| Command | Does |
| --- | --- |
| `health` | `GET /health` |
| `stations`, `station <STATION_ID>` | Station health, HQS status, capacity, roles, authorized Operations, and the host's CPU, load and memory with their sample time (`unknown` when not reported) |
| `agent-roles --operation <ID>` | Role purpose and policy visible with `agent.read` |
| `pools --operation <ID>` | Pool accounts the Operation may draw from: Sessions, quota used, limited until and tokens today (`pool.read`) |
| `pools clear-limit <POOL> <ACCOUNT> --operation <ID>` | Lifts an account's provider limit (`pool.manage`, audited) |
| `missions [--operation <ID>]` | Visible Missions and their Session counts |
| `mission <MISSION_ID> [--operation <ID>]` | One Mission and its provider Sessions; `--operation` disambiguates a repeated ID |
| `sessions [--operation <ID>] [--mission <ID>] [--state <STATE>]` | Authorized provider Sessions, filtered |
| `session <SESSION_ID>` | One provider Session |
| `mission cancel <MISSION_ID>` | Stops the live Session, if any, and abandons the Mission |
| `mission retry <MISSION_ID> [--agent <CODENAME>]` | Queues a stalled or finished Mission again as a new attempt |
| `mission resume <MISSION_ID> [--message <TEXT>]` | Resumes a parked, blocked or stalled Mission now |
| `session interrupt <SESSION_ID>` | Stops the Session's current turn; its Mission waits, blocked |
| `green-ink list --operation <ID>` | The steps Agents wait to take with green ink (`approval.decide`) |
| `green-ink approve <REQUEST_ID> --operation <ID> [--expires-in <SECONDS>] [--note <TEXT>]` | Signs a waiting step as you, for an hour unless chosen |
| `green-ink reject <REQUEST_ID> --operation <ID> [--reason <TEXT>]` | Refuses a waiting step; the Agent is told the reason |
| `green-ink revoke <REQUEST_ID> --operation <ID>` | Withdraws an approval before it expires |
| `class-authorizations list\|revoke [<ID>] --operation <ID>` | Standing approvals of a kind of step (`approval.class`) |
| `held-briefs list\|release\|refuse [<KEY>] --operation <ID> [--reason <TEXT>]` | Briefs vetting held, with what it found; release one to the queue or refuse it (`mission.create`) |

Every command accepts `--server <ORIGIN>` and `--timeout <SECONDS>` (default 5, fractions allowed;
`0` waits without limit), the time one HTTP request may take. The control commands need `mission.control`
on the Mission's Operation, take `--operation <ID>` (found from the board when omitted),
`--idempotency-key <KEY>`, `--wait <SECONDS>` (default 10; `0` returns once queued) and
`--poll-interval <SECONDS>` (default 0.5, how often they ask for the status while waiting),
and print the control's `STATUS`, the Mission's `STATE` and any `REASON`.

## Environment variables

| Variable | Purpose | Default |
| --- | --- | --- |
| `UNEX_URL` | Universal Exports origin; overridden by `--server`. | `[unex] url` of `ai6.cfg`, else `http://127.0.0.1:7082` |
| `AI6_CONFIG` | The `ai6.cfg` to read `[unex] url` from; must exist when set. | `ai6.cfg` in the user's, then the system's configuration directory |
| `UNEX_ACCESS_TOKEN` | User bearer required by every command except `health`. | None |

## Exit status

`0` on success, `1` for a connection or API error, a missing item, or a control the
Station rejected or no Station took in time, `2` for invalid arguments.

## Development

```console
cargo test -p ai6ctl
cargo clippy -p ai6ctl --all-targets -- -D warnings
```

The integration tests start a loopback Universal Exports gateway in-process, play a
Station that takes and answers controls, seed green ink requests and a class authorisation
in its Registry, and run the built binary against them.

## License

MPL-2.0. See [LICENSE](https://github.com/jose-pr/ai6/blob/main/LICENSE).
