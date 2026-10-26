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
`queued`, `parked`, `quarantined` and `abandoned`.

Mission lists include only Missions with a provider Session registered at the gateway.

### Controls

Each control needs `mission.control` on the Mission's Operation. It posts
`POST /v1/user/operations/{operation}/missions/{mission}/actions`, then polls the
control's status route every 500 ms until the Station reports or `--wait` runs out.

- `ai6ctl mission cancel <MISSION_ID>` stops the live Session, if any, and abandons the
  Mission.
- `ai6ctl mission retry <MISSION_ID> [--agent <CODENAME>]` queues a stalled or finished
  Mission again as a new attempt, for `--agent` when given.
- `ai6ctl mission resume <MISSION_ID> [--message <TEXT>]` resumes a parked, blocked or
  stalled Mission now, sending `--message` to the Agent.
- `ai6ctl session interrupt <SESSION_ID>` stops the Session's current turn; it finds the
  Session's Operation and Mission on the board and names the Session, so the request is
  refused (HTTP 409) if the Mission has moved on to another one.

Shared options: `--operation <ID>` (Mission commands only; found from the board when
omitted, and required for a Mission with no registered Session), `--idempotency-key
<KEY>` (default: unique per invocation; repeating a key returns the first request's
control) and `--wait <SECONDS>` (default `10`; `0` returns once queued).

Output: a header `CONTROL\tACTION\tMISSION\tSTATION\tSTATUS\tSTATE\tREASON` and one row;
`STATUS` is `queued`, `delivered`, `applied`, `rejected` or `expired`, `STATE` the
Mission state the Station reported and `REASON` why it rejected; `-` for empty values.
Exit `1` when the control was rejected or expired, or the request was refused.

### Green ink and class authorisations

`--operation <ID>` is required; IDs are encoded as single path segments. Every decision is
signed by the token's person and audited with the step, never the Agent's reason.

- `ai6ctl green-ink list --operation <ID>` (`approval.decide`): the steps waiting for a
  decision, oldest first, from `GET .../green-ink`. Header
  `REQUEST\tMISSION\tAGENT\tSTEP\tTARGET\tEXPIRES_UNIX\tREASON`; the reason is the Agent's
  untrusted text.
- `ai6ctl green-ink approve <REQUEST_ID> --operation <ID> [--expires-in <SECONDS>] [--note
  <TEXT>]` (default one hour, at most a year), `ai6ctl green-ink reject <REQUEST_ID>
  --operation <ID> [--reason <TEXT>]` and `ai6ctl green-ink revoke <REQUEST_ID> --operation
  <ID>` (withdraws an approval; the Station then revokes a credential leased against it)
  print `REQUEST\tSTEP\tTARGET\tDECISION\tSIGNER\tEXPIRES_UNIX\tREVOKED_BY`. A request
  already decided another way, or expired, is HTTP 409; revoking an undecided one is 404.
- `ai6ctl class-authorizations list --operation <ID>` (`approval.class`) and
  `ai6ctl class-authorizations revoke <ID> --operation <ID>` print
  `CLASS\tKINDS\tTARGET\tAGENTS\tROLES\tCLEARANCE\tUSES\tEXPIRES_UNIX\tSTATE`; `USES` is
  `uses/max` or `uses`, `STATE` `live`, `used up`, `expired` or `revoked`. Granting one is
  API only (`POST .../class-authorizations`).

## Environment variables

| Variable | Effect | Default |
| --- | --- | --- |
| `AI6_EXPORTS_URL` | Universal Exports origin; overridden by `--server`. | `http://127.0.0.1:7082` |
| `AI6_ACCESS_TOKEN` | User bearer; required by every command except `health`. | None |

## Exit status

`0` on success, `1` for connection/API errors (the message names the permission a `403`
needs), a missing item or a rejected or expired control, `2` for invalid arguments.

## Gotchas

- The only writes are the controls and green ink decisions above; no flag accepts the
  Station channel bearer or a terminal-attachment token.
- `mission <ID>` and `session <ID>` still show; a Mission or Session literally named
  `cancel`, `retry`, `resume` or `interrupt` is shown with `ai6ctl missions` or
  `ai6ctl sessions` instead.
- A control that times out under `--wait` exits `0` with its open status; the Station may
  still apply it.
- The access token is read only from the environment, never from an argument, so it
  does not appear in shell history or process listings.
