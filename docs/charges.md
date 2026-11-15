# Charges

A **charge** is one standing Run that supervises an area: M's charge keeps the whole
deployment, an Operation's charge (its 00 Agent in charge) keeps its Operation, and a Station's
charge (its Chief of Station) keeps that Station. It runs on a loop: the Station wakes it with the
events of its area, it takes one turn, and it waits for the next wake.

A charge owns its area and its job is to keep it working: it is told what is stuck, fixes what its
tools allow there, and tells a person what it cannot ([Keeping the area working](#keeping-the-area-working)).

**A charge is not a Mission.** It has no Mission record, brief, placement, run, verdict,
scorecard credit or board card, and no Sleeper queues it. Its Run is recorded in the charge's
own record in the Registry, which names its scope where a Mission's Run record names the
Mission.

The Operation's charge decides the Missions that wait in `verifying`
([Missions](missions.md#verifying)) and what the Operation's [Sources](#source-items) report. The
Station's charge, the [Chief of Station](stations.md#chief-of-station), keeps its Station fit and
provides what the Operations ask of it. [M's charge](#m-and-the-board) watches the whole deployment.
This page describes the Run they all run in.

## Scopes and where a charge runs

| Scope | Charge | Runs on |
| --- | --- | --- |
| `m` | M's | the Station that holds the Headquarters lease |
| `operation:<codename>` | the Operation's | an execution Station the Operation allows (its `stations` and `station_labels`) |
| `station:<id>` | the Station's | that Station |

A Station runs a charge only when the charge's Agent has a handler it can start. On each check-in
it claims the charge, or renews its claim, in the Registry. The claim is a lease like a Run's:
a Station that goes dark stops renewing, and once the lease lapses another Station that may run
the charge takes it over with a higher fencing token, so nothing the dark Station writes afterwards
is accepted. A Station that finds another holds a charge it was running closes its harness there.
M's charge moves with the Headquarters lease; a Station's charge does not move.

## Configuration

Each charge is a record of the `charge` family in the configuration provider (the shared database
in a deployment), keyed by its scope. Nothing reads a charge from `ai6.cfg`. A person with
`config.manage` on `config:charge` lists, adds, edits and removes charges on the Charges kind of
Outer Office's Roster page, or through `/v1/user/roster/charges` (with `/schema` for the form);
every Station takes a change up at its next provider poll, with no restart, and each write is
audited with the revision it moved to. TOML below is the form `ai6ctl get` writes and `ai6ctl apply` reads:

```toml
[charge."operation:G"]
agent = "M-00"          # the Agent that keeps it: its handler, model, effort, route and pool
wake_every = 3600       # seconds; "none" wakes it only on events
max_turns_per_hour = 30

[charge."station:hq-1"]
agent = "chief"
operation = "infra"     # the Operation its Cables go in, for M's or a Station's charge
```

A scope with no table has no charge. The Agent must be on the roster, and an Operation's charge
names an Operation the roster has. **An Operation's charge is kept by an Agent that holds `00`
clearance**: the roster is refused otherwise, naming the charge and the Agent. The Operation must
allow `00` as well, or the Agent cannot serve it.

<!-- schema:record.charge:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `agent` | none | **Agent.** The Agent that keeps the charge, by codename: its handler, model, effort, route and pool account run the charge's Run. |
| `operation` | none | **Cables in Operation.** For M's or a Station's charge, the Operation its Cables are sent and received in, under that Operation's Cable grants; without one, no Cable wakes it. An Operation's charge is in its own Operation, which this may only repeat. |
| `wake_every` | 3,600 | **Woken at least every.** Seconds after its last wake that the charge is woken with nothing else to tell it, while its area has open work (a Mission that is not over, a Report not yet assessed, a request waiting); `none` wakes it only on events. |
| `wake_every_idle` | none | **Woken at least every, when idle.** As `wake_every`, while its area has no open work; `none` takes no timer turn then. Events still wake the charge. |
| `max_turns_per_hour` | 30 | **Turns an hour.** Turns the charge takes in any hour, each wake one turn; past it, events wait for the hour to make room. |
| `max_events_per_turn` | 50 | **Events one turn is given.** Events one turn is given; the rest wait for the next turn. |
| `max_waiting_events` | 500 | **Events waiting.** Events waiting for a turn; past it the oldest are dropped, and the charge's logbook says how many. |
| `turn_timeout` | 1,800 | **Longest turn.** Seconds one turn may run before the Station interrupts it. |
| `lease` | 60 | **Held without renewing for.** Seconds the Station running the charge holds it without renewing before another Station may take it over. |
| `stall_after` | 1,800 | **Run stalled after.** For a Station's charge: seconds a Run of that Station may make no progress before the charge is told it is stalled and may surrender its claim; `none` leaves it to the Run's own stall judgement. |
| `poll_frozen_after` | 600 | **Source poll frozen after.** For M's charge: seconds a Source poll may run, or a Source stay unpolled past its interval, before the charge is told the poll is frozen; `none` never flags one. |
| `stuck_run_lost_after` | 600 | **Run lost after.** Seconds a Mission may stand `working` (or be claimed) with no Run holding it, because its Run's lease lapsed or ended without settling it, before the charge is told it is stuck; `none` never flags one. |
| `stuck_verifying_after` | 3,600 | **Verifying stuck after.** Seconds a Mission may stand `verifying`, counted from when the Station first saw it so, before the charge is told it is stuck; `none` never flags one. |
| `stuck_queued_after` | 900 | **Queued stuck after.** Seconds a Mission may wait in the queue before the charge is told it is stuck; `none` never flags one. |
| `stuck_blocked_after` | 3,600 | **Blocked or held stuck after.** Seconds a Mission may stand `blocked` or `held`, counted from when the Station first saw it (a held Mission, from when its Run ended), before the charge is told it is stuck; `none` never flags one. |
| `stuck_charge_after` | 600 | **Charge stuck after.** For M's charge: seconds another charge may stand with its lease lapsed, or its last turn failed, before M is told it is stuck; `none` never flags one. |
| `max_text_chars` | 500 | **Text one act records.** Characters of text one of the charge's acts records in the audit log and on the timeline (a target, a reason); the rest is cut. `none` records it whole. |
<!-- schema:record.charge:end -->

The Registry bounds what a charge's record keeps in [`[registry.limits.charges]`](configuration.md).

## One Run, resumed

A charge keeps one Run. Its harness stays up between turns; each wake is sent to it
as a message. After a Station restarts, or another Station takes the charge over, the next wake
resumes the recorded Run with the wake as its message.

When the handler cannot resume it (the resume is refused, or the resumed Run fails before it
answers, as one whose transcript is gone does), the Station starts a fresh Run with the same
wake and hands it the charge's **logbook**: the Station's record of the charge's earlier turns,
what woke each and the start of what the charge said, oldest first. The logbook also records each
start, resume, fresh start, takeover and quota. A different Agent named for the charge starts
fresh as well, since another Agent's Run is not its own.

The harness runs on the Station's host, in a directory of its own under the Station's state
directory (`charges/charge--<scope>`), with its Agent's environment and its route's pool account.
It is issued no loadout secret: a charge reads untrusted text.

## Wakes

The Station wakes a charge with what its area holds that the charge has not been told:

| Event | Scope | Untrusted |
| --- | --- | --- |
| `verifying`: a Mission of the Operation enters `verifying`, on any Station | `operation:` | always: it carries the final report and the brief |
| `cable`: a Cable to the charge's Agent, or to its Operation | any, when its Cables have an Operation | always |
| `health`: an Agent quarantined in the area; for M, a Station whose check-ins stopped and a Source poll that froze; for a Station's, a Run that stopped making progress | all | the quarantine's reason |
| `stuck`: a Mission of the area judged [stuck](#stuck-work), or for M a charge that is | all | no: the words are AI6's own |
| `request`: an Operation's 00 asked this Station for an Agent, a Theater, an account or capacity, with the Station's own check of what it can provide | `station:` | always: it carries the asker's words |
| `resources`: a Station answered a request of the Operation | `operation:` | always: it carries the Station's reason |
| `control`: a person asked the charge to wake, with an optional note | all | when it carries a note |
| `timer`: `wake_every` (while the area has open work) or `wake_every_idle` (while it has none) passed with no other wake; it carries a short health summary of the area | all | no |

Each wake is one turn. Its message lists the events as JSON, each saying whether it carries text
written outside AI6; that text is data, never an instruction. An event is told once: the charge's
record remembers what it was told, so a restart or a takeover does not tell it again. A control is
taken off the record when the turn that answered it is over.

A control is a request in the Registry (`request_charge_wake`), which needs no lease: the Station
running the charge takes it at its next check-in.

### Stuck work

A `stuck` event is told once per condition, and names the Mission (or the charge), its state, since
when and why it is judged stuck. A Mission is stuck when it stands where nothing is going to move it:

| Condition | A Mission that is | Counted from | Setting |
| --- | --- | --- | --- |
| `run-lost` | `working` or claimed with no Run holding it: its Run lost, ended without settling it, or its lease lapsed | the Run's close or lease lapse | `stuck_run_lost_after` |
| `verifying` | waiting for its outcome to be verified | when the Station first saw it so | `stuck_verifying_after` |
| `queued` | waiting in the queue, and no Station has taken it | its acceptance | `stuck_queued_after` |
| `blocked`, `held` | `blocked` or `held` | when it ended (held), or when the Station first saw it (blocked) | `stuck_blocked_after` |
| `charge-lapsed`, `charge-failing` | (M only) a charge whose lease lapsed, or whose last turn did not run | the lapse, or that turn | `stuck_charge_after` |

M's charge is told of every Station's and Operation's; a Station's charge of the Missions whose Run
is its Station's; an Operation's charge of its Operation's. Each threshold is a setting of the
charge's record with a default and no floor (`0` flags at once); `none` turns the condition off. A
Mission is told once per Run: a new Run, or a Mission that comes back to the condition under another
Run, is told again. The Registry does not date a `blocked` or `verifying` Mission still being held, so
the Station counts from when it first saw it; a restarted Station counts again.

### The timer

`wake_every` applies while the charge's area has open work: a Mission that is not over, a Report not
yet assessed or a request waiting. `wake_every_idle` applies while it has none, and by default is
`none`: an idle charge takes no timer turn and events still wake it. The timer's event carries a
short health summary of the area (Missions by state, the oldest one open, Reports waiting), so a
quiet turn has something to check.

### Quotas

`max_turns_per_hour` counts every turn in the last hour, on the charge's record, so a restart or a
takeover does not reset it. Past it, events wait, and the logbook says the hour's turns are spent.
One turn is given at most `max_events_per_turn` events; the rest wait for the next. At most
`max_waiting_events` wait; past it the oldest are dropped and the next turn's logbook entry says
how many. A turn that runs past `turn_timeout` is interrupted. None of these has a floor: `none`
lifts each bound.

## Attaching to a charge's Run

A person can attach to a charge's Run, as with a harness running in tmux that someone connects a
terminal to. Attaching is to the charge's own live Run, not a copy and not a queued question: they
are given the Run's history, then what it shows from then on (its messages, tool calls, plans and
permission requests), and may type into it as its user, answer what it asks and interrupt it. Several
people may be attached at once, each prompt is stamped with who sent it, and detaching leaves the Run
running. It works for every handler, a JSON one such as `claude` as much as an `acp` agent, because the
Station already drives each Run over ACP; a `pty` Run's terminal view stays the
[terminal attach](terminals.md) it has always had.

What each needs, on the charge's scope:

| To | Action | On |
| --- | --- | --- |
| attach, type, answer | `charge.converse` | `operation:<codename>`, `station:<id>`, or `config:charge` for M's charge |
| interrupt the turn under way | `mission.control` on an Operation's charge; `station.configure` on a Station's or M's | the same scope |

`charge.converse` is in no seeded role: the chief holds it, and a role grants it on purpose. It is
checked again for every prompt, answer and interrupt, so a role withdrawn after attaching applies at
once. Attaching, detaching, every prompt, answer and interrupt are audited before they are passed on
(`charge.attach`, `charge.detach`, `charge.prompt`, `charge.answer`, `charge.interrupt`, naming the
person, never the text of a prompt).

A prompt is one turn of the Run's own provider session: no new Run starts and no Mission is made, and
the Station's logbook names who prompted it. If a wake's turn is running, the prompt waits behind it
and runs when it ends, never inside it; a prompt does not use up the hour's `max_turns_per_hour`. A
request for leave that an ACP agent makes reaches the attached people, and the first answer to it
decides; with nobody attached, it is decided as it would have been (a restricted Run refuses). What the
Run shows is kept per Run, in the Station's memory, up to the `[attach]` `history_frames` setting; a
Station that restarts has its Runs' history begin again, and the people attached to them, who are not told, attach again.

### The socket

`GET /v1/socket/attach` on Universal Exports is a WebSocket of text frames. The first, within
`[attach] hello_timeout`, is `{"type":"hello","token":ACCESS_TOKEN,"charge":"operation:G"}` (add
`"after":SEQ` to be replayed only what follows a frame you already have). Then `{"type":"prompt",
"text"}`, `{"type":"answer","request":N,"option":"allow"|"refuse"}`, `{"type":"interrupt"}` and
`{"type":"detach"}`. Back come `{"type":"attached"}`, a `{"type":"frame","frame":{seq,at,kind,data}}`
for each thing the Run shows (`event`, `prompt`, `turn`, `permission`, `answered`, `attached`,
`detached`, `interrupted`, `ready` when the replay is over, `closed`, `error`) and
`{"type":"error","code","message"}` for what was refused. `ai6ctl charge attach SCOPE` is a line-based
client of it, and Outer Office and other apps can be others.

**Outer Office** shows charges on its Charges page (Machines in the rail): one card for M's, each Operation's
and each Station's with its Run's state (running while a Station holds its lease, stopped while none does),
the Agent and its route, the Station it runs on, when it last woke and what that turn recorded. Its detail
has a Live tab (an **Open in chat** button that opens the chat panel talking to the charge's Run; the
panel also lists every charge one may converse with under **Talking to**. The attachment above is drawn as
Moneypenny's chat is, with the same message bubbles, tool folding, notices and box: the history, other
people's messages named and one's own as "You", the Agent's messages under its codename, a turn's tool calls
folded into one "Sources checked" row, a permission request as a card answered in place with the first
answer deciding, and Interrupt where the chat has Stop, only for a person who holds the action; talking to
Moneypenny again detaches, and a dropped socket offers Reconnect from the last frame), Decisions (what the charge said at each
wake and, for an Operation's, the Reports it assessed) and Logbook. A Mission's detail has a Live Run row with the same **Open in chat**
under `mission.control`. The list reads `GET /v1/user/charges`, which also says which controls the person
holds; the logbook needs `charge.converse`. A charge whose last turn ended failed, or whose lease lapsed so
that no Station holds it, is in the attention bell and on Overview (`GET /v1/user/attention`), and
Moneypenny's `charges` tool says what each charge last did and offers its page.

The gateway resolves the person's token again for every prompt, answer and interrupt, and looks again
every `[attach] recheck_every` seconds (and when the sign-in ends, if that is sooner) at whether it
stands and still holds `charge.converse`; a person whose sign-in ended or was revoked, or whose role was
withdrawn, is told and the socket is closed. A person who falls more than `backlog` frames behind is let
go and attaches again. A prompt of up to `prompt_bytes` always fits a frame, however it is escaped
(`frame_slack` is the room beyond six bytes per prompt byte); a larger frame is answered with a
`bad_request` error and the socket stays open, up to `socket_bytes`, past which the socket closes.

The Station serves its Runs through the `RunHub` trait
(`ai6::directorates::section_00::hub`); an implementation that keeps the Run and its clients
elsewhere can replace the Station's without the gateway or its clients changing. Universal Exports
queues what people ask for each Station, which takes it with its next poll
(`GET /v1/station/hub/next`) and delivers what the Run shows to
`POST /v1/station/hub/frames`.

| Setting | Where | Default |
| --- | --- | --- |
| `history_frames`, `clients`, `client_backlog`, `waiting_prompts`, `permission_wait`, `delivery_frames`, `delivery_attempts`, `command_wait`, `retry_delay`, `call_margin`, `poll_slice`, `pause_slice` | the `station` record's `[attach]` | see [Configuration](configuration.md) |
| `clients`, `queue`, `backlog`, `prompt_bytes`, `frame_slack`, `socket_bytes`, `hello_timeout`, `max_wait`, `poll_wait`, `recheck_every` | the `unex` record's `[attach]` | see [Configuration](configuration.md) |

None has a floor; `none` lifts each bound that is a count or a time limit.

## Tools

A charge's tools are its scope's, and their rules are Station code:

- **Cables**, on a charge whose Cables have an Operation: its own, or for M's and a Station's charge
  the `operation` its table names. It sends, reads and replies under that Operation's Cable grants
  like any Agent ([Agents](agents.md#cables)).
- **The area tools**, on every charge, each acting in the charge's own scope
  ([below](#keeping-the-area-working)): `board`, `settle_mission`, `retry_mission`,
  `resume_mission`, `cancel_mission`, `wake_charge` and `notify_person`, and for a Station's charge
  `mission_detail`.
- **The Station charge's tools**, on a Station's charge: `station_report`, `file_report`,
  `tidy_safe_house`, `provision_theater`, `surrender_claim` and `answer_request`, each call checking
  that this Station still runs the charge ([Stations](stations.md#chief-of-station)).
- **The Operation charge's tools**, on an Operation's charge: `operation_missions`, `mission_detail`,
  `deliverables`, `deliverable_probe`, `verify_mission`, `redispatch` and `propose_mission`
  ([below](#deciding-missions)), and `request_resources` to ask a Station for what the Operation
  needs to run ([Stations](stations.md#what-an-operation-asks-of-a-station)).
- **M's decision tools**, on M's charge: `mission_detail`, `deliverable_probe`, `verify_mission` and
  `redispatch` for a Mission of any Operation, and **`raise_flag`** ([below](#m-and-the-board)).

## Keeping the area working

A charge owns its area. M's charge acts across all of AI6 (every Operation and Station), a Station's
charge in its Station, an Operation's charge in its Operation. Each tool is audited before it acts,
with the charge as the actor (`charge:m`, `charge:station:<id>`, `charge:operation:<codename>`); an
audit record that cannot be written refuses the act. Each also checks that the Station still holds
the charge's lease. A Mission outside the charge's scope answers as one that does not exist.

| Tool | What it does | M | Station | Operation |
| --- | --- | --- | --- | --- |
| `board` | The Missions of the area by state, what is stuck, the Reports waiting and, for M, the charges | all | its Station's Runs | its Operation |
| `mission_detail` | One Mission: its state and Runs; M also reads the brief and report (untrusted) | yes | state only | yes |
| `settle_mission` | Files a Mission nothing holds as `failed` (its Run results and its brief, taken from the queue or recovered) or `stalled`, with the reason | yes | yes | yes |
| `retry_mission` | Queues its next run: through the operator control for a Mission the Station holds, otherwise by recovering the brief of a dark Station's Mission, bounded by the run limit; a named Agent must be on the roster and not quarantined | yes | yes | yes |
| `resume_mission`, `cancel_mission` | Resumes the Run, or withdraws the Mission, of one the Station holds (cancel asks the Registry otherwise) | yes | yes | yes |
| `wake_charge` | Wakes another charge, with an optional note | any | Operations whose Missions it holds | Stations that hold its Missions, with no note or an AI6 code only |
| `notify_person` | Files a Report in the charge's scope for a person, saying what it found, did and could not do | yes | yes | yes |
| `verify_mission`, `redispatch` | Decide a Mission waiting for its outcome | any Operation | never | its Operation |

Outcomes are an Operation's: its 00 is the first to verify its own Missions. A Station's charge
decides none, and a `settle_mission` on a Mission waiting for its outcome is refused; with no
Operation charge it says so with `notify_person`. M may decide any Mission, as it may act on anything,
and does so as `charge:m`; a person's decision replaces a charge's. A Station's charge reads state, not
an Operation's brief or report, since those cross an Operation.

Text does not cross an Operation unchecked. A `stuck` event carries AI6's own words only (a condition, a
state, a time), never the reason an Agent or another charge gave; a Station's charge is shown no Operation's
reasons, in events or in `board` and `mission_detail`. A note M says it read in one Operation reaches
another Operation's charge only through a compartment of the first that allows sending there
(`about_operation`), checked and audited before it is delivered. `settle_mission` refuses, before it audits or
changes anything, a Mission whose Station's lease on its brief has not lapsed, and every tool refuses once the
charge no longer holds its lease or its audit record cannot be written. `cancel_mission` withdraws a queued
Mission from the queue and settles one whose Station is dark. The text one act records in the audit log is
cut to `max_text_chars`.

A fix a charge cannot make is reported with `notify_person`, not left unsaid. The standing context of
each kind says this is its job, names its tools and says what it never does.

## Deciding Missions

The Operation's 00 Agent in charge answers for its Operation's outcome; the Station only runs and
provides. When a Mission of the Operation enters `verifying`, the charge is woken with the run's
final report and brief (text written outside AI6, marked untrusted) and its declared deliverables.
It verifies them itself: `deliverable_probe` reads every deliverable back, read-only, and keeps what
it found as the Mission's evidence, and `mission_detail` and the rest of its harness let it read the
work. Then it decides:

- `verify_mission(mission, done|failed, reason)`;
- `redispatch(mission, brief_amendment?, reason)`: the Mission's next run, told the reason and
  the amendment.

The decision is a person's decision made by the charge ([Missions](missions.md#verifying)): audited,
on the timeline and the outcome, applied by the Mission's Station, and counted on the scorecard. A
charge cannot decide a Mission that is not verifying, one of another Operation, or one a person has
already decided, and a person's decision replaces its own. Its clearance is read again at every
decision. At `desk`, which a Keep-on-desk retirement or a lowered clearance grant leaves a charge at,
its decisions are recommendations a person confirms.

A Cable to the Operation (its coordinating role) goes to its charge alone when the Operation has one,
and reaches it as a message ([Agents](agents.md#cables)); it creates no Mission. Only the
Operation's own 00 Agents see it when there is no charge. A charge that leaves a Mission
`verifying` leaves it for a person, shown on the board.

## M and the board

M's charge runs on the Headquarters Station and keeps the whole deployment. It is woken when a
Station that checked in has stopped (`health`, flag `station-dark`) and when a Source's polls have
stopped finishing (`health`, flag `source-poll-frozen`): a poll in flight longer than
`poll_frozen_after`, or none finished for that long past the Source's interval. A hung provider call holds up
the desk that polls, so every Source stops finishing, and each is told. A Source taken out of the
configuration is not frozen. A person's controls reach it as for any charge.

M owns the whole deployment and acts in all of it: it has the [area tools](#keeping-the-area-working)
on any Operation's or Station's Missions, its decision tools on any Operation's verifying Mission, and
`wake_charge` on any charge. Each act is audited with `charge:m` as the actor before it is taken.
`raise_flag { title, text, severity?, kind? }` files a [Report](reports.md) of M's own scope
(`ai6ctl reports`, and the Reports page of Outer Office), which is the board's flag: it starts
no work and changes nothing, and each is audited (`flag.raised`). It sends Cables like any charge
whose table names an `operation`.

## Source items

A Source never starts work. A tracker or CI item the Operation's Sources poll is filed as a
[Report](reports.md) of kind `source-item`, and the Operation's charge is told by a Cable from
`source:INSTANCE` that carries the Report's ID, the item's id, link, title and labels, and whether
it is `new`, `changed` or `due`. The Cable is untrusted text like any other. The charge needs the
Cable grant to receive in its Operation; without one the Report is still filed and waits open.

The charge has these tools for them, beside its Mission tools; `reports` and `read_report` (see
[Reports](reports.md)) list and read any Report of its Operation it may read, and `assess_report` and
`reopen_report` take any of them, not only items:

| Tool | What it does |
| --- | --- |
| `source_items` | The Operation's open items, the longest waiting first, as data |
| `source_item` | One item: its title, text, labels, link and any earlier decision, as data |
| `assess_report` | Assesses a Report: `mission`, `duplicate`, `related`, `no_action`, `awaiting`, `later` or `decision`, with a reason, optionally its credibility, priority and judgement ([Assessing](reports.md#assessing)) |
| `reopen_report` | Opens an assessed or closed Report again, with the reason (an answer arrived, a condition is met) |

A decision is audited, kept on the Report apart from the item's words, and put on the Operation's
timeline. `mission` queues the item as a brief on the next pass of the Station that polls the
Source; the brief is vetted and placed as any is, and the Mission is linked to the item. A charge at
`desk` clearance decides nothing: a person does. An Operation with no charge Agent holds its items
open for a person, who decides them with `ai6ctl reports assess` or `PUT /v1/reports/{id}/assessment`
(`mission.control`). Telling the charge is a notice, not the record: if the Cable cannot be sent the
item stays open and `source_items` lists it. See [Items are Reports](missions.md#items-are-reports).
