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

A **Mission ID** identifies one unit of work and may group multiple agent Sessions. It is a
**UID**, a UUIDv7 in lower-case text (`019a1b2c-3d4e-7f00-8123-456789abcdef`), minted where the
brief enters the Registry. It is the Registry's key, and every relation uses it: Session
records, Cables, reviews, linked issues, green ink, memories, audit records, the timeline,
controls and routes. It never changes. A **Session ID** comes from the provider and remains
stable when AI6 resumes that same agent operation.

What people see is the Mission's **handle**, which AI6 gives where the brief enters, so a person
can recognise a Mission from it. A tracker's code is the tracker's: the handle may quote it, and
the link to the issue stays apart from it.

| Where it came from | Handle | Example |
| --- | --- | --- |
| A person, `ai6ctl`, Moneypenny, an Agent's proposal, a review | `<operation>-<n>` | `ai6-42` |
| A Jira Source | `<operation>-<item key>` | `ai6-AI6-64` |
| A GitHub Source | `<operation>-gh-<number>` | `ai6-gh-63` |
| A GitLab Source | `<operation>-gl-<iid>` | `ai6-gl-12` |
| A Source instance that is not its provider's only one in the Operation | its instance name replaces `gh` or `gl`; a Jira key already names its project | `ai6-backend-63` |
| A Sleeper | `<operation>-<sleeper>-<n>`, the same counter | `ai6-nightly-43` |

- The counter is per Operation and shared by every Station and gateway on the Registry. Numbers
  count Missions in the order they entered and are never reused, even when a brief is
  deleted. A brief with no Operation uses `mission` in its place. A review and a proposal take
  their number when they enter the queue; a proposal has none while it waits.
- The Mission stays the same through a retry, a return to the queue, a resume and a Station
  restart, on every Station. A re-polled item or a repeated approval finds the Mission it
  already names, and an item keeps its Mission if the Operation adds a second instance of its
  provider later.
- Nothing reads a code or a number back out of a handle. Only ASCII letters, digits, `_` and `-`
  stay in it, so a code with odd characters is made safe rather than refused, and the handle is
  valid as a Git branch and a URL part. Past `[registry.limits.briefs] handle_bytes` it keeps
  its start and adds a short hash. A handle is unique across the Registry, compared without
  case, and never given twice: when one is already taken (Operation `ai6-gh`'s Mission 63 and
  Operation `ai6`'s GitHub item 63) a number is skipped or a short hash is added. No handle
  reads as a UID.
- A brief dropped into the Registry's queue by hand goes through none of this: its key is
  `<source>--<id>` from its own fields, and it goes by that.

A handle can be **renamed** by whoever holds `mission.control` on the Operation, to any handle no
other Mission was ever given, in the same character set:

```bash
curl -f -X POST "$UNEX_URL/v1/user/operations/G/missions/ai6-42/rename" \
  -H "authorization: Bearer $UNEX_ACCESS_TOKEN" \
  -H 'content-type: application/json' \
  -d '{"handle":"ai6-flaky-watchdog"}'
```

The answer is `{"operation_id", "mission_id", "handle", "previous"}`; a refusal is
`{"error": TEXT}` with `400` (a malformed handle), `409` (a handle another Mission had) or `404`.
A rename changes nothing kept by the UID, and the old handle stays reserved and still names the
Mission, so old links and tracker comments keep working; the Mission can take a former handle of
its own back. It is audited as `mission.rename` and shows on the timeline as a `control`
`rename.done` with the old and new handles. `ai6ctl missions rename MISSION HANDLE` and the
Rename action on Outer Office's Mission detail do the same.

Every route and `ai6ctl` command that names a Mission takes its UID or any handle it had,
compared without case, and answers with its UID (`mission_id`) and its current `handle`;
`GET /v1/missions?mission=ID` finds one. What leaves AI6 takes the handle the Mission has when it
is written: the branch of its safe house, a tracker comment, a report. A rename does not rewrite
them.

A **run** is one Session record of a Mission. It has a UID of its own, and a handle
`<mission handle>.<attempt>` from the handle the Mission had when the run opened: `ai6-42.1`,
`ai6-42.2`. A Mission has one record per attempt. The run keeps its handle through a resume and
a rename, and the provider's Session ID stays apart from it, shown as it always was.

A **name** sits beside the handle and is no part of it: a person's title, or an issue's title on
one line and cut to `[registry.limits.sources] name_bytes`. Whoever can open an issue writes that
title, so it is data: it reaches a prompt only inside the fence with the rest of the issue's text,
and Outer Office and `ai6ctl` show it as plain text. Outer Office and `ai6ctl mission` also link
to the issue, and search matches the ID, the handle, the run and the name.

```json
{
  "title": "Fix the flaky watchdog test",
  "prompt": "It fails one run in ten on macOS.",
  "operation_id": "G"
}
```

## Queue a Mission through Universal Exports

An authorized user needs `mission.create` for the target Operation. The gateway returns
the queued Mission's UID and handle without echoing the prompt:

```bash
curl -f -X POST "$UNEX_URL/v1/user/operations/G/missions" \
  -H "authorization: Bearer $UNEX_ACCESS_TOKEN" \
  -H 'content-type: application/json' \
  -d '{"title":"Fix the flaky watchdog test","prompt":"Investigate the intermittent failure."}'
```

Outer Office's Operations view offers the same action. The AI6 daemon claims the brief
from the shared file-backed Registry, then resolves the Agent role, Operation
cover, Station capacity and provider handler before it starts a Session.

Work on a ticket is numbered by the ticket:

- `"issue": {"tracker": "gh", "item": "o/r#31"}` follows up an item of one of the Operation's
  tracker instances that already has a Mission (`G-gh-31`). The new Mission takes that Mission's
  handle as it is now and the item's next follow-up number, from 2: `G-gh-31-2`, then
  `G-gh-31-3`. It is linked to the item (`related`). An item with no Mission is refused `400`
  with the reason.
- `"create_issue": {"tracker": "gh", "title": "...", "body": "..."}` creates the Mission's
  ticket first, and needs `issue.create` as well; the title and body default to the Mission's.
  The answer is `202` with `state: "awaiting_issue"` and the issue `proposal` (an approved one),
  and no handle yet: the Headquarters Station creates the issue and then enters the Mission,
  numbered by it as its Source numbers items (`G-gh-104`) and linked to it as its source. A
  tracker that cannot create issues says so on the proposal (`failed`), and no Mission enters.
  The item, polled later, leaves the person's brief as it is.

```bash
ai6ctl missions create --operation G --title "Add the regression test" \
  --prompt "Cover the fix." --issue gh:o/r#31
ai6ctl missions create --operation G --title "Fix the login page" \
  --prompt "It times out." --create-issue gh
```

`"stations": ["ID", ...]` pins the Mission to those Stations. A pin is an exact Station ID (`[station]
id`, or the `HOST-xxxxxxxx` a Station gives itself), so one that names no Station that ever checked in,
or one the Operation's own `stations` leaves out, is refused `400` with the reason; an unknown pin is
told the IDs it equals ignoring case and those of Stations on a host of that name.

A brief placement finds no place for stays queued, and the Headquarters Station says why once per
brief and reason, in its log and on the Operation's timeline (`account`, `waiting`, with `brief` and
`reason`): each Station, Theater and Agent ruled out, with its reason. A pinned brief is told only
about the Stations it names, and about each that never checked in. Reasons that differ only in their
figures (seconds silent, Missions held) are one reason.

A Mission whose harness its Agent's environment cannot find fails at its first start instead of
being tried again: the Station looks the handler's program (`AgentHandler::program`) up on that
environment's own `PATH`, never its own, and fails the Mission with "Agent X has no PATH in its
environment (no `[loadout.X] env`)" or "... is not on its PATH"; the brief is filed failed with
that reason.

## Cancel, retry, resume and interrupt

An operator with `mission.control` on the Operation can act on a Mission without reaching
its Station:

| Action | Accepted when the Mission is | Effect |
| --- | --- | --- |
| `cancel` | any state | The live Session is interrupted and closed; the Mission is `abandoned` and its safe house is finished with the work kept. A Mission no Station has taken yet is withdrawn instead (below), and one that already ended stays as it is and answers `applied` with its state. |
| `retry` | stalled, done, failed, blown or abandoned | The Mission is `queued` again as a new attempt, for `agent` when one is named (configured for the Operation, qualified for the Mission's role and not quarantined), else for the Agent the roster gives it now; it continues in the same safe house. |
| `resume` | parked, blocked or stalled | The Mission resumes now on its provider Session, with `message` sent to the Agent, or a line telling it to carry on: a harness started again does nothing until it reads one. |
| `interrupt` | working | The Session's current turn stops, however the harness reports that; the Mission waits, `blocked` with reason `interrupted`, until it is resumed. |

There is no pause: providers have none. A quarantined Mission is released by an operator
(`ai6 quarantine release`), not retried.

```bash
curl -f -X POST "$UNEX_URL/v1/user/operations/G/missions/mission-007/actions" \
  -H "authorization: Bearer $UNEX_ACCESS_TOKEN" \
  -H 'content-type: application/json' \
  -d '{"action":"resume","message":"The review is unblocked; carry on.","idempotency_key":"resume-1"}'
```

A Mission Universal Exports does not know under the Operation in the path (no Session record,
brief lease, brief in any state or board entry of that Operation), or that any of them puts in
another Operation, is refused `404`, before anything is audited. Otherwise
Universal Exports audits the request (`mission.cancel`, `mission.retry`, `mission.resume`,
`session.interrupt`) and queues the control for the Station holding the Mission's latest
Session record; failing that, the Station holding its brief's lease; failing that, the
Headquarters Station. A Station that has gone dark is passed over. That Station is the
authority: it checks that the Mission is in its book under that Operation, that it still
holds the Session's lease with its fencing token and, when the request named a
`session_id`, that the Session is still the Mission's current one, then applies the
control and reports `applied` or `rejected` with the Mission's state and a reason. A cancel of a
Mission no book holds, whose brief has ended, is `applied` with the brief's state; with no Station to
ask, Universal Exports answers that itself. Follow
the control with `GET .../actions/{control_id}`; one no Station acts on within two minutes
is `expired`. Repeating a request with the same `idempotency_key` returns the first one.

`ai6ctl mission cancel|retry|resume` and `ai6ctl session interrupt` do the same and wait for
the outcome; Outer Office offers them in each Mission card's menu, with the reason any action
is unavailable. Closing a Session view never sends a control.

A retry that names no Agent chooses one again from the roster as it is now, as placement would:
the best posted Agent qualified for the Mission's role and its brief's tags, not quarantined, held
or retired, whose handler the Station can run. The Agent the Mission had keeps it while it still
qualifies and ranks as high; a retry no Agent can take is rejected with the reason.

### Withdraw a queued Mission

A Mission no Station has taken, its brief queued or held, has nothing to cancel on a Station. An
operator with `mission.control` withdraws it:

```bash
curl -f -X POST "$UNEX_URL/v1/user/operations/G/missions/G-12/withdraw" \
  -H "authorization: Bearer $UNEX_ACCESS_TOKEN" \
  -H 'content-type: application/json' \
  -d '{"note":"pinned to a Station that was retired"}'
```

The brief is filed as failed with the outcome `withdrawn` and the reason "withdrawn by PERSON:
NOTE"; a Source still gets its report. The withdrawal is audited (`mission.withdraw`) and put on the
Operation's timeline (`intake`, `withdrawn`). A Mission a Station has taken, or that ended, answers
`409` naming `mission cancel`, and one the Operation has no such brief for `404`. `ai6ctl mission
withdraw ID [--note TEXT]` does the same,
and a `cancel` of such a Mission withdraws it with no note and answers `applied`, state `failed`.

### A cancel the Station never confirms

Universal Exports records when a cancel of a claimed brief was first asked for. When the Station
holding it has not finished the brief `[registry.limits.briefs] cancel_settle_after` (300 s) later,
because its Session is wedged, the control never reached it, or it went to another Station, the
Headquarters Station files the brief as failed, "cancelled; the Station did not confirm", with the
Station and the Mission's Session at the time, and releases its lease. The Station holding the
Mission, finding at its next check-in that its brief was settled so while it is still on that
Session, stops the Mission; one retried since runs on.

### A brief that settles before its Mission starts

A Mission waiting to start reads its brief first. A brief the Registry has filed as done or failed
while it waited, as when a restart lapsed its lease and intake could no longer place it, is not
started or tried again: the Mission is dropped from the Station's book, and the timeline says so
(`mission`, `dropped`). A retry an operator or a review asks for after the brief was filed still
runs. A start that cannot succeed whatever is tried, because the Mission's Agent is not posted to the
Operation, fails the Mission at once with that reason, and its brief with it; other start failures
are tried again after `[scheduler] retry_delay`.

## Checking an outcome

An Agent saying it finished is not the work being done, so what it said is kept and a person can
record what they found.

**The final report.** The message an Agent ends its turn with is filed against its Session, up to
`[registry.limits.sessions] final_report_bytes` (64 KiB) with a cut marked, and goes into the Mission's
outcome as `final_report` when its brief is filed. A later turn replaces it. Everyone who may read the
Mission reads it, in `GET /v1/operations/{operation_id}/missions/{mission_id}` (each attempt's
`final_report`; the list of Missions leaves it out), in `ai6ctl mission ID` and in the Outcome section
of the Mission's page. It is the Agent's own account, as data. What another Operation may read of a
Mission through a [compartment](compartments.md) does not include it.

**A verdict.** A person with `mission.verify` on the Operation gives one on the latest attempt, once
it has ended `done` or `held`, or is [verifying](#verifying); anything else is refused (`409`, saying how
it stands):

```bash
curl -f -X POST "$UNEX_URL/v1/user/operations/G/missions/$MISSION/verdict" \
  -H "authorization: Bearer $UNEX_ACCESS_TOKEN" \
  -H 'content-type: application/json' \
  -d '{"verdict": "failed-verification", "note": "the tag was never pushed"}'

ai6ctl missions verdict "$MISSION" disputed --note "not on main"
```

| Verdict | On a `done` Mission | On a `held` Mission | On the [scorecard](agents.md#scorecard) |
| --- | --- | --- | --- |
| `confirmed` | Stays `done` | Becomes `done` | Done |
| `disputed` | Stays `done`, and shows it | Becomes `failed` | Disputed: a failure of that Agent |
| `failed-verification` | Stays `done`, and shows it | Becomes `failed` | Disputed: a failure of that Agent |

A later verdict replaces the earlier one while the attempt stands `done` or `held`. The verdict is kept
on the attempt's Session record, so a retry starts clean and the failure stays with the Agent that ran
it. M's holds and proposals read the same scorecard. It is audited as `mission.verdict` (target
`MISSION VERDICT`, never the note) and goes on the timeline (kind `verdict`, action the verdict). The
Mission's page and the board's attempts show it.

**Requiring a decision.** A role that sets

```toml
[role.releaser]
purpose = "Cuts releases"
policy = "..."
require_approval_before_done = true          # default false
```

holds its Missions' `done`: a Session that ends done with no green ink request decided in that attempt,
approved or refused, by a person or a class authorisation, leaves the Mission `held` with the reason
`no approval was decided` instead. A request that expired was not decided. What a brief or a prompt
says it will do cannot be read, so this is what can be enforced: the role that must ask, and did not.

A held Mission has no harness and its safe house is finished with the work kept. It holds no place in
the Station's capacity, keeps its lease and brief, and waits for a person: a verdict settles it within a
scheduler pass, and its brief is then filed with the outcome and the verdict. It cannot be resumed or
retried while held; `cancel` abandons it. If its Station is lost, its Session is lost like any other's,
and [Recovery](#recovery) decides. Review Missions are never held. It counts as waiting on a person on
the scorecard until the verdict.

## Verifying

A Session saying it is done does not make its Mission done. A Mission of an Operation whose Session ends
done is `verifying`: its work is kept, no harness runs, its brief stays claimed, and it holds no place in
the Station's capacity. It leaves `verifying` only by a decision, and never by itself: not when it is
old, and not when it declares nothing to check. Review and panel Missions, Head of Station rounds and
Missions with no Operation settle done as before.

**Deliverables.** What a Mission is to deliver is declared before the work, as text, `kind` or
`kind:target`, by its role and by its brief:

| Kind | Target | Read back by |
| --- | --- | --- |
| `branch` | a branch name; without one, the Mission's own branch | `git ls-remote` of the remote, from the Mission's working directory or its Operation's checkout |
| `commit_on` | `BRANCH`, or `BRANCH@COMMIT` | the branch's tip, then its history (the branch is fetched); without a commit, the one the working directory is at |
| `comment` | text a comment carries | the linked tracker item read back by id, its latest comments |
| `artifact` | a URL | an HTTP GET, status only |
| `approval` | optional: the kind of step | the Mission's green ink requests: one approved and not withdrawn |
| `file` | a path inside the working directory | the file, when the work is on the Station's own disk |

```toml
[role.releaser]
purpose = "Cuts releases"
policy = "..."
deliverables = ["branch", "commit_on:main", "approval"]
```

```bash
ai6ctl missions create --operation G --title "Fix the build" --prompt "..." \
  --deliverable branch --deliverable file:docs/report.md
```

The Station records the list on the attempt when it goes `verifying` (`deliverables` in the Mission's
detail), the role's first, then the brief's, each once.

**Probes.** The Station provides the means to look and decides nothing about what it finds. Each
probe reports `found`, `missing` (the place was read and the thing is not there, by name), `unprobed`
(this Station cannot look: the work is on a Theater it does not reach, or no tracker item is linked) or
`failed` (the look failed or ran out of time, so nothing is known). Each has its timeout in the Station's
`[probes]` table (`git_timeout`, `http_timeout`, `tracker_timeout`, `file_timeout`), and what each
found is kept on the attempt as `evidence`, oldest first, up to `[registry.limits.sessions]
evidence_entries`. A tracker probe works where the Station holds the tracker's credentials, and an
`artifact` probe makes a request from the Station to the address the role or brief names.

**The decision.** Until an Operation's own watch decides, a person with `mission.verify` on the
Operation does:

```bash
ai6ctl missions probe "$MISSION"                  # asks the Station to probe, waits, prints the findings
ai6ctl missions verify "$MISSION" done --reason "branch and approval are there"
ai6ctl missions verify "$MISSION" failed --reason "the work was never pushed"
ai6ctl missions verify "$MISSION" redispatch --reason "no branch on the remote" \
  --amend "Push the branch before you finish."
```

`POST /v1/user/operations/{operation_id}/missions/{mission_id}/probe` and `.../verify` (body
`{"decision": "done"|"redispatch"|"failed", "reason", "amendment"?}`) are the routes. A decision needs
its reason, is audited first (`mission.verify`, target `MISSION DECISION`, never the reason), goes on the
timeline (kind `verify`) and is refused with `409` unless the Mission's latest attempt is verifying.
Its Station then settles the Mission within a scheduler pass:

| Decision | The Mission | The attempt |
| --- | --- | --- |
| `done` | Becomes `done` | Counts as done on the scorecard |
| `failed` | Becomes `failed` | Disputed: a failure of that Agent |
| `redispatch` | Is queued as attempt N+1 of the same Mission, for the best qualified Agent, in the safe house the last attempt kept, with a new provider Session told the reason and the amendment after its brief | Disputed: a failure of the Agent that ran it |

A person who gives a verdict on a `verifying` Mission with `ai6ctl missions verdict` settles it the same
way as `confirmed` or `failed-verification`; only `verify` redispatches. A standing order whose attempt is
`verifying` waits for the decision like one that is held: its next window does not fire over it.

## Green ink

Before an irreversible step (a push to a protected branch or tag, a merge, a release, a
deletion, a change of clearance) an Agent asks for green ink: a person's recorded approval
of exactly that step. (M files the `clearance` requests itself, to promote an Agent; see
[Track record and promotion](agents.md#track-record-and-promotion).) It calls
`request_approval` on its Session's AI6 tool endpoint with
the `kind` of step, its `target` (for a push, `REMOTE:REF` such as
`origin:refs/heads/main`), a `reason` and a retry key, and optionally `links`: up to 8
`{label, url}` pairs naming what the request is about, each an `https` address with no credentials
(`[registry.limits.approvals] links`, `link_label_bytes`, `link_url_bytes`). It is answered at once:

| Status | Meaning |
| --- | --- |
| `not_required` | The Operation's [green ink policy](operations.md#green-ink) lets this Agent's clearance take the step. |
| `approved` | A live class authorisation covers it, or a person already signed it; `expires_at` says until when. |
| `pending` | A person decides. The Mission is `blocked` with reason `green-ink` until then. |
| `rejected`, `expired` | Do not take the step. A request nobody decides within 24 hours (`request_ttl_secs`) expires as refused, and is kept where a person can [re-arm it](#expired-requests). |

While a request is pending the Mission's [Watchdog](watchdog.md) is paused: waiting for a
person is not a stall, and terminal output or an operator message does not end the wait.
If the Agent's turn ends meanwhile, the Session closes and the Mission stays blocked.

An operator with `approval.decide` on the Operation sees the waiting steps on Outer Office's
Green Ink page, or from `GET /v1/user/operations/{operation_id}/green-ink`,
and approves one for a chosen time (one hour unless chosen) or rejects it with a reason.
The signer is always the signed-in person. The Station holding the Mission tells the Agent
the outcome, the step and the expiry (in a message to its live Session, or by resuming the
Session with it), and the Mission is `working` again once nothing it asked for is pending.

The Green Ink page renders a reason as Markdown and links what it names to the Operation's
[Sources](operations.md#sources): a Jira key of a configured project, `#12` or `owner/name#12`, a commit
SHA, `refs/heads/x` and `branch x`. `GET .../green-ink/sources` says where those are read (each GitHub or
GitLab instance's page, each Jira site and project, nothing not served over `https`). The app only forms
the address; it asks the tracker nothing, so such a link may lead to a page that does not exist. These
open at once, and so does a link the Agent gave to one of those hosts; any other shows its full address
first.

```bash
curl -f -X POST "$UNEX_URL/v1/user/operations/G/green-ink/$REQUEST_ID/approve" \
  -H "authorization: Bearer $UNEX_ACCESS_TOKEN" \
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
Outer Office's Class authorisations tab.

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
.../green-ink/{request_id}/revoke` with an optional `{"reason"}`, kept on the record as
`revoked.note`, or `ai6ctl green-ink revoke`); it then covers nothing, and a credential leased
against it is revoked. Without the app, `ai6ctl` signs the same way:

```bash
ai6ctl green-ink list --operation G
ai6ctl green-ink approve "$REQUEST_ID" --operation G --expires-in 1800 --note "main only"
ai6ctl green-ink reject "$REQUEST_ID" --operation G --reason "not before the audit"
ai6ctl green-ink revoke "$REQUEST_ID" --operation G --reason "wrong branch"
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
| `green_ink.rearmed` | the person | A request that expired undecided waits for a decision again |
| `class_authorization.created`, `class_authorization.revoked` | the person | A class authorisation is granted or withdrawn |
| `class_authorization.used` | `class:<id>` | A class authorisation approves a request |

### Expired requests

A request nobody decided in time counts as refused and leaves the inbox; it does not vanish. The
Expired tab of the Green Ink page, `GET .../green-ink/expired?since=UNIX` and `ai6ctl green-ink list
--expired` list those that expired since `since`, by default within `[text]
green_ink_expired_window` (30 days), the newest first and at most `[registry.limits.approvals]
expired_listed` (200). A request a person decided is not on it.

A person with `approval.decide` can re-arm one: `POST .../green-ink/{request_id}/rearm` with
`{"note"?, "expires_in_secs"?}`, `ai6ctl green-ink rearm REQUEST_ID --operation G [--note TEXT]
[--expires-in SECONDS]`, or the tab's button. The same request is pending again until the new expiry,
by default as long as it first waited, and decided like any other; approving it still needs what its
kind needs. Only an expired, undecided request can be re-armed: one still waiting or one a person
decided is `409`.

The Station holding the Mission takes it up within a scheduler pass. A working Mission is `blocked`
with reason `green-ink` again, and a live Session is told it waits again, having heard it expired. A
Mission still waiting stays so, with no resume. One whose Session ended and which went on to end has
nobody to tell: the approval is still recorded, and a retry of the Mission can use it.

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
`nice`, `nohup`, …), `sh -c`, `pwsh -Command`, `cmd /c`, `eval`, `iex` and git aliases, and
substitutions and here-documents, and follows `cd`, `git -C` and `GIT_DIR` to the right
repository. A redirection (`2>&1`, `> log`) is neither an argument nor the end of the arguments.
What it cannot read is held: a word only the shell can fill in (`git push origin HEAD:$BRANCH`),
a quote that never closes, an encoded PowerShell command. It then asks the Station whether the
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
from `GET /v1/user/operations/{operation_id}/timeline` or Outer Office's Timeline view:

| Kind | Recorded by | What it says |
| --- | --- | --- |
| `lifecycle` | Universal Exports | A Mission's state changed, on which Session and Station. |
| `control` | Universal Exports | A cancel, retry, resume or interrupt was applied, rejected or expired, and the state after. |
| `cable` | the sending Station | An Agent sent a Cable: sender, scope and recipient count. |
| `sleeper` | the Registry, the Stations | A Sleeper was authorized or revoked, and by whom; a window was `skipped` and why, or fired `late`; an attempt `started` or `resumed` its Session, `attempt-failed` or was `superseded`. |
| `green_ink` | the Station | An Agent asked for green ink (`requested`), or was told the decision (`approved`, `rejected`, `expired`): the step, its target and the request, and who signed. |
| `audit` | Universal Exports | Mission requests and controls, proposal decisions, Agent role, quarantine and Station settings changes. |

Each item names its Station, Mission and Session where it has one, so the view filters by
Mission (`?mission=`). It never holds a prompt, a resume message, a Cable's body or target,
terminal bytes or a secret, and a Cable shows only in its sender's Operation; live terminal
output stays on the Session's own stream. Pages hold at most 200 items and follow an opaque
`cursor`; activity arriving meanwhile goes on top without shifting older pages. The
Registry keeps the newest 10,000 activity events.

## Session context

When a Mission's Session ends, the Station files what the agent was given and did, as its
harness kept it, in the Registry: the Session's **context**. It asks the Session's handler where
its harness keeps the Session (`AgentHandler::session_record`) and reads that file: Claude Code's
transcript (`claude-code-jsonl`, `projects/*/<session>.jsonl` under `CLAUDE_CONFIG_DIR` or
`~/.claude`), Codex's rollout (`codex-rollout-jsonl`, `sessions/*/*/*/rollout-*-<thread>.jsonl`
under `CODEX_HOME` or `~/.codex`), or the Antigravity CLI's transcript (`antigravity-jsonl`,
`~/.gemini/antigravity-cli/brain/<conversation>/transcript.jsonl`). A handler that names none, such
as `acp`, a file that cannot be read, and a harness a runner supervises on another host get the
events the Station saw instead (`ai6-events-jsonl`, one `{at, kind, text, tools, usage, data}` per
line), and the record says so (`source`: `harness` or `events`). Moneypenny's Sessions are filed
the same way after each of her turns and when her harness closes.

A context keeps its newest 8 MiB, from a line start, and records how large it was (`cut_from`);
the Registry keeps 2,000 of them, the oldest filed going first (`[registry.limits.contexts]`
`context_bytes`, `records`). One is filed again in place of the last for the same Session.

`GET /v1/user/operations/{operation_id}/sessions/{session_id}/context` answers `{session, owner,
station, handler, format, source, filed_at, cut_from?, bytes, content}` with `session.context` on
the Operation, and each read is audited (`session.context.read`, resource `mission:<uid>`). A
Session of another Operation, or with no context, answers `404`. Outer Office shows it in the
Context tab of a Session's page: the turns, each tool call and its result folded, and
**Download** for the file as kept.

## File-backed intake

The Registry defaults to `registry` in the Station's state directory and can be named
with `ai6.cfg`'s `[registry] location` or `ai6 serve --registry`, which also take a SQLite or
PostgreSQL database ([Stations](stations.md#shared-registry)). In a directory, briefs in `new/` are
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
`sha256:HEX` of the brief record as stored (canonical JSON without the provenance, and
without the Mission ID and handle the Registry gives it): a brief changed in any other way after
it was accepted, its Operation, assignment, Stations, link, data or text, loses its provenance;
a rename does not. When a brief finishes, its provenance is spent: it moves
beside the outcome (`STATE/KEY.provenance`, or the `finished_provenance` column), where only
the CX report reads it, so the same record queued again is a `registry_drop`. A brief found in the queue with none, written there by hand or
by an integration that did not go through AI6, is accepted as `registry_drop`: its Sessions
start exposed (`brief:registry_drop`, see [Medical](medical.md#exposure)), and a finished
brief named `source:PROVIDER` is reported to that Source only when it has a `source`
provenance for it naming that provider and item, and its Operation is the one its Source
data names. The Mission record, the gateway's Mission detail (`brief.provenance`) and
Outer Office's Mission brief page show the origin. Anything that can write the Registry's own
records can still forge them: the queue is the integration point this separates.

Distributed Registry placement is not implemented. A Session runs on its Station's host,
or, for a Theater reached only through a transport, on the Station in remote-hands mode
(see [Theaters](theaters.md)).

## Recovery

Work should survive a refused quota, a lost lease and a Station that goes dark (the
tradecraft word is *extraction*).

### Session records and fencing

Besides the Station's own `missions.json`, every Session has a record in the shared
Registry (`sessions/`). It holds AI6's Session ID, the run's UID and its handle `<mission handle>.<attempt>` (distinct from the provider's), the
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
Outer Office shows them on the Mission card.

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

The Outer Office board shows `parked` as a Session state, and the stream of Mission events
carries the `limit` event.

The Headquarters Station also keeps a brief **queued** while every account of the pools its
Agent would draw from is limited or at its `max_concurrent`, instead of starting a Session
that cannot run. The reason ("no account with headroom in pools …", with the first reset
when every account is limited) is recorded once on the Operation's timeline (`account` /
`waiting`), and the brief is placed on the first pass after an account frees up.
Outer Office's **Account pools** view and `ai6ctl pools --operation <id>` show each account's
Sessions, quota used, limit and tokens today (`pool.read`).

### Budgets, exposure and quarantine

[Medical](medical.md) adds three things to a Mission:

- **Usage.** Tokens and cost reported by the provider are recorded per Session and Mission
  in the shared Registry, and `GET /v1/operations/{operation_id}/missions/{mission_id}`
  returns them as `usage`. A [budget](medical.md#budgets) that is reached stops the Session
  and leaves the Mission `blocked` with `blocked_reason: "budget"`; raise the limit and resume.
- **Exposure.** A Session that read a Source item, called a web tool or read a Cable from an
  exposed Session is marked on the board (`exposure`, shown as an `EXPOSED` chip in Outer Office)
  and, under the default `strict` policy, restricted. See [Exposure](medical.md#exposure).
- **Quarantine.** A Mission whose Agent is [quarantined](medical.md#quarantine) is
  `quarantined`: its safe house is kept, it is not resumed, and releasing the Agent returns it
  to the queue with the next qualified Agent. `quarantined` is a Session state in the board,
  `ai6ctl` and Outer Office.

## Review

An Operation, a role or a brief can ask that a Mission's output is judged before it is `done`, by a
panel of Agents on different models, with weights and rounds: [Review](review.md).

## Vetting and held briefs

Before M places a brief, the Station holding the Headquarters lease vets it under the
Operation's [`vetting` settings](operations.md#vetting). The checks are deterministic and
cost no tokens:

| Check | Applies to | Finding |
| --- | --- | --- |
| Size and visible text | every brief | `malformed`: the brief is **refused** |
| Same URL or title as queued, held or running work, other than another window of the same Sleeper | every brief | `duplicate`, naming that work |
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

An operator with `mission.create` on the Operation reviews held briefs in Outer Office
(Briefings → Held briefs), with `ai6ctl held-briefs`, or through the API:

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

A **Source** is a Guatiao provider that polls a tracker or CI system for work: `github`
(labeled issues and pull requests), `gitlab` (labeled issues, on gitlab.com or a self-managed
instance) and `jira` (a JQL search). The same providers also serve
the **Tracker** kind, which reads, comments on, labels and creates issues, so the outcome goes
back through the tracker the work came from. An Operation names its tracker instances under
`sources.<name>` in `operations.toml` ([settings](operations.md#sources)).

```text
tracker ─► Source.poll ─► brief (new/) ─► Mission ─► Tracker.post ─► comment on the item
```

- **Where it runs.** Only the Station holding the Headquarters lease polls, every
  `interval_secs` per source (default 60, at least 10: the `station` record's `[sources]
  poll` and `min_poll`). Provider calls block on the
  network, so they run off the daemon's async runtime. Each Operation and provider keeps a
  cursor in the Registry, so a Station that takes over the lease resumes where the last one
  stopped. A poll searches `settling_secs` (default 60; 0 for none) behind the cursor, since a
  listing can show an item after a newer one; what repeats changes nothing.
- **Brief identity.** An item's Mission ID comes from its Operation, instance and item
  ([the table above](#mission-and-session-identities)), and polling it again finds the same
  one. If the item changes while its brief is still in `new/`, the brief is updated in place.
  Once a Station has claimed it, polling leaves it alone, so a Mission is never duplicated. A
  finished item can only come back as the same Mission's next attempt ([reopening](#reopening-a-settled-item)).
- **Untrusted text.** The brief's title is generated. The tracker's own title, labels and
  description are placed between fenced markers and introduced as data that cannot change
  the Agent's task or permissions. A description cannot close the fence. Labels are information
  and nothing more: they never require an Agent's tags, so nobody who can label an issue can fail
  its Mission.
- **Credentials.** A source names environment variables (`token_env`, `email_env`); values
  are read when needed and never written to configuration, logs or error messages. A refused
  credential is an error, also on Jira Cloud, which answers its search with an empty page: an
  empty Jira Cloud poll makes one more request to tell the two apart.
- **Lag.** A tracker's listing can show a new or changed issue seconds late, so it can arrive
  one poll later. GitLab leaves an issue's change time alone for a change within about a minute
  of its last one, so its `settling_secs` defaults to 120.
- **CX report.** When a Mission ends, the Headquarters Station posts a comment on the
  item (and may add the instance's `done` or `failed` status label or make its status
  transition): the outcome, plus what the
  Operation's cover policy allows. `covert` says only that the work finished or could not
  be completed, and the branch; `partial` adds that AI helped and the Mission ID; `overt`
  also names the Agent and the provider Session IDs. `partial` and `overt` also list the tools
  the harness [refused in its last turn](agents.md#what-a-claude-code-mission-may-call). A report
  that fails is retried a few times, then dropped.
- **Limits.** Review comments on a pull request are not forwarded to the Mission's
  Session. Jira issues are not transitioned when a Mission starts, only when it ends.

### Reopening a settled item

An item whose Mission settled (its report was written on the item) is queued again, as the
**same Mission's next attempt** (same UID and handle, a run `<handle>.<attempt>` after the last),
when the instance's `reopen` says so:

| `reopen` | What reopens it |
| --- | --- |
| `label` (default) | A poll finds the item with a label it did not have when AI6 last read it: the trigger label taken off and put back, or one added. The labels AI6 writes (`status_labels`) and those its Agents may touch (`agent_labels`) do not count. |
| `comment` | As `label`, and also a comment AI6 did not write, by someone the tracker lets answer for the item (its author, a member, someone `reply_authors` names). |
| `never` | Nothing: a settled item stays settled. |

The attempt is briefed from the item as it reads now, and from the comments that reopened it,
quoted as untrusted data as the item is. Nothing reopens while an attempt is queued, held or
running. Each attempt has its own count of claims, so `intake_attempts` bounds the lapses of
that attempt alone, here and for a standing Sleeper's windows. Each reopen is audited (`mission.reopened`) and on the Operation's timeline (`source`,
`reopened`, with the reason).

To see a label taken off, the Headquarters Station reads the newest settled items of each such
instance again, at most `[sources] reopen_watch` (20) per instance every `reopen_watch_every`
(300 s); each read is one request to the tracker. A label taken off and put back between two of
those reads is not seen.

A Source's text is untrusted already. With `comment`, anyone the tracker lets answer for an item
can start its work again with what they write, so `comment` is reported as a weakening at start
and at check-in.

## Linked issues

A Mission that started from an issue is **linked** to it, and keeps it current. A link names a
tracker instance and an item (`github:jose-pr/ai6#12`, `gitlab:group/project#12`, `jira:GUA-12`), how the issue relates to
the Mission (`source`, `related`, `child`, `parent`, `mirror`), who linked it, and whether it
is in force (`proposed`, `active`, `unlinked`). The source issue is linked when its brief is
queued, so the Mission's page can show it before it is placed. A Mission holds at most 32
links; an unlinked one stays as a record and a later poll does not bring it back.

Every write to a tracker is an **outbox** entry first: a Registry record any Station may
queue, that only the Headquarters Station sends, because only it holds tracker tokens. An
entry is `pending`, then `sending` under the election term that began it, then `posted`,
`refused` or `failed`. A Headquarters Station that takes over finds what the last one left
`sending`: a comment already on the issue (found by the entry's hidden marker, or the same
text from AI6's own account) is recorded as posted, not posted twice; an entry that was not
sent goes back to pending; a creation is refused, since sending it again could make a second
issue. A failed send is retried with a doubling delay and fails after `send_attempts`; a
refusal (a call the tracker cannot make, a bad value, an unlinked issue) is never retried.
Each queue and settle is audited and appears on the timeline as an `issue` event, never with
the text.

**Waiting on the issue.** An Agent that needs more data asks with `issue_request_info`: the
question is an outbox entry like any other, the Mission becomes `blocked` with reason `issue`,
and its Watchdog pauses. The Headquarters Station reads new comments on the source, related and
child issues of unfinished Missions every `reply_every` (60 s), cursor per tracker instance in
the Registry. A comment is delivered only from someone allowed to answer for the issue: the
issue's author or reporter, a GitHub owner, member or collaborator, a GitLab project member at
the instance's `reply_min_access` (Developer) or above, a Jira assignee, or a
login in the instance's `reply_authors`. AI6's own comments are skipped, told by the mark AI6
leaves on what it writes (a hidden `<!-- ai6:… -->` on GitHub and GitLab, the comment property
`dev.coqui.ai6` on Jira) and never by their author, so one person's token works: that person's
own reply is delivered like anyone's. Any other author is a timeline event naming them, never
their text. The Station that holds the Mission delivers a
permitted reply as untrusted text in the same fence as an issue's body: a message to a live
Session, a resume of one that ended its turn while it waited. A stalled, parked or quarantined
Mission keeps the reply until it goes on; a queued or finished one is not told. The Session is
marked exposed (`issue:TRACKER`), which a later attempt inherits. One
question per issue may be open. Nobody answering within `reply_ttl_secs` (3 days) resumes the
Agent told so (`on_timeout = "resume"`), fails the Mission (`"fail"`) or keeps waiting
(`"wait"`); an operator who resumes or stops the Mission withdraws the question.

An Agent's close, reopen or assignee change, and a transition the instance does not list, wait
for [green ink](#green-ink) (kinds `issue-close` and `issue-assign`, governed under `strict`;
`licensed` lets `00` Agents close without asking). The entry waits `held` until its approval is
decided: approved, it is sent; refused or expired, it is refused.

What the Station writes by itself follows the Operation's [`updates`](operations.md#issues):
by default one **status comment** on the source issue, posted when the Mission starts working
and edited in place as it waits for input or is paused, at most once a minute, with the
instance's `status_labels` swapped and its `status_transitions` made when the issue offers
them (one it does not offer is a timeline event). It names the Mission's branch once it has one.
When the Mission ends the report is a new comment, so people are notified. Every word follows the Operation's cover policy.

**Creating, splitting and mirroring.** An Agent asks for issues with `issue_create`,
`issue_split` and `issue_mirror`; its text passes the same check as a comment, and it carries on
without waiting. By default a person approves each request (`create = "propose"`), or a class
authorisation `issue-create` approves one from an unexposed Session with no findings; `create =
"direct"` is a weakening that lets `field` and `00` Agents create at once, except from an exposed
Session, and `create = "off"` refuses them. A Mission may hold `max_created` (10) issues, those
waiting included, an Operation's Agents `max_created_per_day` (50), and a split may go
`max_split_depth` (2) levels below an issue a person made; past any of these the call is refused.
A title equal to an open linked issue, or to one AI6 created in the Operation in the last 30 days,
is a finding `duplicate`, which stops a class authorisation and never a person.

The Headquarters Station then creates what was approved through the outbox, ties each new issue
to the Mission, and the Agent is told the outcome with the new keys, as soon as a Session of the
Mission can hear it. A created issue never starts
work: every intake label of the Operation's instances is stripped, and an Agent cannot add one
later. The person who approves may tick **mark ready** (which needs `mission.create` too): the
Headquarters Station then adds the labels in that person's name (audit `issue.ready`), and intake,
vetting and the Chief of Staff decide what runs, as for any issue. When intake takes an issue AI6
created, its Mission is linked to the issue it was split from, read-only.

A child goes under its parent where the tracker can: a GitHub sub-issue; on GitLab a task under an
issue (a task takes no children); on Jira Cloud a sub-task
under a standard issue and a standard issue under an epic; on Server a sub-task. Where it cannot,
the child names its parent in its first line and the parent gets one comment listing the children;
a Jira child of a sub-task is created beside it and tied by `link_type` (`Relates`). A mirror copies
the title (Jira cuts it to 255 bytes), the body as quoted text under a header naming the original,
the labels the target has, and the priority through the target instance's `priority_map`. Assignees,
the issue type, components, sprints, custom fields, attachments and comment history are not carried,
and each dropped field is listed in the proposal and the result. The original gets a comment "Tracked
in URL", and a remote link where the tracker has them; both sides stay linked to the Mission, and
nothing is kept in sync afterwards.

**People.** Reading a Mission's links and posts needs `mission.read`. `issue.link`, `issue.comment`,
`issue.update` and `issue.create` on the Operation let a person write, from Mission detail,
`ai6ctl` or through Moneypenny's proposals, which they confirm:

```text
ai6ctl mission issues m-12 --operation G
ai6ctl issues comment github:jose-pr/ai6#12 --mission m-12 --operation G --body "Use the release branch."
ai6ctl issues link jira:GUA-7 --mission m-12 --operation G --relation related
ai6ctl issue-proposals list --operation G
ai6ctl issue-proposals approve ip-1 --operation G --mark-ready
```

No route calls a tracker: each writes records the Headquarters Station sends. A person's writes go
out under the Operation's token, with a footer naming them, and need no green ink: their action is
the permission. Their text is refused for a token shape or invisible characters, and keeps its links
and mentions. A comment on an issue with an open question is its answer, as an operator's text: the
Mission resumes, unexposed. A link a person makes is read once by the Headquarters Station; a foreign
or missing issue comes back unlinked, with the reason on the entry.

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
  sleeper's Agent or role (within its `stations`), each narrowed to that Station, and the
  window and the Station keep one Mission ID, so a failover never doubles a round. A Station's load does not
  exclude it (its round waits for room); a Station that has gone dark gets its round when it
  checks in again within the window. This is how the [Head of
  Station](stations.md#head-of-station) runs.
- **A standing Mission.** With `standing = true` the sleeper is one Mission (one per Station with
  `each_station`), `<operation>-<sleeper>-<n>`, and each window is a new attempt of it: the run
  `<handle>.<attempt>` records the window it answers (`window` on its Session record), and the
  board and scorecards show one Mission with a run history. A window that comes while an attempt
  is queued, held or running does not fire; it fires when the attempt settles if it is still the
  latest window and within `catch_up`, and is on the timeline as skipped otherwise.
  With `resume = true` as well, an attempt resumes the last attempt's provider Session when that
  attempt ran on the same handler, did not fail, and its provider gave it an ID, and starts a new
  Session otherwise, as it does when the provider refuses the resume; the timeline says which
  (`sleeper`, `resumed` or `started`, with the window). `resume` without `standing` is refused when
  the roster loads.
- **A failed attempt.** A failed attempt leaves the order standing: the next window fires as the
  next attempt, and the reason is in the attempt's outcome. The attempt of any Sleeper that fails
  is on the timeline (`sleeper`, `attempt-failed`, with the window and the reason, or a line saying
  the harness's words are in the outcome) and, with a provider Session, on the board (reason
  `order-attempt-failed`). It asks for no approval. Cancelling an attempt is not a failure, and leaves
  the order standing too; `ai6 sleeper revoke` stops it.
- **A stalled attempt.** The Station holding a standing Mission's attempt looks at each check-in:
  when the order's next window has come and the attempt is `stalled` (past its Agent's
  `stall_timeout`), has no live Session on the Station and no re-attach that may still bring one
  back, or did not start, the window supersedes it. The attempt fails with that reason, its harness
  stopped, audited (`sleeper.superseded`) and on the timeline (`sleeper`, `superseded`), and the
  window fires as the next attempt. It is not charged to the Agent. An attempt waiting for a person
  (`blocked` with a reason), a provider's limit (`parked`) or an operator's release (`quarantined`)
  is left alone, and its windows are recorded as skipped.
- **A missed window.** The latest window that has not fired fires once, late, while it is within
  `[registry.limits.sleepers] catch_up`: by default its own period, the time since the window before
  it; `none` keeps no limit. It is never more than one. Every other window that passes unfired is on
  the timeline when a later one comes (`sleeper`, `skipped`, with the window and why: not authorized,
  the scheduler was not running, the last attempt was in flight, no Station could take it, or older
  than its catch-up bound), up to `skipped_events` of them, the older ones as one event. A window
  before an order's first approval never fires and is not recorded. An edit withdraws the approval,
  and the approval given again may fire what the order missed once; a revoke ends what was owed.
- **Duplicates.** A Sleeper's windows share its title, and are never held as duplicates of each
  other; a person's brief with the same title is still a duplicate of an open window.
- **Fingerprint.** Every field the sleeper sets is part of what `authorize` approves, so
  adding `stations`, `each_station`, `standing` or `resume` withdraws the approval until it is
  given again; a sleeper that sets none of them keeps the approval it has.
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
the matching live provider Session. Set `[unex] url` and `token` in `ai6.cfg`; without a
token the Station publishes nothing. Events use a
bounded background worker and finite request timeouts, so a gateway outage does not block
the provider Session. Terminal bytes remain binary.
