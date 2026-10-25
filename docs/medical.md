# Medical: usage, exposure and quarantine

Medical watches the health of Agents. The [watchdog](watchdog.md) catches a Session that
stalls or loops. This page covers the rest: what Sessions consume, which Sessions have read
input an attacker could have written, and how an Agent that misbehaves is stopped
everywhere. Every setting here has a default and can be overridden; weakening a default is
a choice the configuration makes visible, never a silent one.

## Usage

Handlers report what the provider reports. Claude Code's `result` events carry token counts
and `total_cost_usd`; Codex reports tokens per model request (it reports no cost, so Codex
Sessions are limited by tokens only). The Station turns the reports into additions and keeps
one ledger per Mission in the shared Registry (`usage/<operation>/<mission>.json`), so every
Station sees the same spend and a resumed Session is never counted twice.

Measuring is always on. The Mission detail of Universal Exports shows the ledger:

```console
GET /v1/operations/G/missions/mission-7
{ "usage": { "total": { "input": 1200, "output": 340, "cache_read": 90000,
                         "cache_write": 6000, "cost_micro_usd": 48000 },
             "sessions": { "<session id>": { "...": 0 } },
             "daily_cost_micro_usd": { "2026-10-26": 48000 } } }
```

Money is an integer count of millionths of a dollar. A token budget counts input, output and
cache-write tokens; cache reads re-count the same context on every turn, so they stay
visible but are not counted against a limit.

## Budgets

Limits are set per Operation and per Agent (the Agent's keys layer over the Operation's, and
over the Agent it extends):

```toml
[operation.G.budget]
mission_cost_usd = 5.0
operation_cost_usd_per_day = 40.0 # a UTC day, summed over the Operation's Missions
warn_at = 0.8                     # fraction of a limit that raises a warning

[agent.007.budget]
session_tokens = 2000000
mission_tokens = 5000000
```

| Key | Default | Meaning |
| --- | --- | --- |
| `session_tokens` | unset | Tokens one Session may use |
| `mission_tokens` | unset | Tokens all of a Mission's Sessions may use |
| `mission_cost_usd` | unset | Dollars all of a Mission's Sessions may spend |
| `operation_cost_usd_per_day` | unset | Dollars an Operation's Missions may spend per UTC day |
| `warn_at` | `0.8` | Fraction (above 0, at most 1) of a limit that raises a warning |
| `preset` | none | `strict` sets conservative limits (2M tokens per Session, 5M per Mission, $10 per Mission, $50 per Operation per day); keys written beside it override it |

No limit is set unless you set one or choose `strict`: subscription quotas are already
handled by [parking](missions.md#provider-limits-park-and-resume). A negative, fractional
(for token counts) or non-numeric value fails the load with an error naming the key.

Crossing `warn_at` emits a `budget` event on the Mission and writes an audit record
(`budget.warn`). Reaching a limit writes `budget.limit`, interrupts the Session and moves the
Mission to `blocked` with `blocked_reason: "budget"`, never `failed`. The safe house and the
provider Session stay, so an operator who raises the limit can resume the same work. When a
Mission cost limit is set, Claude Code is also started with `--max-budget-usd` for what the
Mission may still spend, as a second stop.

## Exposure

A Session is **exposed** once it has read input someone else could have written:

| Source | When |
| --- | --- |
| `brief:source` | The Mission came from a [Source](missions.md#sources) item |
| `tool:NAME` | It called a tool the Operation lists (default `WebFetch` and `WebSearch`) |
| `cable:SENDER` | It read a Cable from another Operation or from an exposed Session |
| `review` | It read review text written by someone else |

Exposure is recorded in the Session's Registry record, never goes away, and is inherited by
a later attempt at the same Mission. Moneypenny shows an `EXPOSED` chip on the Session and
`ai6ctl` lists the sources. Marking is always on; what an exposed Session may then do is the
Operation's policy:

```toml
[operation.G.exposure]
preset = "strict"                      # default; "observe" only marks
restrict = ["proposals", "cables"]     # choose restrictions one by one (optional)
tools = ["WebFetch", "WebSearch", "Browse"]
```

| Restriction | Effect on an exposed Session |
| --- | --- |
| `proposals` | Mission proposals carry its exposure into the Mission they become, so they are never auto-promoted |
| `cables` | It may send a Cable only as a reply inside an existing conversation (`send_cable` is refused, naming the exposure) |
| `approvals` | Its requests for authorization always go to a human, never to a class authorization |
| `secrets` | It is given no new secret leases |

`strict` applies all four; `restrict` replaces that set. The Station enforces `proposals`,
`cables` and `approvals` (a [green ink](missions.md#green-ink) request from an exposed
Session waits for a person even when a class authorisation would cover it) itself;
`secrets` is carried by `ExposureGate::restricts` for the part of AI6 that issues leases to
ask.

## Quarantine

A quarantine stops an Agent in an Operation (or in every Operation). It is a record in the
shared Registry, so it takes effect on every Station:

- the Agent's running Sessions are interrupted and its Missions become `quarantined`, with
  their safe houses kept (never finished or cleaned up);
- Cables the Agent sends are held until an authorized release;
- the roster skips the Agent, so the next qualified Agent takes new briefs and queued Missions.

An Operation's settings trigger it automatically from the Agent's recent incidents in that
Operation:

```toml
[operation.G.quarantine]
stalls = 3          # stalls or loops within the window; 0 turns the trigger off
budget_stops = 2    # budget hard stops within the window; 0 turns it off
blown = true        # one blown cover
window_hours = 24
```

An operator can quarantine an Agent at any time:

```console
ai6 quarantine add 007 "writes outside its brief" --operation G
ai6 quarantine list
ai6 quarantine release 007 --operation G
```

or through Universal Exports (needs `agent.configure` on the Operation):

```console
POST /v1/user/operations/G/quarantine/007           { "reason": "..." }
GET  /v1/user/operations/G/quarantine
POST /v1/user/operations/G/quarantine/007/release
```

Only an operator releases a quarantine. Releasing it returns each stopped Mission to the
queue, assigned to the next qualified Agent when there is one; an operator can instead
abandon the Mission. A quarantine of every Operation (`ai6 quarantine add` without
`--operation`) is released with `ai6 quarantine release AGENT --all` on a Station, since no
single Operation's permission covers it.
