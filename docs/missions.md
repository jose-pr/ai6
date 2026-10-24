# Missions and intake

coagent ships no tracker integration. Jira, GitHub issues, a webhook, a schedule: you
supply those, as a program in any language, a Python package, a skill or an overlay. I
Branch owns only the socket they plug into: a **brief** format, an **inbox**, the
**source** contract, and **reports** back.

```text
source ─► brief ─► inbox/new ─► M claims it ─► station ─► profile ─► agent runs
                                                                         │
           source ◄─ report ◄── inbox/done|failed ◄── outcome ◄──────────┘
```

## A brief

```json
{
  "id": "GUA-12",
  "source": "jira",
  "title": "Fix the flaky watchdog test",
  "body": "It fails one run in ten on macOS.",
  "station": "Kingston",
  "role": "implement",
  "tags": ["ci"],
  "priority": 5,
  "url": "https://example.atlassian.net/browse/GUA-12",
  "data": {"anything": "the source wants back"}
}
```

Only `title` is required. `station` is an id, name or alias; `profile` names an agent
outright, otherwise M picks the best one for `role` and `tags` that the station accepts.
Unknown keys are refused, so a typo is an error; extras go under `data`.

## Three ways in

| Way in | How |
| --- | --- |
| **Drop a brief** | write it into the inbox's `new/` (a temporary name, then a rename), or `python -m coagent_i submit brief.json` |
| **Be polled** | a `[[source]]` in `sources.toml` whose command prints briefs as JSON |
| **Be a package** | a Python object in the `coagent.intake` entry-point group |

```toml
# ~/.config/coagent/sources.toml
[[source]]
name = "jira"
command = ["jira-briefs", "--jql", "project = GUA AND labels = agent"]
report = ["jira-report"]          # the outcome as JSON on stdin
station = "G"                     # for briefs that name none
timeout = 60
env = { JIRA_SITE = "example.atlassian.net" }
```

A source's command prints a JSON list of briefs, an object with `briefs`, or one JSON
object per line. Its `report` command receives `{"brief": .., "outcome": ..}`, where the
outcome carries `state`, `returncode`, `reason`, `runtime`, `agent`, `profile`, `station`
and, for a worktree mission, the worktree's path, branch and commits.

## The inbox

A directory (default `~/.local/state/coagent/inbox`, or `COAGENT_INBOX`) with one
subdirectory per state: `new`, `claimed`, `done`, `failed`. Claiming is a rename, so two
supervisors never take the same brief; every brief is filed once per `source--id` key.

```bash
python -m coagent_i poll            # every source once
python -m coagent_i list            # waiting, best first
python -m coagent_i list --state done
```

## Running missions

```bash
python -m coagent_m.watchdog launch --next          # the best waiting brief
python -m coagent_m.watchdog launch --brief b.json  # one brief from a file
```

M resolves the station, picks the profile, cuts a worktree, runs the agent with the
brief's title and body as its prompt, files the outcome beside the brief, and reports it
to the brief's source. A brief no profile at its station suits is filed as `failed`
with state `refused`, and still reported.
