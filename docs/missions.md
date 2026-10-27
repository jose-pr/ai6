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

A Mission Universal Exports knows nothing of (no Session record, brief lease, queued brief of
the Operation or board entry) is refused `404`, before anything is audited. Otherwise
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
of exactly that step. (M files the `clearance` requests itself, to promote an Agent; see
[Track record and promotion](agents.md#track-record-and-promotion).) It calls
`request_approval` on its Session's AI6 tool endpoint with
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

A class authorisation of kind `proposal` lets M promote Agents' Mission proposals without
asking: its target pattern is matched against the proposal's title, its Agents against the
proposing Agent and its roles against the role the proposing Session's own Mission was given
(recorded with the proposal, never the proposal's word for it). A proposal that asks for a
role must ask for that same role or one the class authorisation lists in `proposed_roles`.
Headquarters vets each pending proposal (the [vetting rules](#vetting-and-held-briefs)
other than the hourly rate, then the Chief of Staff's review when the Operation has one) and
shows the result in the proposal inbox. A proposal whose vetting found nothing is promoted
when such a class authorisation covers it, once it is 30 seconds old (so the proposing
Session's last tool results have reached the Registry) and only if its Session had not read
untrusted input: neither the proposal's own mark nor, under an Operation that restricts
proposals, the parent Mission's Session records may show an exposure. One use is counted
and the approval is audited as `mission.proposal.approved` by `class:<id>`. Every other
proposal waits for an operator, as before; none is runnable before it is promoted. A
promotion, by a class authorisation or an operator, clears the promoted brief in the same
Registry step: of its review when reviewed as a proposal, and, for an operator's approval,
of everything vetting found except a malformed brief.

An approval can be withdrawn before it expires (`POST
.../green-ink/{request_id}/revoke`, or `ai6ctl green-ink revoke`); it then covers nothing,
and a credential leased against it is revoked. Without the app, `ai6ctl` signs the same
way:

```bash
ai6ctl green-ink list --operation G
ai6ctl green-ink approve "$REQUEST_ID" --operation G --expires-in 1800 --note "main only"
ai6ctl green-ink reject "$REQUEST_ID" --operation G --reason "not before the audit"
ai6ctl class-authorizations list --operation G
ai6ctl class-authorizations revoke "$CLASS_ID" --operation G
```

Every change is audited with the step it concerns (`REQUEST_ID KIND TARGET`, or a class
authorisation's `ID KINDS PATTERN`), never the Agent's reason:

| Action | Actor | When |
| --- | --- | --- |
| `green_ink.requested` | `agent:<codename>` | An Agent's request is recorded and waits for a person |
| `green_ink.approved`, `green_ink.rejected` | the signer | A person decides |
| `green_ink.revoked` | the person | An approval is withdrawn |
| `green_ink.expired` | `expiry` | The Station tells the Agent a request expired undecided |
| `class_authorization.created`, `class_authorization.revoked` | the person | A class authorisation is granted or withdrawn |
| `class_authorization.used` | `class:<id>` | A class authorisation approves a request |

### What holds a step

Two things hold a step an Agent takes without green ink.

The **harness gate** turns an attempt into a request. Before Claude Code runs a shell
command (a `PreToolUse` hook running `ai6 green-ink gate`), whenever Codex asks to run one
(a gated Codex thread asks before every command it does not consider a safe read), and before
the Station's own `theater_run` runs a command on a remote-hands Theater, AI6 names the
governed steps in it and asks the repository the command acts on what its refs and push
settings mean:

- each ref a `git push` updates, with the remote it goes to: refspecs as written (`--force`,
  `+ref`, `heads/main`, `tag v1` included), `--all`, `--tags`, `--follow-tags` and
  `--mirror` as `refs/heads/*`, `refs/tags/*` and `refs/*`, and for a plain `git push`
  whatever `remote.NAME.push`, `push.default` and the branch's push remote say, `-c` on the
  command included;
- each ref `git push --delete` or `:ref` removes;
- `gh pr merge NUMBER` (with its `-R OWNER/REPO`), `gh release create TAG`, and `gh api`
  calls that merge a pull request.

It reads through shell keywords (`if`, `do`, `{`, `!`), wrappers (`sudo`, `env`, `timeout`,
`nice`, `nohup`, …), `sh -c`, `pwsh -Command`, `cmd /c`, `eval` and git aliases, and follows
`cd`, `git -C` and `GIT_DIR` to the right repository. It then asks the Station whether the
Operation's policy and a live approval of the Mission cover each step. If not, the command is
refused and the Agent is told the exact `kind` and `target` to pass to `request_approval`:

```text
Green ink: it would push origin:refs/heads/main, which needs green ink: call
request_approval with kind "push" and target "origin:refs/heads/main", wait until it is
approved, then run the command again.
```

A push to a ref the Operation does not protect goes ahead; a destination with a pattern
character (`refs/heads/*ain`) is always protected. An approval covers the same ref on the same
remote however it was written (`origin:main` covers `git push origin refs/heads/main`), for as
long as it lasts; an approval without a remote covers every remote. The gate holds what it
cannot read rather than guess: a ref the repository cannot name, a directory it cannot follow
(`cd $DIR`), arguments from `xargs`, `gh api` calls that move refs or releases, and any other
command that runs `git push` or `gh` (`find -exec`, `Start-Process`). It is still best effort:
arguments built at run time, `gh` aliases and other programs that push are not seen. An
approval is not tied to one commit.

The **credential** is the guarantee. A loadout secret marked `green_ink` (say, the token
that can push to `main`) reaches a Session only while the Mission holds a live approval of a
kind and target it names, once per approval on any Station, and its lease is revoked as soon
as the approval expires or is revoked (see [Credentials](agents.md#credentials)). Credentials already on the
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
with `AI6_REGISTRY` or `ai6 serve --registry`, which also take a SQLite or PostgreSQL
database ([Stations](stations.md#shared-registry)). In a directory, briefs in `new/` are
claimed atomically and filed under `claimed/`, then `done/` or `failed/`; in a database
the same states are rows of `ai6_briefs`. The stable brief identity combines its source
and external record ID. User-supplied integrations can write validated brief JSON into a
directory Registry's `new/` or call the Universal Exports API.

### Provenance

Every Mission records where its brief came from, its **provenance** (bona fides): AI6 sets
it where the brief enters, from what it authenticated there, and keeps it apart from the
brief record (in a directory Registry, `provenance/KEY.json`; in a database, a column), so a
`provenance` field written into a brief file or row is never read.

| `origin` | Who asked (`principal`) | `reference` | Set by |
| --- | --- | --- | --- |
| `operator` | the person, from the gateway's Directory | | Universal Exports (`POST .../missions`) |
| `source` | the Source provider (`github`) | the item; `item_url` its link | the Headquarters Station's poll |
| `sleeper` | the sleeper's name | its definition's fingerprint | the Headquarters Station |
| `proposal` | the proposing Agent | its provider Session | the approval (`universal-exports`) |
| `review` | nobody | the review's identity | the Headquarters Station, for a [Chief of Staff review](agents.md#chief-of-staff) |
| `registry_drop` | nobody | | the Station that took the brief |

Each also carries `received_by`, `received_at` (Unix seconds) and `digest`, the
`sha256:HEX` of the whole brief record as stored (canonical JSON without the provenance): a
brief changed in any way after it was accepted, its Operation, assignment, Stations, link,
data or text, loses its provenance. When a brief finishes, its provenance is spent: it moves
beside the outcome (`STATE/KEY.provenance`, or the `finished_provenance` column), where only
the CX report reads it, so the same record queued again is a `registry_drop`. A brief found in the queue with none, written there by hand or
by an integration that did not go through AI6, is accepted as `registry_drop`: its Sessions
start exposed (`brief:registry_drop`, see [Medical](medical.md#exposure)), and a finished
brief named `source:PROVIDER` is reported to that Source only when it has a `source`
provenance for it naming that provider and item, and its Operation is the one its Source
data names. The Mission record, the gateway's Mission detail (`brief.provenance`) and
Moneypenny's Mission brief page show the origin. Anything that can write the Registry's own
records can still forge them: the queue is the integration point this separates.

Distributed Registry placement is not implemented. A Session runs on its Station's host,
or, for a Theater reached only through a transport, on the Station in remote-hands mode
(see [Theaters](theaters.md)).

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
Station through its private API lives only in that Station's book.

### Runner loss: the Station lives

A Station that cannot reach a Session's runner keeps waiting, since a dropped link costs
only time: the runner spools what it misses. It does not wait forever. Once the runner has
been out of reach for the Operation's `lost_after_secs` (45 seconds, three check-ins, unless
set), the Station probes it once more through the Theater's transport. If the runner answers,
the Station carries on reading where it stopped and the wait starts over. If the runner is
gone, no longer has the Session, or the Theater cannot be reached, the Station gives the
Session up:

- it stops reading the harness and lets its Session lease lapse, so the record is marked
  `lost` with the reason (the last events it saw are kept for the hand-off);
- it hands the brief back, which the Registry queues again as a [new
  attempt](#lost-sessions-a-new-attempt-with-a-hand-off), or files when the Operation says
  `on_lost = "fail"`; the Mission is `abandoned` in this Station's book, or `failed` when it
  came from no brief;
- the Mission's events end with a `lost` event naming the reason.

A runner that restarts has lost the harnesses it supervised and says so with a `lost` event,
which the Station handles the same way at once. A harness the [detached
wrapper](theaters.md#the-detached-wrapper) runs without a runner is treated alike: out of reach
for the grace, or quiet for it while its process is gone with no exit recorded, it is lost.

The same holds after a restart or an adoption. A Station that cannot re-attach to a Session
tries again after 5 seconds, then 10, 20 and 40, and every minute after that; once its tries
have failed for `lost_after_secs`, it probes the runner (or the Theater) and gives the
Session up the same way when the probe fails too.

When the next attempt runs on the Theater the lost one ran on, its Station first ends the
lost harness if the runner still has it, so the two never work side by side.

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
An attempt that works where the lost one did (the safe house its Station kept, or the same
directory on the same Theater) is told to inspect the work it finds there instead. Otherwise
the hand-off says the work could not be recovered.

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

- `rotate` (the default, also for a Mission outside any Operation) first marks the Session's [pool account](agents.md#account-pools)
  limited until the reset and frees it. When another account of the Agent's pools has
  headroom, a Session whose history goes with it (Claude Code on an environment
  credential, moving to another environment credential) is parked only until the next
  scheduler pass and resumed, same provider Session, on that account; otherwise (a Codex or
  `config-dir` Session, or no environment account with headroom) it is queued again as a new
  attempt,
  told it continues an earlier one, and starts on the other account. The timeline records
  `account` / `limited` with the account, the reset and what happens next. A Session with no
  pool account, or whose pools have no account with headroom, parks as for `wait`.
- `wait` parks as above.
- `backup` posts the next qualified Agent, as [Organization](organization.md) describes,
  among those whose handler differs from the limited one. The Mission is queued again for
  that Agent as a **new attempt** (a new Session record whose predecessor is the limited
  one), starting a new provider Session in the same safe house with a note that an earlier
  attempt stopped at a limit. With no such Agent available it parks instead.

The Moneypenny board shows `parked` as a Session state, and the stream of Mission events
carries the `limit` event.

The Headquarters Station also keeps a brief **queued** while every account of the pools its
Agent would draw from is limited or at its `max_concurrent`, instead of starting a Session
that cannot run. The reason ("no account with headroom in pools …", with the first reset
when every account is limited) is recorded once on the Operation's timeline (`account` /
`waiting`), and the brief is placed on the first pass after an account frees up.
Moneypenny's **Account pools** view and `ai6ctl pools --operation <id>` show each account's
Sessions, quota used, limit and tokens today (`pool.read`).

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

## Vetting and held briefs

Before M places a brief, the Station holding the Headquarters lease vets it under the
Operation's [`vetting` settings](operations.md#vetting). The checks are deterministic and
cost no tokens:

| Check | Applies to | Finding |
| --- | --- | --- |
| Size and visible text | every brief | `malformed`: the brief is **refused** |
| Same URL or title as queued, held or running work | every brief | `duplicate`, naming that work |
| Untrusted briefs passed in the last hour, per Operation and per Source | untrusted briefs | `rate` |
| The Operation's spend today at `operation_cost_usd_per_day` | every brief | `budget` |
| Unicode tag characters (outside an emoji flag), bidirectional overrides and unbalanced embeddings or isolates, runs of zero-width characters, variation selectors or fillers, invisible operators, HTML comments outside the quoted item, lines that imitate the markers around quoted text, manipulation phrases | untrusted briefs | `screening` |

**Untrusted** briefs are a Source's items, Agents' Mission proposals, and any brief whose
[provenance](#provenance) is missing or no longer matches it (a registry drop):
their text was written outside AI6's operators. An operator's or a Sleeper's brief is
trusted and is not screened or rate limited. Screening normalizes text before it looks for
phrases: invisible characters are removed, so a phrase split by one is still found. A lone
direction mark, an emoji's joiner or presentation selector, and a subdivision flag's tag
letters are ordinary text and are not findings.

A brief with no finding passes and is placed as usual. A refused brief is filed as failed with
outcome state `refused` (a Source still gets its CX report). Any other finding **holds** the
brief: it leaves the queue for the Operation's held briefs, where no Station claims it, until
an operator acts. Holding rather than refusing is deliberate: a real issue trips a check
often enough that a person should decide. When the Operation has a qualified
`chief-of-staff` Agent, an untrusted brief that passes the rules is also reviewed by it
before it is placed ([Chief of Staff](agents.md#chief-of-staff)); the review can only add a
hold. The Operation's timeline records each decision as an `intake` event (`passed`,
`reviewing`, `cleared`, `held`, `refused`, `released`) with the finding kinds, never the
brief's text.

An operator with `mission.create` on the Operation reviews held briefs in Moneypenny
(Administration → Held briefs), with `ai6ctl held-briefs`, or through the API:

```bash
ai6ctl held-briefs list --operation G
ai6ctl held-briefs release source_jira--OPS-12 --operation G
ai6ctl held-briefs refuse source_jira--OPS-13 --operation G --reason "a mirror of OPS-12"
```

Each held brief lists its findings (`duplicate: the same URL as other work (mission-41)`);
its text, which may carry what screening caught, is shown only on request and as plain text,
with each character that renders as nothing shown by its code (`<U+200B>`) and a run of tag
characters as the text it spells (`<tags: ...>`). A **release** returns it to the queue
cleared of the kinds it was held for, so the same checks do not hold it again. A clearance,
an operator's or a review's, is bound to the brief as the operator or reviewer read it (the
digest of its record): a brief that changes afterwards, such as a Source item edited while
its brief waits, is vetted and reviewed again. The clearance is kept apart from the brief, so
a brief's own text cannot claim one. A **refusal** files it as failed with outcome state
`refused`; a Source still gets its CX report, and the reason stays in the Registry. Both
are audited (`mission.brief.released`, `mission.brief.refused`). Releasing or refusing
again changes nothing; a brief already placed or finished can be neither (`409`). A held
brief is never claimed, and a Source that polls its item again leaves it alone.

A brief held only for a rate limit returns to the queue on its own after the Operation's
`rate_hold_hours` (24 by default) and is vetted again, so a burst does not leave work held
for good; the timeline records it as `requeued`. An Operation holds at most 1,000 briefs;
past that, a brief that would be held waits in the queue.

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
role = "maintenance"      # optional: agent, role, priority, stations, each_station
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
- **Stations.** `stations = ["ID", ...]` limits the sleeper's briefs to those Stations, as a
  brief's `stations` does: it narrows the Operation's list and never widens it.
- **A round of every Station.** With `each_station = true` the sleeper queues one brief per
  window for each Station whose last check-in placement accepts for the Operation and the
  sleeper's Agent or role (within its `stations`), each narrowed to that Station and keyed by
  the window and the Station, so a failover never doubles a round. A Station's load does not
  exclude it (its round waits for room); a Station that has gone dark gets its round when it
  checks in again within the window. This is how the [Head of
  Station](stations.md#head-of-station) runs.
- **Fingerprint.** Every field the sleeper sets is part of what `authorize` approves, so
  adding `stations` or `each_station` withdraws the approval until it is given again.
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
