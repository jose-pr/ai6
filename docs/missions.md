# Missions and intake

AI6 does not bundle tracker integrations. A user or external integration supplies a
Mission brief through the Universal Exports API or the file-backed Registry. The Rust
daemon validates the brief, resolves its Operation and Agent policy, and schedules the
Mission at an eligible Station.

```text
source ─► brief ─► Registry ─► AI6 Station ─► AgentHandler ─► provider Session
                                                │
                                                └─► outcome and DipTels to Universal Exports
```

## Mission and Session identities

A **Mission ID** identifies one unit of work and may group multiple agent Sessions. A
**Session ID** comes from the provider and remains stable when AI6 resumes that same
agent operation. An external tracker task ID is input metadata; it is not an AI6 Mission
ID.

```json
{
  "title": "Fix the flaky watchdog test",
  "prompt": "It fails one run in ten on macOS.",
  "operation_id": "G"
}
```

## Queue a Mission through Universal Exports

An authorized user needs `mission.create` for the target Operation. The gateway returns
the queued Mission ID without echoing the prompt:

```bash
curl -f -X POST "$AI6_EXPORTS_URL/v1/user/operations/G/missions" \
  -H "authorization: Bearer $AI6_ACCESS_TOKEN" \
  -H 'content-type: application/json' \
  -d '{"title":"Fix the flaky watchdog test","prompt":"Investigate the intermittent failure."}'
```

The Moneypenny app's Operations view offers the same action. The AI6 daemon claims the brief
from the shared file-backed Registry, then resolves the Agent role, Operation
cover, Station capacity and provider handler before it starts a Session.

## Cancel, retry, resume and interrupt

An operator with `mission.control` on the Operation can act on a Mission without reaching
its Station:

| Action | Accepted when the Mission is | Effect |
| --- | --- | --- |
| `cancel` | queued, working, blocked, stalled, parked or quarantined | The live Session is interrupted and closed; the Mission is `abandoned` and its safe house is finished with the work kept. |
| `retry` | stalled, done, failed, blown or abandoned | The Mission is `queued` again as a new attempt, for `agent` when one is named (configured for the Operation, qualified for the Mission's role and not quarantined); it continues in the same safe house. |
| `resume` | parked, blocked or stalled | The Mission resumes now on its provider Session, with `message` sent to the Agent when given. |
| `interrupt` | working | The Session's current turn stops; the Mission waits, `blocked`, until it is resumed. |

There is no pause: providers have none. A quarantined Mission is released by an operator
(`ai6 quarantine release`), not retried.

```bash
curl -f -X POST "$AI6_EXPORTS_URL/v1/user/operations/G/missions/mission-007/actions" \
  -H "authorization: Bearer $AI6_ACCESS_TOKEN" \
  -H 'content-type: application/json' \
  -d '{"action":"resume","message":"The review is unblocked; carry on.","idempotency_key":"resume-1"}'
```

Universal Exports audits the request (`mission.cancel`, `mission.retry`, `mission.resume`,
`session.interrupt`) and queues the control for the Station holding the Mission's latest
Session record; failing that, the Station holding its brief's lease; failing that, the
Headquarters Station. A Station that has gone dark is passed over. That Station is the
authority: it checks that the Mission is in its book under that Operation, that it still
holds the Session's lease with its fencing token and, when the request named a
`session_id`, that the Session is still the Mission's current one, then applies the
control and reports `applied` or `rejected` with the Mission's state and a reason. Follow
the control with `GET .../actions/{control_id}`; one no Station acts on within two minutes
is `expired`. Repeating a request with the same `idempotency_key` returns the first one.

`ai6ctl mission cancel|retry|resume` and `ai6ctl session interrupt` do the same and wait for
the outcome; Moneypenny offers them in each Mission card's menu, with the reason any action
is unavailable. Closing a Session view never sends a control.

## Activity timeline

An operator with `audit.read` on an Operation reads what changed across it, newest first,
from `GET /v1/user/operations/{operation_id}/timeline` or Moneypenny's Timeline view:

| Kind | Recorded by | What it says |
| --- | --- | --- |
| `lifecycle` | Universal Exports | A Mission's state changed, on which Session and Station. |
| `control` | Universal Exports | A cancel, retry, resume or interrupt was applied, rejected or expired, and the state after. |
| `cable` | the sending Station | An Agent sent a Cable: sender, scope and recipient count. |
| `sleeper` | the Registry | A Sleeper was authorized or revoked, and by whom. |
| `audit` | Universal Exports | Mission requests and controls, proposal decisions, Agent role, quarantine and Station settings changes. |

Each item names its Station, Mission and Session where it has one, so the view filters by
Mission (`?mission=`). It never holds a prompt, a resume message, a Cable's body or target,
terminal bytes or a secret, and a Cable shows only in its sender's Operation; live terminal
output stays on the Session's own stream. Pages hold at most 200 items and follow an opaque
`cursor`; activity arriving meanwhile goes on top without shifting older pages. The
Registry keeps the newest 10,000 activity events.

## File-backed intake

The Registry defaults to the platform AI6 state directory and can be overridden
with `AI6_REGISTRY`. Briefs in `new/` are claimed atomically and filed under `claimed/`,
then `done/` or `failed/`. The stable brief identity combines its source and external
record ID. User-supplied integrations can write validated brief JSON into `new/` or call
the Universal Exports API.

SQLite/PostgreSQL Registry backends and distributed Registry placement are not currently
implemented. A Session runs on its Station's host, or, for a Theater reached only through a
transport, on the Station in remote-hands mode (see [Theaters](theaters.md)).

## Recovery

Work should survive a refused quota and a lost lease. Two mechanisms exist today.

### Session records and fencing

Besides the Station's own `missions.json`, every Session has a record in the shared
Registry (`sessions/`). It holds AI6's Session ID (distinct from the provider's), the
provider Session ID, the Station that supervises it, its Theater, the attempt number and the
ID of the Session it replaced, its exposure to untrusted input, and its status (`active`,
`parked` or `ended`). Another
Station that opens the same Registry sees the same records.

A Station acts on a Session only while it holds the record's **lease**, and every write
presents the **fencing token** the lease was granted with. The token comes from the
Headquarters election term and always grows past the previous holder's, so a Station that
was paused, lost its lease and woke up again is refused: it closes its harness, abandons
the Mission and leaves the safe house for the new holder. A live lease held by another
Station also refuses a resume. The daemon renews its leases with each Station check-in
(15 seconds; a lease lasts 60). A Mission started before records were shared gets a record
the next time it resumes.

Moving a running Session to another Station, or to another Theater when its own is lost,
is not implemented: a Session's records and lease are the groundwork for it, and
provider transcripts stay with the Theater that holds them.

### Provider limits: park and resume

A provider can refuse work until a quota resets (a five-hour or weekly window). The Claude
and Codex AgentHandlers recognise that, read the reset time when the provider reports one,
and end the turn with a `limit` event instead of `completed` or `failed`. The daemon then
closes the harness and **parks** the Mission:

```json
{ "state": "parked", "parked_until": 1790000015 }
```

`parked_until` is the reset time plus 15 seconds of grace, in Unix seconds. A Mission
parked by a provider that gave no reset time, or a time already past, waits
`limit_fallback_secs` (30 minutes by default) instead of retrying in a loop. The safe house
stays in place. When the time comes the daemon resumes the **same provider Session** and
tells it to carry on; the Mission returns to `working`. Parking survives a daemon restart,
and an operator can resume a parked Mission early with the ordinary resume request.

An Operation chooses the response with `on_limit` ([settings](operations.md#provider-limits)):

- `wait` (the default) parks as above.
- `backup` posts the next qualified Agent, as [Organization](organization.md) describes,
  among those whose handler differs from the limited one. The Mission is queued again for
  that Agent as a **new attempt** (a new Session record whose predecessor is the limited
  one), starting a new provider Session in the same safe house with a note that an earlier
  attempt stopped at a limit. With no such Agent available it parks instead.

The Moneypenny board shows `parked` as a Session state, and the stream of Mission events
carries the `limit` event.

### Budgets, exposure and quarantine

[Medical](medical.md) adds three things to a Mission:

- **Usage.** Tokens and cost reported by the provider are recorded per Session and Mission
  in the shared Registry, and `GET /v1/operations/{operation_id}/missions/{mission_id}`
  returns them as `usage`. A [budget](medical.md#budgets) that is reached stops the Session
  and leaves the Mission `blocked` with `blocked_reason: "budget"`; raise the limit and resume.
- **Exposure.** A Session that read a Source item, called a web tool or read a Cable from an
  exposed Session is marked on the board (`exposure`, shown as an `EXPOSED` chip in Moneypenny)
  and, under the default `strict` policy, restricted. See [Exposure](medical.md#exposure).
- **Quarantine.** A Mission whose Agent is [quarantined](medical.md#quarantine) is
  `quarantined`: its safe house is kept, it is not resumed, and releasing the Agent returns it
  to the queue with the next qualified Agent. `quarantined` is a Session state in the board,
  `ai6ctl` and Moneypenny.

## Sources

A **Source** is a Guatiao provider that polls a tracker or CI system for work and posts the
outcome back: `github` (labeled issues and pull requests) and `jira` (a JQL search). An
Operation names its sources under `sources.<provider>` in `operations.toml`
([settings](operations.md#sources)).

```text
tracker ─► Source.poll ─► brief (new/) ─► Mission ─► Source.report ─► comment on the item
```

- **Where it runs.** Only the Station holding the Headquarters lease polls, every
  `interval_secs` (default 60, at least 10) per source. Provider calls block on the
  network, so they run off the daemon's async runtime. Each Operation and provider keeps a
  cursor in the Registry, so a Station that takes over the lease resumes where the last one
  stopped.
- **Brief identity.** An item becomes a brief keyed `source_<provider>--<item>`. If the
  item changes while its brief is still in `new/`, the brief is updated in place. Once a
  Station has claimed it, polling leaves it alone; a finished item is never queued again,
  so a Mission is never duplicated.
- **Untrusted text.** The brief's title is generated. The tracker's own title, labels and
  description are placed between fenced markers and introduced as data that cannot change
  the Agent's task or permissions. A description cannot close the fence.
- **Credentials.** A source names environment variables (`token_env`, `email_env`); values
  are read when needed and never written to configuration, logs or error messages.
- **CX report.** When a Mission ends, the Headquarters Station posts a comment on the
  item (and may add a label or move a Jira issue): the outcome, plus what the
  Operation's cover policy allows. `covert` says only that the work finished or could not
  be completed, and the branch; `partial` adds that AI helped and the Mission ID; `overt`
  also names the Agent and the provider Session IDs. A report that fails is retried a few
  times, then dropped.
- **Limits.** Review comments on a pull request are not forwarded to the Mission's
  Session. Jira issues are not transitioned when a Mission starts, only when it ends.

## Sleepers

A **Sleeper** is a standing order: a brief that queues itself on a schedule, such as a
nightly dependency update. It is configured on the Operation and does nothing until an
operator approves it.

```toml
[operation.G.sleepers.nightly-deps]
schedule = "0 3 * * *"   # minute hour day-of-month month day-of-week, in UTC
title = "Update dependencies"
prompt = "Update the lockfile, run the tests and keep the work on a branch."
role = "maintenance"      # optional: agent, role, priority
```

```bash
ai6 sleeper list
ai6 sleeper authorize G nightly-deps
ai6 sleeper revoke G nightly-deps
```

- **Authorization.** `authorize` approves the sleeper exactly as written. Editing its
  schedule, title, prompt or assignment withdraws the approval until it is authorized
  again. Windows at or before the approval never fire.
- **One brief per window.** The Station holding the Headquarters lease checks the clock
  every 20 seconds and queues the brief for the latest scheduled minute. The brief's key
  comes from the Operation, the sleeper and that minute, so a Station that takes over the
  lease after a failover finds the key already used (queued, running or finished) and
  queues nothing. A window more than 24 hours old is skipped rather than run late.
- **Prompt.** The prompt is the operator's own text, not fenced as untrusted data.
- **Events.** There is no event trigger; a CI failure or a new issue arrives through a
  [Source](#sources).

## Restart recovery and publication

AI6 persists the selected Agent, handler, terminal, working directory, model, effort,
provider Session ID and cover settings for a started Mission. Environment values are
not stored. On restart, the scheduler retries reattachment using the saved provider
Session ID and verified Station safe house. A handler must return the same Session ID or
AI6 refuses the resume.

A Station can publish lifecycle and terminal events to Universal Exports. When
enabled, it consumes the gateway's authenticated input/resize queue and routes frames to
the matching live provider Session. Configure `AI6_EXPORTS_URL` and
`AI6_EXPORTS_STATION_TOKEN` together; leave both unset to disable publication. Events use a
bounded background worker and finite request timeouts, so a gateway outage does not block
the provider Session. Terminal bytes remain binary.
