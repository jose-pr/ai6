# `ai6ctl` — command header

Reference for the `ai6ctl` binary: every command, its inputs, output contract and
gotchas, so it can be scripted without reading the source. Development documentation
lives at <https://github.com/jose-pr/ai6>.

## Build

No Cargo features. MSRV: Rust 1.89. Depends on clap, reqwest (rustls), serde and Tokio.
The crate has no library target.

## Commands

All commands accept `--server <ORIGIN>` (env `AI6_EXPORTS_URL`, default
`http://127.0.0.1:7082`). The origin must have no credentials or path; HTTP is accepted
only for loopback, HTTPS is required elsewhere. Redirects are not followed.

- `ai6ctl health` checks `GET /health` and prints `Universal Exports is healthy`.
- `ai6ctl stations` lists visible Station health, HQS status, running/capacity, roles,
  authorized Operations and last check-in from `GET /v1/stations`.
- `ai6ctl station <STATION_ID>` prints one visible Station; exit `1` if absent.
- `ai6ctl agent-roles --operation <OPERATION_ID>` lists role purpose, policy and whether
  the Operation overrides the Station definition (`agent.read` on that Operation). The
  Operation ID is encoded as one URL path segment.
- `ai6ctl missions [--operation <ID>]` lists visible Missions and Session counts.
- `ai6ctl mission <MISSION_ID> [--operation <ID>]` shows a Mission and its provider
  Sessions; `--operation` disambiguates a Mission ID used by several Operations.
- `ai6ctl sessions [--operation <ID>] [--mission <ID>] [--state <STATE>]` filters the
  authorized `GET /v1/board` response.
- `ai6ctl session <SESSION_ID>` prints one provider Session.

Session listings end with an `EXPOSURE` column: the untrusted input the Session read,
comma separated, or `-`. A Session state is free text from the board and includes
`parked` and `quarantined`.

Mission lists include only Missions with a provider Session registered at the gateway.

## Environment variables

| Variable | Effect | Default |
| --- | --- | --- |
| `AI6_EXPORTS_URL` | Universal Exports origin; overridden by `--server`. | `http://127.0.0.1:7082` |
| `AI6_ACCESS_TOKEN` | User bearer; required by every command except `health`. | None |

## Exit status

`0` on success, `1` for connection/API errors or a missing item, `2` for invalid
arguments.

## Gotchas

- Read-only by design: there is no command that writes, and no flag accepts the Station channel bearer
  or a terminal-attachment token.
- The access token is read only from the environment, never from an argument, so it
  does not appear in shell history or process listings.
