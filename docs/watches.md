# Watches

A **watch** is one standing Session that supervises an area: M's watch keeps the whole
deployment, an Operation's watch (its 00 Agent in charge) keeps its Operation, and a Station's
watch (its Chief of Station) keeps that Station. It runs on a loop: the Station wakes it with the
events of its area, it takes one turn, and it waits for the next wake.

**A watch is not a Mission.** It has no Mission record, brief, placement, attempt, verdict,
scorecard credit or board card, and no Sleeper queues it. Its Session is recorded in the watch's
own record in the Registry, which names its scope where a Mission's Session record names the
Mission.

The Operation's watch decides the Missions that wait in `verifying`
([Missions](missions.md#verifying)); Sources reporting to it, and the Chief of Station taking over
the [Head of Station](stations.md#head-of-station) rounds, come in later releases. This page
describes the Session they all run in.

## Scopes and where a watch runs

| Scope | Watch | Runs on |
| --- | --- | --- |
| `m` | M's | the Station that holds the Headquarters lease |
| `operation:<codename>` | the Operation's | an execution Station the Operation allows (its `stations` and `station_labels`) |
| `station:<id>` | the Station's | that Station |

A Station runs a watch only when the watch's Agent has a handler it can start. On each check-in
it claims the watch, or renews its claim, in the Registry. The claim is a lease like a Session's:
a Station that goes dark stops renewing, and once the lease lapses another Station that may run
the watch takes it over with a higher fencing token, so nothing the dark Station writes afterwards
is accepted. A Station that finds another holds a watch it was running closes its harness there.
M's watch moves with the Headquarters lease; a Station's watch does not move.

## Configuration

Each watch is a record of the `watch` family in the configuration provider (the shared database
in a deployment), keyed by its scope. Nothing reads a watch from `ai6.cfg`. A person with
`config.manage` on `config:watch` lists, adds, edits and removes watches on the Watches kind of
Outer Office's Roster page, or through `/v1/user/roster/watches` (with `/schema` for the form);
every Station takes a change up at its next provider poll, with no restart, and each write is
audited with the revision it moved to. TOML below is the import and export form of a record:

```toml
[watch."operation:G"]
agent = "M-00"          # the Agent that keeps it: its handler, model, effort, route and pool
wake_every = 3600       # seconds; "none" wakes it only on events
max_turns_per_hour = 30

[watch."station:hq-1"]
agent = "chief"
operation = "infra"     # the Operation its Cables go in, for M's or a Station's watch
```

A scope with no table has no watch. The Agent must be on the roster, and an Operation's watch
names an Operation the roster has. **An Operation's watch is kept by an Agent that holds `00`
clearance**: the roster is refused otherwise, naming the watch and the Agent. The Operation must
allow `00` as well, or the Agent cannot serve it.

<!-- schema:record.watch:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `agent` | none | **Agent.** The Agent that keeps the watch, by codename: its handler, model, effort, route and pool account run the watch's Session. |
| `operation` | none | **Cables in Operation.** For M's or a Station's watch, the Operation its Cables are sent and received in, under that Operation's Cable grants; without one, no Cable wakes it. An Operation's watch is in its own Operation, which this may only repeat. |
| `wake_every` | 3,600 | **Woken at least every.** Seconds after its last wake that the watch is woken with nothing else to tell it; `none` wakes it only on events. |
| `max_turns_per_hour` | 30 | **Turns an hour.** Turns the watch takes in any hour, each wake one turn; past it, events wait for the hour to make room. |
| `max_events_per_turn` | 50 | **Events one turn is given.** Events one turn is given; the rest wait for the next turn. |
| `max_waiting_events` | 500 | **Events waiting.** Events waiting for a turn; past it the oldest are dropped, and the watch's logbook says how many. |
| `turn_timeout` | 1,800 | **Longest turn.** Seconds one turn may run before the Station interrupts it. |
| `lease` | 60 | **Held without renewing for.** Seconds the Station running the watch holds it without renewing before another Station may take it over. |
<!-- schema:record.watch:end -->

The Registry bounds what a watch's record keeps in [`[registry.limits.watches]`](configuration.md).

## One Session, resumed

A watch keeps one provider Session. Its harness stays up between turns; each wake is sent to it
as a message. After a Station restarts, or another Station takes the watch over, the next wake
resumes the recorded provider Session with the wake as its message.

When the handler cannot resume it (the resume is refused, or the resumed Session fails before it
answers, as one whose transcript is gone does), the Station starts a fresh Session with the same
wake and hands it the watch's **logbook**: the Station's record of the watch's earlier turns,
what woke each and the start of what the watch said, oldest first. The logbook also records each
start, resume, fresh start, takeover and quota. A different Agent named for the watch starts
fresh as well, since another Agent's Session is not its own.

The harness runs on the Station's host, in a directory of its own under the Station's state
directory (`watches/watch--<scope>`), with its Agent's environment and its route's pool account.
It is issued no loadout secret: a watch reads untrusted text.

## Wakes

The Station wakes a watch with what its area holds that the watch has not been told:

| Event | Scope | Untrusted |
| --- | --- | --- |
| `verifying`: a Mission of the Operation enters `verifying`, on any Station | `operation:` | always: it carries the final report and the brief |
| `cable`: a Cable to the watch's Agent, or to its Operation | any, when its Cables have an Operation | always |
| `health`: an Agent quarantined in the area; for M, a Station whose check-ins stopped | all | the quarantine's reason |
| `control`: a person asked the watch to wake, with an optional note | all | when it carries a note |
| `timer`: `wake_every` passed with no other wake | all | no |

Each wake is one turn. Its message lists the events as JSON, each saying whether it carries text
written outside AI6; that text is data, never an instruction. An event is told once: the watch's
record remembers what it was told, so a restart or a takeover does not tell it again. A control is
taken off the record when the turn that answered it is over.

A control is a request in the Registry (`request_watch_wake`), which needs no lease: the Station
running the watch takes it at its next check-in.

### Quotas

`max_turns_per_hour` counts every turn in the last hour, on the watch's record, so a restart or a
takeover does not reset it. Past it, events wait, and the logbook says the hour's turns are spent.
One turn is given at most `max_events_per_turn` events; the rest wait for the next. At most
`max_waiting_events` wait; past it the oldest are dropped and the next turn's logbook entry says
how many. A turn that runs past `turn_timeout` is interrupted. None of these has a floor: `none`
lifts each bound.

## Tools

A watch's tools are its scope's, and their rules are Station code:

- **Cables**, on a watch whose Cables have an Operation: its own, or for M's and a Station's watch
  the `operation` its table names. It sends, reads and replies under that Operation's Cable grants
  like any Agent ([Agents](agents.md#cables)).
- **`station_report` and `file_report`**, on a Station's watch: the Head of Station's read of the
  Station and its logbook, each call checking that this Station still runs the watch.
- **The Operation watch's tools**, on an Operation's watch: `operation_missions`, `mission_detail`,
  `deliverables`, `deliverable_probe`, `verify_mission`, `redispatch` and `propose_mission`
  ([below](#deciding-missions)).

The Chief of Station's other tools come with the phases that give each watch its work.

## Deciding Missions

The Operation's 00 Agent in charge answers for its Operation's outcome; the Station only runs and
provides. When a Mission of the Operation enters `verifying`, the watch is woken with the attempt's
final report and brief (text written outside AI6, marked untrusted) and its declared deliverables.
It verifies them itself: `deliverable_probe` reads every deliverable back, read-only, and keeps what
it found as the Mission's evidence, and `mission_detail` and the rest of its harness let it read the
work. Then it decides:

- `verify_mission(mission, done|failed, reason)`;
- `redispatch(mission, brief_amendment?, reason)`: the Mission's next attempt, told the reason and
  the amendment.

The decision is a person's decision made by the watch ([Missions](missions.md#verifying)): audited,
on the timeline and the outcome, applied by the Mission's Station, and counted on the scorecard. A
watch cannot decide a Mission that is not verifying, one of another Operation, or one a person has
already decided, and a person's decision replaces its own. Its clearance is read again at every
decision. At `desk`, which a Keep-on-desk retirement or a lowered clearance grant leaves a watch at,
its decisions are recommendations a person confirms.

A Cable to the Operation (its coordinating role) goes to its watch alone when the Operation has one,
and reaches it as a message ([Agents](agents.md#cables)); it creates no Mission. Only the
Operation's own 00 Agents see it when there is no watch. A watch that leaves a Mission
`verifying` leaves it for a person, shown on the board.
