# `ai6ctl` — command header

Reference for the `ai6ctl` binary: every command, its inputs, output contract and
gotchas, so it can be scripted without reading the source. Development documentation
lives at <https://github.com/jose-pr/ai6>.

## Build

No Cargo features. MSRV: Rust 1.89. Depends on clap, reqwest (rustls), serde and Tokio.
The crate has no library target.

## Commands

All commands accept `--server <ORIGIN>` (env `UNEX_URL`), `--config <PATH>`, `--ca-file <PATH>`
(env `UNEX_CA_FILE`) and `--timeout <SECONDS>`
(default 5, fractions allowed, `0` for no limit: the time one HTTP request may take). The origin
is `--server`, else `UNEX_URL`, else `[unex] url` of the `ai6.cfg` the `ai6` command would read
(`--config`, `AI6_CONFIG`, the user's then the system's configuration directory; nothing else
of it but `ca_file` is read), else `http://127.0.0.1:7082`. The origin must have no credentials or path; HTTP is accepted
only for loopback, HTTPS is required elsewhere. Redirects are not followed. The public roots are
trusted, and besides them the PEM authorities of `--ca-file`, else `UNEX_CA_FILE`, else `[unex]
ca_file` of that `ai6.cfg`; a file that cannot be read or holds no usable certificate is refused
before any request, naming the path. An `ai6.cfg` that cannot be read when `--server` gave the
origin leaves the public roots alone.

- `ai6ctl health` checks `GET /health` and prints `Universal Exports is healthy`.
- `ai6ctl stations` lists visible Station health, HQS status, running/capacity, roles,
  authorized Operations and last check-in from `GET /v1/stations`, then the host's
  resources: header `STATION\tHEALTH\tHQS\tRUNNING/CAPACITY\tROLES\tOPERATIONS\tSEEN_UNIX\t
  CPU_PERCENT\tCPUS\tLOAD_1M\tMEM_FREE_MB\tMEM_TOTAL_MB\tSAMPLED_UNIX` (one line, no
  spaces); a figure the Station did not report prints `unknown`.
- `ai6ctl station <STATION_ID>` prints one visible Station row (no header); exit `1` if absent.
- `ai6ctl agents --operation <OPERATION_ID>` prints `GET /v1/user/operations/{id}/agents`
  as tab-separated `AGENT POSTED DONE FAILED BLOWN ABANDONED STALLED LOST QUARANTINED BUDGET_STOPS
  GREEN_INK_REJECTED LAST_MISSION_UNIX MODEL RETIREMENT CLEARANCE PROMOTION HOLDS` (one header line):
  counts in that Operation (`LOST`: Sessions lost with their Station or runner, never
  failures), `QUARANTINED` adding quarantines of every Operation, `LAST_MISSION_UNIX` `-`
  before any, `MODEL` the current occupant (`default` for the handler's own, `-` before any
  record), `RETIREMENT` `status:YYYY-MM-DD` (`retiring`, `desk_duty`, `retired`) or `-`,
  `CLEARANCE` the clearance it works at there (`field(configured:desk)` when a grant moved it,
  `-` unknown) and `PROMOTION` `REQUEST_ID:AGENT:CLEARANCE@OPERATION` while M's promotion
  request waits for green ink, else `-`. A refusal names `agent.read`.
- `HOLDS` of `ai6ctl agents` lists the holds that keep the Agent (`agent`) or a route
  (`HANDLER/MODEL/POOL`, `-` unset) out of use, each `WHAT:BY`, comma-separated; `-` for none. A
  route hold needs `roster.read` to be shown. `ai6ctl agents lift <AGENT> [--handler H [--model M]
  [--pool P]] --operation <ID>` deletes `.../agents/{agent}/hold` (the whole Agent's hold, or that
  route's) and prints `AGENT: hold by WHO lifted in ID: REASON` (`404`, no such hold, exits 1);
  needs `agent.configure`, audited as `agent.hold.lifted`.
- `ai6ctl models` prints `GET /v1/user/models` as tab-separated `STATION SOURCE HANDLER KNOWN MODELS
  UNKNOWN_BECAUSE`: `MODELS` the ids comma-separated (`-` for none), `UNKNOWN_BECAUSE` why a handler
  could not list (`-` when it did). Needs `roster.read`.
- `ai6ctl agents scorecard --operation <ID> [--window-days N] [--show JUDGMENT]` prints
  `GET .../agents/scorecard` as a `#` line (the window and small-sample line) and tab-separated
  `AGENT ROUTE JUDGED FAILURE_RATE SAMPLE` then one column per judgment (`DONE FAILED BLOWN STALLED
  WAITING_ON_PERSON BUDGET_STOP PARKED LOST HANDED_OFF WITHDRAWN RUNNING UNRECORDED`) and
  `REFUSED_TOOLS TOKENS COST_USD WALL_SECS`: per Agent (`ROUTE` `all`), per route (`HANDLER MODEL
  POOL`) and per model (`AGENT` `(model)`). `SAMPLE` is `SMALL` under the Operation's line. `--show`
  adds one line per attempt behind that judgment (`MISSION`, `SESSION`). Needs `agent.read` and
  `roster.read`.
- `ai6ctl agents lower <AGENT> <CLEARANCE> --reason <TEXT> [--expires-in-secs N] --operation
  <ID>` puts `.../agents/{agent}/clearance` and prints `AGENT: held at CLEARANCE in ID`;
  `ai6ctl agents restore <AGENT> --operation <ID>` deletes it and prints `AGENT: grant to
  CLEARANCE removed; configured clearance applies in ID` (404, no grant, exits 1). Both need
  `agent.configure`. Raising a clearance is signing M's promotion request with `ai6ctl
  green-ink approve` (which then also needs `agent.configure`).
- `ai6ctl agent-roles --operation <OPERATION_ID>` lists role purpose, policy and whether
  the Operation overrides the Station definition (`agent.read` on that Operation). The
  Operation ID is encoded as one URL path segment.
- `ai6ctl pages list` prints `GET /v1/user/pages` as tab-separated `ID KIND BYTES EXPIRES_UNIX
  TITLE SHARED_BY` (`EXPIRES_UNIX` is `remembered` without an expiry; `SHARED_BY` is `-` for
  your own pages, else whose the shared page is);
  `pages open <ID> [--network none|own]` prints the address with a hand-off that
  `POST /v1/user/pages/{id}/open` gives (single-use, seconds long: open it at once in a browser);
  `pages remember <ID>` and `pages forget <ID>`
  post `.../remember`; `pages share <ID> [SUBJECT...]` puts `.../shares` (`person:`, `group:`,
  `account:`, `compartment:` or `operation:` and an ID; none shares with nobody) and prints
  `ID: shared with SUBJECT...` or `ID: shared with nobody`; `pages public <ID> on|off` puts
  `.../public` and prints `ID: public at URL` or `ID: no public link`; `pages rm <ID>` deletes. The ID is encoded as one path segment; 404 reads "no such page of yours" (another
  person's page too); a refusal with a reason prints the gateway's reason.
- `ai6ctl pools --operation <OPERATION_ID>` prints `GET /v1/user/operations/{id}/pools`
  as tab-separated `POOL ACCOUNT KIND SESSIONS WINDOWS LIMITED_UNTIL_UNIX TOKENS_TODAY`:
  `SESSIONS` is `active/max_concurrent` (or the count without a limit), `WINDOWS`
  `name=42.5%` per window (`name=?` without a figure) or `-`, `LIMITED_UNTIL_UNIX` `-`
  unless in the future; `SESSIONS` counts other Operations' Sessions too. A refusal names
  `pool.read`.
- `ai6ctl pools clear-limit <POOL> <ACCOUNT> --operation <OPERATION_ID>` posts
  `.../pools/{pool}/{account}/clear-limit` and prints `pool/account: limit cleared`; 403 names
  `pool.manage`, 404 an account not declared for the Operation.
- `ai6ctl missions [--operation <ID> [--subtree]]` lists visible Missions as `STATION MISSION
  SESSIONS NAME UID`: `MISSION` the handle (the ID when the gateway gives none), `NAME` the name
  beside it (`-` before a Station published one; an issue's title, as data) and `UID` the
  Mission's ID; `--operation` asks the gateway for that Operation's (`GET
  /v1/missions?operation=`), and `--subtree` also for every Operation under it
  (`&subtree=true`).
- Wherever a command takes a Mission (`<MISSION_ID>`, `<MISSION>`, `--mission`), it takes its UID
  or any handle it had, compared without case; the gateway looks it up (`GET
  /v1/missions?mission=`), so a handle from before a rename still finds it.
- `ai6ctl missions verdict <MISSION> <confirmed|disputed|failed-verification> [--note <TEXT>]
  [--operation <ID>]` (`mission.verify`) posts `.../verdict` and prints `MISSION attempt N: VERDICT by
  PERSON`; a refusal prints the gateway's words. `ai6ctl mission <ID>` also prints a `VERDICT` line under
  a run and, from the Mission's detail, a `FINAL REPORT`, the latest attempt's `DELIVERABLES` and its
  `EVIDENCE` (`DELIVERABLE RESULT DETAIL`).
- `ai6ctl missions probe <MISSION> [--wait <SECS>] [--operation <ID>]` (`mission.verify`, default wait
  60 s) posts `.../probe`, waits for the Station to answer (`probe_requested` clears in the Mission's
  detail) and prints the findings made since; at the end of the wait it says the Station has not
  answered. `ai6ctl missions verify <MISSION> <done|redispatch|failed> --reason <TEXT> [--amend <TEXT>]
  [--operation <ID>]` posts `.../verify` and prints `MISSION attempt N: DECISION by PERSON`; `--reason` is
  required, `--amend` only with `redispatch`. `ai6ctl missions create` takes `--deliverable KIND[:TARGET]`,
  repeatable.
- `ai6ctl missions rename <MISSION> <HANDLE> [--operation <ID>]` (`mission.control`) posts
  `.../missions/{mission}/rename` and prints `UID: OLD is now NEW`; a refusal prints the
  gateway's words and exits `1`. The Operation is looked up when not given.
- `ai6ctl missions create --operation <ID> --title <T> --prompt <P> [--agent A] [--role R]
  [--tag T]... [--station S]... [--issue TRACKER:ITEM | --create-issue TRACKER [--issue-title T]
  [--issue-body B]]` (`mission.create`; `--create-issue` also `issue.create`) posts
  `.../operations/{op}/missions` and prints `HANDLE (UID) queued`, or for `--create-issue`
  `waiting for its ticket: issue proposal ID`. `--issue` that is not `TRACKER:ITEM` is refused
  before anything is sent.
- `ai6ctl operations` reads `GET /v1/user/profile` and prints `OPERATION\tACTIONS`, each
  Operation indented two spaces under its `parent` (the nearest one the person may read; a
  hidden ancestor is never named), roots and siblings in ID order.
- `ai6ctl operations show <ID> [--effective]` prints `operation`, `parent` (`-` for none) and
  `actions`; `--effective` also reads `GET /v1/user/roster/operation/{id}/effective` (needs
  `roster.read`) and prints `lineage` (`A > B`), `inherit` and each written key as
  `key = <JSON value>\town` or `\tfrom <Operation>`. An Operation not in the profile is an
  error.
- `ai6ctl access show [--operation X]` prints the policy (`GET /v1/user/access` or the
  Operation's): `revision`, `role`, `rule N`, `delegable`, `chief stored`, `chief host` (and
  `inherited`, `binding`, `orphaned` for an Operation). `access role set NAME --actions a,b
  [--description T] [--operation X]` and `access role remove NAME` read the revision, then write
  with it. `access rules export FILE` writes `revision = "…"` and `[[rule]]` entries; `access
  rules import FILE` sends the whole list under that revision and fails when the policy changed
  since. `access delegable set a,b`, `access chief set SUBJECT…` (chief only) and `access check
  SUBJECT ACTION RESOURCE` (prints `allowed|denied`, tier, rule and role). A refusal prints the
  gateway's reason and exits `1`.
- `ai6ctl accounts [list]`, `create NAME [--groups g,h]`, `disable NAME`, `enable NAME`, `groups
  NAME g,h` and `reset-password NAME` use `/v1/user/accounts`; a generated password is printed
  once as `password\t…`, to stdout only. Accounts are never deleted through the API.
- `ai6ctl mission <MISSION_ID> [--operation <ID>]` shows a Mission (the `missions` row), its
  runs (`RUN STATUS ROUTE`, `RUN` the run's handle, else its UID) and its provider Sessions
  (`SESSION STATE REASON SEQUENCE EXPOSURE`; `REASON` is the lifecycle reason the Station
  published, such as `permission-denied`, or `-`); `--operation` disambiguates a Mission ID
  used by several Operations.
- `ai6ctl sessions [--operation <ID>] [--mission <ID>] [--state <STATE>]` filters the
  authorized `GET /v1/board` response.
- `ai6ctl session <SESSION_ID>` prints one provider Session.

Session listings end with an `EXPOSURE` column: the untrusted input the Session read,
comma separated, or `-`. A Session state is free text from the board and includes
`queued`, `parked`, `quarantined` and `abandoned`.

Mission lists include only Missions with a provider Session registered at the gateway.

- `ai6ctl missions --compartment <ID>` keeps what that compartment covers;
  `ai6ctl missions products <MISSION_ID>` prints a Mission's outcome, CX report and refs.

### Compartments

- `ai6ctl reviews show --operation OP MISSION` prints each gate's review: `GATE POLICY STATUS iteration round sent back turns tokens`, then `why`, each seat (`seat N AGENT weight W [veto] MODEL`), each vote (`ITERATION.ROUND AGENT VOTE weight W MODEL REASONS`) and an overrule. `ai6ctl reviews overrule --operation OP MISSION [--gate output|plan] --decision pass|redo|refuse --reason TEXT` needs `mission.review`.
- `ai6ctl compartments` lists what the person may read: `COMPARTMENT ACTIONS COVERS READ_IN`.
- `ai6ctl compartments show <ID>`.
- `ai6ctl compartments set <ID> [--description TEXT] [--actions a,b] [--min-clearance LEVEL]
  [--max-fanout N|none]` makes one or changes it; what is not named stays.
- `ai6ctl compartments cover <ID> operation:<ID>|mission:<ID> [--no-propagate]`, `uncover`.
- `ai6ctl compartments read-in <ID> <SUBJECT> [--no-propagate]`, `read-out`.
- `ai6ctl compartments remove <ID>`.

A refusal prints the gateway's reason and exits 1.

### Memories

- `ai6ctl memories search [QUERY] [--scope person:ID|operation:ID|global]`: `ID SCOPE REVISION
  SOURCE SUMMARY`, best first, then `(N of TOTAL)` when cut; with no query, the newest of the scope.
  SOURCE is who wrote it, with `(exposed: …)` when an exposed Session did.
- `ai6ctl memories show <ID>`: the fields, then the content.
- `ai6ctl memories add <SCOPE> --summary TEXT --content TEXT [--name N] [--compartments a,b]`,
  `edit <ID> --revision N --summary TEXT --content TEXT [--name N] [--compartments a,b]`: prints
  `ID REVISION`, or the proposal when the caller may only propose.
- `ai6ctl memories forget <ID> [--revision N]`.
- `ai6ctl memories proposals [--scope SCOPE]`, `decide <ID> [--reject]`.

What the person may not see answers as not there (`404`).

### Controls

Each control needs `mission.control` on the Mission's Operation. It posts
`POST /v1/user/operations/{operation}/missions/{mission}/actions`, then polls the
control's status route every `--poll-interval` (0.5 s) until the Station reports or `--wait` runs out.

- `ai6ctl mission cancel <MISSION_ID>` stops the live Session, if any, and abandons the
  Mission; one no Station has taken yet is withdrawn, and one that already ended stays as it is, and
  the control comes back `applied`.
- `ai6ctl mission retry <MISSION_ID> [--agent <CODENAME>]` queues a stalled or finished
  Mission again as a new attempt, for `--agent` when given, else for the Agent the roster gives
  it now.
- `ai6ctl mission withdraw <MISSION_ID> [--note <TEXT>] [--operation <ID>]` posts
  `POST /v1/user/operations/{operation}/missions/{mission}/withdraw` and prints
  `MISSION STATE REASON`: a Mission whose brief is queued or held is filed as failed with who
  withdrew it and the note. HTTP 404 (none queued or held) and 409 (a Station took it, or it ended:
  use `mission cancel`) fail the command with that said.
- `ai6ctl mission resume <MISSION_ID> [--message <TEXT>]` resumes a parked, blocked or
  stalled Mission now, sending `--message` to the Agent.
- `ai6ctl session interrupt <SESSION_ID>` stops the Session's current turn; it finds the
  Session's Operation and Mission on the board and names the Session, so the request is
  refused (HTTP 409) if the Mission has moved on to another one.

Shared options: `--operation <ID>` (Mission commands only; found from the board when
omitted, and required for a Mission with no registered Session), `--idempotency-key
<KEY>` (default: unique per invocation; repeating a key returns the first request's
control) `--wait <SECONDS>` (default `10`; `0` returns once queued) and `--poll-interval <SECONDS>` (default `0.5`).

Output: a header `CONTROL\tACTION\tMISSION\tSTATION\tSTATUS\tSTATE\tREASON` and one row;
`STATUS` is `queued`, `delivered`, `applied`, `rejected` or `expired`, `STATE` the
Mission state the Station reported and `REASON` why it rejected; `-` for empty values.
Exit `1` when the control was rejected or expired, or the request was refused.

### Green ink and class authorisations

`--operation <ID>` is required; IDs are encoded as single path segments. Every decision is
signed by the token's person and audited with the step, never the Agent's reason.

- `ai6ctl green-ink list --operation <ID> --expired` (`approval.decide`): the requests that
  expired undecided, the newest first, from `GET .../green-ink/expired`. Header
  `REQUEST\tMISSION\tAGENT\tSTEP\tTARGET\tEXPIRED_UNIX\tREASON`. `ai6ctl green-ink rearm <REQUEST_ID>
  --operation <ID> [--expires-in <SECONDS>] [--note <TEXT>]` opens one again and prints it as
  `list` does; `ai6ctl green-ink revoke` takes `--reason <TEXT>`, kept on the record.
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

### Held briefs

- `ai6ctl mission issues <MISSION> --operation <ID>` (`mission.read`): from `GET
  .../missions/<MISSION>/issues`, tables `ISSUE\tRELATION\tSTATE\tPHASE\tQUESTION\tURL` and
  `ENTRY\tKIND\tISSUE\tSTATE\tURL_OR_REASON`; never an issue's text.
- `ai6ctl issues link|unlink|accept|comment|update <TRACKER:ITEM> --mission <ID> --operation <ID>`
  and `ai6ctl issues create <TRACKER> --operation <ID> --title <TEXT> [--body] [--label]...
  [--issue-type] [--priority] [--parent] [--mission]`: the routes of the same names; prints the
  issue or proposal and its state.
- `ai6ctl issue-proposals list --operation <ID>` (`mission.read`): header
  `ID\tSTATE\tPROPOSER\tMISSION\tISSUES\tFINDINGS\tDROPPED`, never their text.
  `ai6ctl issue-proposals approve <ID> --operation <ID> [--mark-ready]` and `reject <ID> --operation
  <ID> [--reason <TEXT>]` (`issue.create`; `--mark-ready` also `mission.create`).
- `ai6ctl held-briefs list --operation <ID>` (`mission.create`): the briefs vetting held,
  oldest first, from `GET .../held-briefs`. Header `KEY\tSOURCE\tTITLE\tHELD_UNIX\tFINDINGS`;
  findings are `kind: detail (related)` joined by `; `. The brief's body is never printed;
  its source and title show each character that renders as nothing by code (`<U+200B>`) and
  a run of tag characters as the text it spells (`<tags: ...>`).
- `ai6ctl held-briefs release <KEY> --operation <ID>` and `ai6ctl held-briefs refuse <KEY>
  --operation <ID> [--reason <TEXT>]` print `KEY\tSTATE` (`queued` or `refused`); repeating
  one prints the same, a key the Operation neither holds nor decided is HTTP 404, and a brief
  already placed or finished is HTTP 409.

## Environment variables

| Variable | Effect | Default |
| --- | --- | --- |
| `UNEX_URL` | Universal Exports origin; overridden by `--server`. | `[unex] url` of `ai6.cfg`, else `http://127.0.0.1:7082` |
| `UNEX_CA_FILE` | PEM certificate authorities trusted besides the public roots; overridden by `--ca-file`. | `[unex] ca_file` of `ai6.cfg`, else the public roots alone |
| `AI6_CONFIG` | The `ai6.cfg` to read `[unex] url` and `ca_file` from; must exist when set. | The search of the `ai6` command |
| `UNEX_ACCESS_TOKEN` | User bearer; required by every command except `health`. | None |

## Exit status

`0` on success, `1` for connection/API errors (the message names the permission a `403`
needs), a missing item or a rejected or expired control, `2` for invalid arguments.

## Gotchas

- The only writes are the controls, green ink decisions and access and account changes above; no flag accepts the
  Station channel bearer or a terminal-attachment token.
- `mission <ID>` and `session <ID>` still show; a Mission or Session literally named
  `cancel`, `retry`, `resume` or `interrupt` is shown with `ai6ctl missions` or
  `ai6ctl sessions` instead.
- A control that times out under `--wait` exits `0` with its open status; the Station may
  still apply it.
- The access token is read only from the environment, never from an argument, so it
  does not appear in shell history or process listings.
