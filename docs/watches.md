# Watches

A **watch** is one standing Session that supervises an area: M's watch keeps the whole
deployment, an Operation's watch (its 00 Agent in charge) keeps its Operation, and a Station's
watch (its Chief of Station) keeps that Station. It runs on a loop: the Station wakes it with the
events of its area, it takes one turn, and it waits for the next wake.

**A watch is not a Mission.** It has no Mission record, brief, placement, attempt, verdict,
scorecard credit or board card, and no Sleeper queues it. Its Session is recorded in the watch's
own record in the Registry, which names its scope where a Mission's Session record names the
Mission.

What each watch decides comes in later releases: the Operation's watch verifying Missions that
wait in `verifying` ([Missions](missions.md#verifying)), Sources reporting to it, and the Chief of
Station taking over the [Head of Station](stations.md#head-of-station) rounds. This page describes the Session
they all run in.

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

Each watch is a `[watch.<scope>]` table of the `watch` family (the `watches` file, or the `watch`
tables of `ai6.cfg`):

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
names an Operation the roster has.

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
| `verifying`: a Mission of the Operation enters `verifying`, on any Station | `operation:` | when it names deliverables |
| `cable`: a Cable to the watch's Agent | any, when its Cables have an Operation | always |
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

Deciding verifying Missions, proposing Missions and the Chief of Station's other tools come with
the phases that give each watch its work.
