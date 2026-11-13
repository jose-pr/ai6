# Organization

AI6 borrows its names from the Secret Intelligence Service of the James Bond novels and
films, and from real public intelligence vocabulary where Bond has no word. The names
are a convention, not decoration: each one says who owns a decision.

## Directorates and posts

| Name | Here | Owns |
| --- | --- | --- |
| **M** | `ai6::directorates::m` | Decisions: the roster, Agent selection, clearance, cover, the Mission lifecycle and the access kinds |
| **Registry** | `ai6::directorates::registry` | The shared record (briefs, leases, check-ins, audit, access tokens) and the Cable relay |
| **Q Branch** | `ai6::directorates::q` | Gear: terminals, loadouts, Mission-scoped secret leases, MCP configuration, sandbox arguments |
| **00 Section** | `ai6::directorates::section_00`, `crates/providers/agents/` | The AgentHandler contract and the Claude and Codex handlers |
| **Medical** | `ai6::directorates::medical` | Agent health ([Medical](medical.md)): the watchdog for stalls, loops and blocked prompts; token and cost budgets; exposure; quarantine |
| **Station** | `ai6::station`, `ai6 serve` | A node running the daemon: its server, provider host, safe houses, publisher and identity |
| **Universal Exports** | `crates/universal-exports` | The public front: the only address operators and clients reach |
| **Outer Office** | `clients/outer-office` | The operator app (short name OO): board, briefs, terminals, administration, audit |
| **Moneypenny** | `crates/universal-exports` (`moneypenny`) | The operator's AI assistant, at her desk in Outer Office |
| **DipTel** | `crates/ai6-diptel` | The wire contract for Run lifecycle and terminal frames |

Directorates decide; the Station is where their decisions run. In the novels Universal
Export is the plate on the door of the London headquarters, the one address outsiders
use, which is the gateway's job. Outer Office is the room outside M's office where
Moneypenny keeps her desk (*Thunderball*, the films, *Skyfall*; plain English for the
setting, not a verified Fleming phrase): operators work in the app and deal with her there.

## Work and infrastructure

```text
Work            Operation ──► Mission ──► Run
Infrastructure  Station ──────────────────► Theater   (where a Run works)
```

- **Operation**: a global project and its policy. Stations serve Operations; they do not
  own them. An Operation's `station_labels` names the Station labels its Missions need.
- **Mission**: one brief under an Operation; its Mission ID may group several Runs.
- **Run**: one agent run, under one UUID AI6 makes; a Mission or a Run is executed. Never a Task or a Session.
- **Station**: a node running `ai6 serve`, with the `control` role, the `execution` role
  or both. The elected **Headquarters Station (HQS)** holds M's authority by lease.
- **Theater**: the host or environment a Run works on, reached through a transport
  (`local`, `ssh`, ...). The harness runs on the Theater (field mode: under the Station on
  its own host, or under a runner or the detached wrapper elsewhere), or on the Station while
  acting on a remote Theater through the transport (remote-hands mode).
- **Safe house**: a Mission's isolated workspace inside its Theater, made by a safe-house provider (`git` cuts a worktree).

**Objective** (a Mission's intended outcome), **Task** and **Step** (planning detail
inside a Mission) are words for briefs and plans; AI6 does not store them as records.
Operations nest: [Subordinate Operations](operations.md#subordinate-operations).

## Agents, roles and clearance

**Codenames are slots; models are their occupants.** An Agent (`[agent.007]`) is a
codename that owns its harness, model and effort. Missions, plans and roles name a
codename, never a model; replacing the model behind `007` is a change to one Agent.

**A role is a job, not a model.** `[role.case-officer]` holds a `purpose` and a `policy`
only. An Agent lists the roles it is qualified for. M posts the highest-priority
qualified Agent whose handler is available.

**A backup for every 00.** The next qualified Agent is the backup: when `007`'s handler is
unavailable, `008` takes the Mission. Role names are real intelligence job titles; they
grant nothing.

| Clearance | May |
| --- | --- |
| `desk` | Read, plan and propose; nothing leaves the worktree |
| `field` | Change its worktree and run tests; irreversible steps need M's authorization |
| `00` | Act unattended within the Operation's policy (the *licence to kill*) |

**The 00 in charge.** An Operation may give a `00` Agent its [charge](charges.md), which answers
for its outcome: it decides Missions in [`verifying`](missions.md#verifying), takes the
Operation's Cables and proposes child Missions for an operator to approve. Clearance is
autonomy, never policy authority: it bypasses no cover, Q lease or Station policy;
[Compartments](compartments.md) are need-to-know.

## Conventions

**Cover.** Agents act under the Operation's cover: an author identity for commits and a
policy for how openly the work may admit an agent did it: `covert` (nothing), `partial`
(a generic "AI helped", never the model or the Agent), `overt` (full disclosure), or one
of your own. Every Mission's commits are checked; a commit that breaks cover is **blown**.

**Q issues kit, not secrets.** A loadout lists what an Agent is issued, by name; a
secret's value reaches the Run only as a Mission-scoped lease, never in a manifest.

**Assume any agent can be turned.** 006 was a trusted peer until he was not. Anything an
agent reads can steer it, so clearance is least privilege, credentials are per Mission,
and Cable bodies and briefs are untrusted text.

**Green ink.** The real chief of SIS signs in green ink. In AI6 it is a person's recorded
approval of one irreversible step (a push to a protected branch, a release, a deletion). A
step taken without it is refused by the harness gate where AI6 can
see it, and the credential it needs is leased only against an approval.

## Built from the same lore

| Name | Canon | Owns |
| --- | --- | --- |
| **Sleepers** | real tradecraft | Standing orders: a schedule wakes an Agent with a set brief, once each authorized by M (`operation.<id>.sleepers`) |
| **CX report** | real (SIS reporting) | The outcome report a Source receives when its Mission ends: an issue comment, a Jira transition |
| **Chief of Staff** | novel, film | The `chief-of-staff` role: reviews untrusted briefs before M places them, and can only hold them; the deterministic vetting before it is M's (`operation.<id>.vetting`) |
| **Chief of Station** | novel and real (SIS) | The Station's charge: keeps it fit, provides what 00s ask ([Stations](stations.md#chief-of-station)) |
| **Liaison** | real | A2A, as Cables ([Liaison](liaison.md)) |

## Planned posts

| Name | Canon | Would own |
| --- | --- | --- |
| **Bona fides** | real (proof something is genuine) | Provenance and signatures for briefs, providers and hooks |

## Field dictionary

| Term | In AI6 |
| --- | --- |
| **Agent** | a codename that owns a harness, model and effort: `[agent.007]` in `agents.toml` |
| **Role** | a job description an Agent can be qualified for: `[role.case-officer]` (purpose and policy) |
| **Chief (C)** | the built-in role that holds every action |
| **Controller** | the built-in admin role; at `operation:X`, that Operation's admin |
| **Operation** | a global project and its policy (`operations.toml`, `operation_id`) |
| **Subordinate Operation** | an Operation whose `parent` names another |
| **Mission / Mission ID** | one brief and unit of work under an Operation (`mission_id`) |
| **Run / Run ID** | an agent run of a Mission or a charge, one UUID AI6 makes (`id`; `run_id` elsewhere); a Mission starts its next Run, a Run starts or resumes: executed |
| **Conversation** | a chat with Moneypenny; the run behind it is a Run |
| **Charge** | what an Agent answers for (an Operation's `00`, a Station's Chief of Station, M's service); one standing Run ([Charges](charges.md)) |
| **Session** | only another protocol's word, at its wire (ACP `sessionId`, sign-in sessions, `Mcp-Session-Id`); never a Run |
| **Station** | a node running the daemon (`ai6 serve --station`, `station_id`) |
| **Headquarters Station (HQS)** | the control Station holding the current Headquarters lease |
| **Fallback Station** | a control Station ready to take over when the HQS lease lapses |
| **Relocation** | Headquarters authority moving to a fallback Station |
| **Outstation** | an execution-only Station |
| **Check-in / gone dark** | a Station's recurring Registry record / a Station whose check-ins lapsed |
| **Extraction** | carrying a Mission on when its Station or runner is lost: another Station adopts a runner's Run, or a lost one is retried with a hand-off (`recovery`) |
| **Theater** | the host or environment a Run works in (an Operation's `theater`) |
| **Safe house** | a Mission's isolated workspace, made by a safe-house provider |
| **Registry** | the shared store for briefs, leases, check-ins and audit (`RegistryBackend`: `FileRegistry` or `SqlRegistry`) |
| **Brief / debrief** | a Mission's request / the outcome the Registry files when it ends |
| **Dead drop** | how the Registry works: briefs and outcomes exchanged without the sides meeting |
| **Cable** | a durable, addressed agent-to-agent message with a relay-authenticated sender |
| **DipTel** | a live frame: Run lifecycle, terminal output or input |
| **Loadout** | what Q issues an Agent: tools, sandbox and secret names |
| **Cover / blown** | an Operation's commit attribution policy / a commit that broke it |
| **Clearance** | `desk`, `field` or `00`: what an Agent may do without asking |
| **Compartment** | need-to-know access to other Operations' work; one is **read in** to it and **read out** |
| **Green ink** | a person's recorded approval of one irreversible step an Agent asked to take |
| **Class authorisation** | a standing approval of a kind of step on matching targets, until it ends or is revoked |
| **HUMINT / SIGINT** | briefs from people / briefs from signals such as CI failures and alerts |
| **Exfiltration route** | how work leaves a safe house: with `git`, the kept branch and its commits |
