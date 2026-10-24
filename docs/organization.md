# Organization

coagent is organized the way the fictional Secret Intelligence Service of the James Bond
novels and films is. The names are a convention, not decoration: each one says who owns
a decision, and the fiction's rules turn out to be good rules for running coding agents.

## Divisions

| Division | Here | Owns |
| --- | --- | --- |
| **M** (the Chief) | `m/` | policy, mission authorization, scheduling, stations, assigning agents; a role one office holds at a time, and another takes over |
| **Management offices** | `m/` (`serve`) | M's offices on many theaters: control (may be M), execution (run missions), or both |
| **Chief of Staff** | `m/` (planned) | vetting incoming briefs and budgets before M sees them |
| **Miss Moneypenny** | `moneypenny/` | information flow, the board, scheduling view, office security (who may attach) |
| **00 Section** | `00/` | the agents that execute work |
| **Q Branch** | `q/` | gear: credentials, sandboxes, tools that act |
| **R** (Q's deputy) | `q/` (planned) | acceptance of new gear before Q issues it |
| **I Branch** | `i/` | intelligence and communications: briefs, intake, the wire format, briefings, debriefs |
| **G** (Graphology) | `i/` (planned) | provenance: is a brief, plugin or hook file really from who it claims |
| **Medical & Psychological** | `m/` | agent health: the watchdog today; budgets, exposure and quarantine next |
| **Stations** | configuration | the projects missions are run for |

No division depends on another except on I Branch's formats.

## People and clearance

| Role | Clearance | Meaning here |
| --- | --- | --- |
| Desk officer | `desk` | reads, plans and proposes; nothing leaves its worktree |
| Case officer | `field` | changes its worktree and runs tests under *standard operational immunity*; anything irreversible needs M's authorization |
| 00 agent | `00` | the *licence to kill*: may act unattended, including irreversible steps |
| Station Chief | per station | an optional lead agent for one station (below) |
| Sleeper | any | a standing order, dormant until triggered (below) |

## Conventions

**Codenames are slots; models are their occupants.** A 00 number is recycled property
of the Crown: when an agent retires, the next qualified one inherits the number. Missions,
plans and roles name a codename (`007`), never a model. Replacing the model behind `007`
is a change to one profile.

**Promotion is earned and signed.** A profile is promoted to `00` only on a recorded
track record (missions completed at `field` without incident) and a human approval.

**Retirement.** A profile past its date (a model at end of life, or one that keeps
failing) is retired to desk duty: kept for reading and advice, never sent into the field.

**A backup for every 00.** M keeps a replacement in mind for each agent (008 for 007),
used when the first choice is unavailable, over budget or refused by the station.

**Green ink.** Orders from the Chief's office are written in green ink. Here that means
human approvals for irreversible steps (a push to a protected branch, a release, a
deletion), each kept as a signed record of who approved what, when, for which mission.

**Universal Exports.** Field agents travel as employees of a trading company. Agents
here act under the station's **cover**: an identity (the author their commits carry) and
a policy saying how openly the work may admit an agent did it — `covert` (nothing),
`partial` (a generic "AI helped", never the model or the agent), `overt` (full
disclosure), or one of your own. Every commit is checked against it; a commit that
breaks it is a **blown** cover. See [stations](stations.md#cover).

**The Leather Goods Department.** Q's shipments travel under a plain manifest: a loadout
lists what an agent is issued, by name, and never contains a secret's value.

**Assume any agent can be turned.** 006 was a trusted peer until he was not. Any agent
can be steered by what it reads (prompt injection), so clearance is least privilege,
credentials are per station, and a session that read untrusted input is treated as
exposed.

## Stations

A station is a project, by id (`G`, `GT`, `B2`: a letter is the convention, not a
limit), which is its one cover name, and by project name. It holds the project's checkout, the agents posted
there, the highest clearance a mission there may use, its budget, and settings for
intake sources. Registering a project as a station is what makes its own `.coagent/`
configuration readable.

**Station Chief.** An optional lead agent per station: it knows the station's repos and
CI, coordinates with friendly agencies (other bots and services on the project), breaks
missions into tasks, and provides visiting 00 agents with safe houses (worktrees) and
logistics (environment). M still authorizes and schedules.

**Sleepers.** A standing order at a station: a trigger (CI turned red, a label added, a
schedule) wakes a named profile with a set brief. Sleepers are delivered through I
Branch's intake like any other brief.

## Medical & Psychological Section

Agents wear out in recognisable ways, and the section's job is to notice:

- **stalls and loops**: the watchdog kills them today;
- **exhaustion**: a context window or token budget running out;
- **burnout**: a profile failing repeatedly, quarantined until a person looks;
- **exposure**: a session that read untrusted input (an issue body, a web page) is
  marked, and what it may do next is restricted;
- **debriefs**: every mission ends with a record I Branch keeps.

## Field dictionary

The code uses plain names; these are what they stand for.

| Term | In coagent |
| --- | --- |
| **Briefing room** | where a mission's parameters are handed over: the brief, plus the context hooks inject (`COAGENT_CONTEXT_FILE`) |
| **Theatre of operations** | the theater a mission runs on: this machine, or one registered in `theaters.toml` |
| **Drop zone** | where an agent is inserted: the theater, the path and the terminal it is started on |
| **Safe house** | the mission's git worktree: deniable, disposable, removed when the agent left nothing |
| **Dead drop** | the inbox: briefs and outcomes exchanged without the two sides ever meeting |
| **Live drop** | handing a brief straight in (`submit -`, `launch --brief`) |
| **Exfiltration route** | how work leaves the safe house: the kept branch and its commits, named in the report |
| **Going rogue / off-grid** | a session M has lost contact with (a dropped connection) or an agent acting outside its clearance |
| **Burned / blown** | credentials or a cover identity that leaked: Q revokes them and issues new ones |
| **HUMINT** | briefs written by people: tickets, requests |
| **SIGINT** | signals turned into briefs: CI failures, alerts, webhooks |
| **Bespoke assets** | tools issued to one agent for one mission |
| **The Quartermaster's inventory** | Q's record of what was issued to whom |
| **The Managing Director's office** | headquarters configuration (`~/.config/coagent/`) |
| **Blades** | the informal channel where operators talk to M |
