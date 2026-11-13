# Charges

A **charge** is one standing Run that supervises an area: M's charge keeps the whole
deployment, an Operation's charge (its 00 Agent in charge) keeps its Operation, and a Station's
charge (its Chief of Station) keeps that Station. It runs on a loop: the Station wakes it with the
events of its area, it takes one turn, and it waits for the next wake.

**A charge is not a Mission.** It has no Mission record, brief, placement, run, verdict,
scorecard credit or board card, and no Sleeper queues it. Its Run is recorded in the charge's
own record in the Registry, which names its scope where a Mission's Run record names the
Mission.

The Operation's charge decides the Missions that wait in `verifying`
([Missions](missions.md#verifying)) and what the Operation's [Sources](#source-items) report; the
Chief of Station taking over the [Head of Station](stations.md#head-of-station) rounds comes in a
later release. This page describes the Run they all run in.

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
| `wake_every` | 3,600 | **Woken at least every.** Seconds after its last wake that the charge is woken with nothing else to tell it; `none` wakes it only on events. |
| `max_turns_per_hour` | 30 | **Turns an hour.** Turns the charge takes in any hour, each wake one turn; past it, events wait for the hour to make room. |
| `max_events_per_turn` | 50 | **Events one turn is given.** Events one turn is given; the rest wait for the next turn. |
| `max_waiting_events` | 500 | **Events waiting.** Events waiting for a turn; past it the oldest are dropped, and the charge's logbook says how many. |
| `turn_timeout` | 1,800 | **Longest turn.** Seconds one turn may run before the Station interrupts it. |
| `lease` | 60 | **Held without renewing for.** Seconds the Station running the charge holds it without renewing before another Station may take it over. |
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
| `health`: an Agent quarantined in the area; for M, a Station whose check-ins stopped | all | the quarantine's reason |
| `control`: a person asked the charge to wake, with an optional note | all | when it carries a note |
| `timer`: `wake_every` passed with no other wake | all | no |

Each wake is one turn. Its message lists the events as JSON, each saying whether it carries text
written outside AI6; that text is data, never an instruction. An event is told once: the charge's
record remembers what it was told, so a restart or a takeover does not tell it again. A control is
taken off the record when the turn that answered it is over.

A control is a request in the Registry (`request_charge_wake`), which needs no lease: the Station
running the charge takes it at its next check-in.

### Quotas

`max_turns_per_hour` counts every turn in the last hour, on the charge's record, so a restart or a
takeover does not reset it. Past it, events wait, and the logbook says the hour's turns are spent.
One turn is given at most `max_events_per_turn` events; the rest wait for the next. At most
`max_waiting_events` wait; past it the oldest are dropped and the next turn's logbook entry says
how many. A turn that runs past `turn_timeout` is interrupted. None of these has a floor: `none`
lifts each bound.

## Tools

A charge's tools are its scope's, and their rules are Station code:

- **Cables**, on a charge whose Cables have an Operation: its own, or for M's and a Station's charge
  the `operation` its table names. It sends, reads and replies under that Operation's Cable grants
  like any Agent ([Agents](agents.md#cables)).
- **`station_report` and `file_report`**, on a Station's charge: the Head of Station's read of the
  Station and its logbook, each call checking that this Station still runs the charge.
- **The Operation charge's tools**, on an Operation's charge: `operation_missions`, `mission_detail`,
  `deliverables`, `deliverable_probe`, `verify_mission`, `redispatch` and `propose_mission`
  ([below](#deciding-missions)).

The Chief of Station's other tools come with the phases that give each charge its work.

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
