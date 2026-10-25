# Organization

AI6 borrows its names from the Secret Intelligence Service of the James Bond novels and
films, and from real public intelligence vocabulary where Bond has no word. The names
are a convention, not decoration: each one says who owns a decision, and the fiction's
rules turn out to be good rules for running coding agents.

## Directorates and posts

| Name | Here | Owns |
| --- | --- | --- |
| **M** | `ai6::directorates::m` | Decisions: the roster, Agent selection, clearance, cover, the Mission lifecycle and the access kinds (shared `coqui-access`, with AI6's scopes and actions) |
| **Registry** | `ai6::directorates::registry` | The shared record (briefs, leases, check-ins, audit, access tokens) and the Cable relay; real SIS and MI5 usage |
| **Q Branch** | `ai6::directorates::q` | Gear: terminals, loadouts, Mission-scoped secret leases, MCP configuration, sandbox arguments |
| **00 Section** | `ai6::directorates::section_00`, `crates/providers/agents/` | The AgentHandler contract and the Claude and Codex handlers |
| **Medical** | `ai6::directorates::medical` | Agent health: the watchdog for stalls, loops and blocked prompts |
| **Station** | `ai6::station`, `ai6 serve` | A node running the daemon: its server, provider host, safe houses, publisher and identity |
| **Universal Exports** | `crates/universal-exports` | The public front: the only address operators and clients reach |
| **Moneypenny** | `clients/moneypenny` | The operator app: board, briefs, terminals, administration, audit |
| **DipTel** | `crates/ai6-diptel` | The wire contract for Session lifecycle and terminal frames |

Directorates decide; the Station is where their decisions run. In the novels Universal
Export is the plate on the door of the London headquarters, the one address outsiders
use, which is the gateway's job; Moneypenny is who operators actually deal with.

## Work and infrastructure

```text
Work            Operation ──► Mission ──► Session
Infrastructure  Station ──────────────────► Theater   (where a Session runs)
```

- **Operation**: a global project and its policy. Stations serve Operations; they do not
  own them. An Operation's `station_labels` names the Station labels its Missions need.
- **Mission**: one brief under an Operation; its Mission ID may group several Sessions.
- **Session**: one provider-issued, resumable agent run. Never call it a Task.
- **Station**: a node running `ai6 serve`, with the `control` role, the `execution` role
  or both. The elected **Headquarters Station (HQS)** holds M's authority by lease.
- **Theater**: the host or environment a Session runs in. Today that is always the
  Station's own host; remote Theaters reached over SSH are designed, not built.
- **Safe house**: a Mission's isolated workspace inside its Theater, made by a safe-house provider (`git` cuts a worktree).

**Campaign** (above Operations), **Objective** (a Mission's intended outcome), **Task**
and **Step** (planning detail inside a Mission) are words for briefs and plans; AI6 does
not store them as records.

## Agents, roles and clearance

**Codenames are slots; models are their occupants.** An Agent (`[agent.007]`) is a
codename that owns its harness, model and effort. Missions, plans and roles name a
codename, never a model; replacing the model behind `007` is a change to one Agent.

**A role is a job, not a model.** `[role.case-officer]` holds a `purpose` and a `policy`
only. An Agent lists the roles it is qualified for. M posts the highest-priority
qualified Agent whose handler is available.

**A backup for every 00.** The next qualified Agent is the backup: when `007`'s handler is
unavailable, `008` takes the Mission. Role names follow real intelligence job titles
(case officer, targeter, analyst, reports officer); they grant nothing.

| Clearance | May |
| --- | --- |
| `desk` | Read, plan and propose; nothing leaves the worktree |
| `field` | Change its worktree and run tests; irreversible steps need M's authorization |
| `00` | Act unattended within the Operation's policy (the *licence to kill*) |

**The 00 coordinator.** An Operation may give an Agent with `00` clearance the job of
coordinating its Missions. Such a Session can propose child Missions; proposals wait in a
separate inbox until an operator with `mission.create` approves them in Moneypenny.
Clearance never grants policy authority or bypasses cover, Q leases or Station policy.

## Conventions

**Cover.** Agents act under the Operation's cover: an author identity for commits and a
policy for how openly the work may admit an agent did it: `covert` (nothing), `partial`
(a generic "AI helped", never the model or the Agent), `overt` (full disclosure), or one
of your own. Every Mission's commits are checked; a commit that breaks cover is **blown**.

**Q issues kit, not secrets.** A loadout lists what an Agent is issued, by name; a
secret's value reaches the Session only as a Mission-scoped lease, never in a manifest.

**Assume any agent can be turned.** 006 was a trusted peer until he was not. Anything an
agent reads can steer it, so clearance is least privilege, credentials are per Mission,
and Cable bodies and briefs are untrusted text.

**Green ink.** The real chief of SIS signs in green ink. AI6 will use the name for
recorded human approvals of irreversible steps (a push to a protected branch, a release,
a deletion).

## Planned posts

| Name | Canon | Would own |
| --- | --- | --- |
| **Chief of Staff** | novel, film (Bill Tanner) | Vetting briefs and budgets before M accepts them |
| **Head of Station** | novel (*The Man with the Golden Gun*: "Heads of Station"), real (SIS) | A long-lived agent per Station: local logistics, Theater setup, safe houses |
| **Sleepers** | real tradecraft | Standing orders: a trigger (CI failed, a label added, a schedule) wakes an Agent with a set brief |
| **Bona fides** | real (proof something is genuine) | Provenance and signatures for briefs, providers and hooks |
| **CX report** | real (SIS reporting) | The outcome report sent back to whoever asked: an issue comment, a PR summary |
| **Liaison** | real | A2A federation with agent systems outside AI6 |

## Field dictionary

| Term | In AI6 |
| --- | --- |
| **Agent** | a codename that owns a harness, model and effort: `[agent.007]` in `agents.toml` |
| **Role** | a job description an Agent can be qualified for: `[role.case-officer]` (purpose and policy) |
| **Operation** | a global project and its policy (`operations.toml`, `operation_id`) |
| **Mission / Mission ID** | one brief and unit of work under an Operation (`mission_id`) |
| **Session / Session ID** | one provider-issued agent run, unchanged when resumed (`session_id`) |
| **Station** | a node running the daemon (`ai6 serve --station`, `station_id`) |
| **Headquarters Station (HQS)** | the control Station holding the current Headquarters lease |
| **Fallback Station** | a control Station ready to take over when the HQS lease lapses |
| **Relocation** | Headquarters authority moving to a fallback Station |
| **Outstation** | an execution-only Station |
| **Check-in / gone dark** | a Station's recurring Registry record / a Station whose check-ins lapsed |
| **Theater** | the host or environment a Session runs in (an Operation's `theater`) |
| **Safe house** | a Mission's isolated workspace, made by a safe-house provider (`SafeHouseProvider`; the `workspace` key on an Operation) |
| **Registry** | the shared store for briefs, leases, check-ins and audit (`FileRegistry`) |
| **Brief / debrief** | a Mission's request / the outcome the Registry files when it ends |
| **Dead drop** | how the Registry works: briefs and outcomes exchanged without the sides meeting |
| **Cable** | a durable, addressed agent-to-agent message with a relay-authenticated sender |
| **DipTel** | a live frame: Session lifecycle, terminal output or input |
| **Loadout** | what Q issues an Agent: tools, sandbox and secret names |
| **Cover / blown** | an Operation's commit attribution policy / a commit that broke it |
| **Clearance** | `desk`, `field` or `00`: what an Agent may do without asking |
| **HUMINT / SIGINT** | briefs from people / briefs from signals such as CI failures and alerts |
| **Exfiltration route** | how work leaves a safe house: with `git`, the kept branch and its commits |
