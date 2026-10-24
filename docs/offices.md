# Offices, failover and the store

M is a role, not a machine. Run `serve` on several theaters and each is an **office**; they
share one store, and exactly one of them is **M**, the office that decides. If M's
office is lost, another office takes the role over and nothing waiting is lost.

```bash
# headquarters: decides, and runs missions too
python -m coagent_m.watchdog serve --office hq-1 --role control --role execution
# a standby: takes over as M if hq-1 is lost
python -m coagent_m.watchdog serve --office hq-2 --role control
# workers: run missions, never decide
python -m coagent_m.watchdog serve --office lab-gpu --role execution --label gpu=yes
python -m coagent_m.watchdog serve --office win-1 --role execution --label os=windows
```

All of them point at the same store with `COAGENT_INBOX` (or `--inbox`).

## Roles

| Role | Does |
| --- | --- |
| `control` | may be elected M: polls intake, places briefs on offices, recovers what lost offices held, enforces station and profile budgets across all offices |
| `execution` | runs the missions placed on it, in its theatres and stations |

The default is both, so one office alone is a complete M. Election is a leader lease
the office keeps renewing; when it lapses, the next `control` office takes it.

## Placement

M places each waiting brief on a live `execution` office with room:

- whose **labels** match the station's `selector` (as a Kubernetes `nodeSelector`;
  values may be globs). Every office is labelled `office`, `host` and `os`, plus any
  `--label` given;
- that can reach the station's checkout: a station on a remote theater can run from any
  office, a local checkout only from an office on that machine;
- least loaded first.

```toml
[station.G]
path = "~/devel/guatiao"
selector = { gpu = "yes", os = "linux" }
```

## When an office is lost

Every claim is a **lease** its office renews with each heartbeat. An office that stops
— a crash, a lost theater, a cut network — stops renewing, its leases lapse, and M puts
those briefs back in the queue for another office: up to `--max-attempts` times
(default 3), after which the brief is filed as failed with state `lost`. An office that
comes back and finds its lease taken stops that mission rather than run it twice. A
kept worktree branch from the first attempt is still there for the next.

Leases compare wall clocks, so offices need their clocks in step (NTP). `--lease`
(default 60 s) is both the lease and how quickly a lost office is noticed.

Today an agent runs as a child of its office, so an office's loss ends its agents and
the mission is re-run. Sessions that outlive their office, so another office can attach
to a still-running agent, are the next step.

## The store

| `COAGENT_INBOX` | Store | For |
| --- | --- | --- |
| a directory (default `~/.local/state/coagent/inbox`) | files, renamed atomically | one machine, or offices sharing a directory |
| `sqlite:///path/inbox.db` | SQLite (`coagent-i[sql]`) | one machine |
| `postgresql+psycopg://user@db/coagent` | PostgreSQL (`coagent-i[postgres]`) | offices on different theaters |

Any database SQLAlchemy supports works the same way. Every step two offices could race
on — claiming a brief, renewing a lease, recovering it, filing it, taking the leader
lease — is one conditional update, so only one office ever wins it.

## Moneypenny

Moneypenny, the board and terminal gateway, is a secretary, not a decision maker: it
holds no state of its own, reads the shared store, and so any number can run at once
behind a load balancer, one per office or anywhere else. Attaching to an agent's
terminal is routed to the office running that session. It will run inside `serve` (a
`--moneypenny` option) once the gateway exists.
