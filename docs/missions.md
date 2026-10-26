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

## Green ink

Before an irreversible step (a push to a protected branch or tag, a merge, a release, a
deletion, a change of clearance) an Agent asks for green ink: a person's recorded approval
of exactly that step. It calls `request_approval` on its Session's AI6 tool endpoint with
the `kind` of step, its `target` (for a push, `REMOTE:REF` such as
`origin:refs/heads/main`), a `reason` and a retry key, and is answered at once:

| Status | Meaning |
| --- | --- |
| `not_required` | The Operation's [green ink policy](operations.md#green-ink) lets this Agent's clearance take the step. |
| `approved` | A live class authorisation covers it, or a person already signed it; `expires_at` says until when. |
| `pending` | A person decides. The Mission is `blocked` with reason `green-ink` until then. |
| `rejected`, `expired` | Do not take the step. A request nobody decides within 24 hours (`request_ttl_secs`) expires as refused. |

While a request is pending the Mission's [Watchdog](watchdog.md) is paused: waiting for a
person is not a stall, and terminal output or an operator message does not end the wait.
If the Agent's turn ends meanwhile, the Session closes and the Mission stays blocked.

An operator with `approval.decide` on the Operation sees the waiting steps in Moneypenny's
Administration → Green ink, or from `GET /v1/user/operations/{operation_id}/green-ink`,
and approves one for a chosen time (one hour unless chosen) or rejects it with a reason.
The signer is always the signed-in person. The Station holding the Mission tells the Agent
the outcome, the step and the expiry (in a message to its live Session, or by resuming the
Session with it), and the Mission is `working` again once nothing it asked for is pending.

```bash
curl -f -X POST "$AI6_EXPORTS_URL/v1/user/operations/G/green-ink/$REQUEST_ID/approve" \
  -H "authorization: Bearer $AI6_ACCESS_TOKEN" \
  -H 'content-type: application/json' \
  -d '{"expires_in_secs": 1800, "note": "Push the release branch only."}'
```

A **class authorisation** approves a kind of step in advance: step kinds, a target glob
(`origin:refs/heads/ai6/*`), the Agents (`*` for any) or roles it covers, the lowest
clearance, an optional use limit and an expiry. A matching request is approved at once and
the use is counted and audited as `class_authorization.used` by `class:<id>`. A Session that
has read untrusted input never uses one under the default exposure policy (the `approvals`
restriction, see [Exposure](medical.md#exposure)). Operators with `approval.class` grant,
list and revoke them from `/v1/user/operations/{operation_id}/class-authorizations` or
Moneypenny's Class authorisations tab.

Decisions and class authorisations are audited (`green_ink.approved`,
`green_ink.rejected`, `class_authorization.created`, `class_authorization.revoked`); the
reason text is not.

### What holds a step

Two things hold a step an Agent takes without green ink.

The **harness gate** turns an attempt into a request. Before Claude Code runs a shell
command (a `PreToolUse` hook running `ai6 green-ink gate`) and whenever Codex asks to run
one, AI6 names the governed steps in it: each ref a `git push` updates (`--force` and
`+ref` included; `--all`, `--tags` and `--mirror` as `refs/heads/*`, `refs/tags/*` and
`refs/*`), each ref `git push --delete` or `:ref` removes, `gh pr merge NUMBER` and
`gh release create TAG`, also inside `sh -c`, `bash -c`, `pwsh -Command` and `cmd /c`. It
asks the Station whether the Operation's policy and a live approval of the Mission cover
each one. If not, the command is refused and the Agent is told the exact `kind` and `target`
to pass to `request_approval`:

```text
Green ink: it would push origin:refs/heads/main, which needs green ink: call
request_approval with kind "push" and target "origin:refs/heads/main", wait until it is
approved, then run the command again.
```

A push to a ref the Operation does not protect goes ahead. An approval covers the same ref
however it was written (`origin:main` covers `git push origin refs/heads/main`), for as long
as it lasts. The gate is best effort: a command that builds its arguments at run time, or a
step taken through another tool, is not seen.

The **credential** is the guarantee. A loadout secret marked `green_ink` (say, the token
that can push to `main`) reaches a Session only while the Mission holds a live approval of a
kind it names, once per approval, and its lease is revoked as soon as the approval expires
or is revoked (see [Credentials](agents.md#credentials)). Credentials already on the
Theater are outside that, which is why the Station reports them as weakenings.

## Activity timeline

An operator with `audit.read` on an Operation reads what changed across it, newest first,
from `GET /v1/user/operations/{operation_id}/timeline` or Moneypenny's Timeline view:

| Kind | Recorded by | What it says |
| --- | --- | --- |
| `lifecycle` | Universal Exports | A Mission's state changed, on which Session and Station. |
| `control` | Universal Exports | A cancel, retry, resume or interrupt was applied, rejected or expired, and the state after. |
| `cable` | the sending Station | An Agent sent a Cable: sender, scope and recipient count. |
| `sleeper` | the Registry | A Sleeper was authorized or revoked, and by whom. |
| `green_ink` | the Station | An Agent asked for green ink (`requested`), or was told the decision (`approved`, `rejected`, `expired`): the step, its target and the request, and who signed. |
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

Work should survive a refused quota, a lost lease and a Station that goes dark (the
tradecraft word is *extraction*).

### Session records and fencing

Besides the Station's own `missions.json`, every Session has a record in the shared
Registry (`sessions/`). It holds AI6's Session ID (distinct from the provider's), the
provider Session ID, the Station that supervises it, its Theater, the attempt number and the
ID of the Session it replaced, its exposure to untrusted input, its status (`active`,
`parked`, `ended` or `lost`), how its harness was launched (handler, terminal, working
directory, model, effort, mode and the runner's key), the last 20 things it was seen doing
and its last safe-house checkpoint. Another Station that opens the same Registry sees the
same records.

A Station acts on a Session only while it holds the record's **lease**, and every write
presents the **fencing token** the lease was granted with. The token comes from the
Headquarters election term and always grows past the previous holder's, so a Station that
was paused, lost its lease and woke up again is refused: it closes its harness, abandons
the Mission and leaves the safe house for the new holder. A live lease held by another
Station also refuses a resume. The daemon renews its leases with each Station check-in
(15 seconds; a lease lasts 60). A Mission started before records were shared gets a record
the next time it resumes.

### Station loss: adoption

When a Station goes dark its brief leases stop being renewed. Once a brief's lease and its
Session's lease have both run out, the Headquarters Station decides what happens to the
Mission (a brief whose Mission never started is simply queued again):

- **Adopted.** The harness ran on a Theater's [field runner](theaters.md#the-field-runner),
  so it is still running. The Headquarters hands the brief's lease (same claim, same
  attempt) to a live Station that reaches that runner, placed as a new Session would be
  but only on that Theater and never on the dark Station. That Station takes the Session's
  lease with a larger fencing token, records the Mission as the dark Station launched it
  and re-attaches to the runner Session, receiving every event the dark Station did not
  take. Nothing is launched twice, and a parked Session resumes when its quota resets.
- **Lost.** The harness ran on the dark Station's own host (Theater `local`), or with no
  runner, so it ended with the Station. Its record is marked `lost` with the reason, its
  fencing token moves on so the Station is refused if it comes back, and the brief is
  queued again for a [new attempt](#lost-sessions-a-new-attempt-with-a-hand-off). An
  Operation with `on_lost = "fail"` files the brief as lost instead. A brief that has
  already been tried three times is filed as lost either way.

Only Missions that came from a brief are recovered this way: a Mission created on one
Station through its private API lives only in that Station's book. A Theater with a
runner that dies while its Station lives is not handled yet: the Station keeps waiting for
the runner.

### Lost Sessions: a new attempt with a hand-off

The Station that takes the brief starts the Mission as attempt N+1, linked to the lost
Session as its predecessor, with new AI6 and provider Session IDs (provider transcripts stay
on the Theater that held them, so the provider Session cannot be resumed). The Agent is told
about the attempt it continues:

```text
This Mission continues attempt 1, which was lost: Station "alpha" went dark with Theater
"local"; its harness ran on that Station's own host.
Its work was last saved as commit 3f2a9c01d4e7 on branch mission-7-fix of origin; your
working directory starts from it. Inspect it before you continue: changes made after that
save are gone.
What it was last seen doing, oldest first:
- text: running the parser tests
- tool: Edit
```

The work survives only if it was **checkpointed**. With `workspace.checkpoint_remote` set
(see [Operations](operations.md#workspace)), the Station saves each running Session's safe
house every `checkpoint_secs`: one commit on top of the branch's own commits that also
holds the uncommitted changes, pushed to the Mission's branch on that remote, without
touching the Agent's worktree, staging area or `HEAD`. The commit follows the Operation's
cover (no Agent or model named, the cover's footer, the `cover_identity` as author). The
next attempt's safe house is then a new worktree on that branch at that commit, so the
`git` provider fetches it from the remote; a provider without history starts from the base.
Without a checkpoint the hand-off says the work could not be recovered.

The attempts are on the Mission: `GET /v1/missions` and the Mission detail list each one's
Station, Theater, status, why it was lost and the branch it was checkpointed to, and
Moneypenny shows them on the Mission card.

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
