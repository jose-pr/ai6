# coagent

**C.O.A.G.E.N.T.** — **C**ontrol plane for **O**rchestrated, **A**utonomous **G**lobal
**E**xecution & **N**etwork **T**erminals.

A **self-hosted supervisor for fleets of coding agents, where every agent is a real
terminal a person can step into.** Missions arrive from whatever you track work in, M
assigns each to the right agent for the project it belongs to, gives it its own git
worktree, and kills it the moment it hangs at a prompt or loops, with every process it
started. Nothing waits forever on a `[y/N]` nobody will see.

## Installation

From a checkout, with Python 3.9 or newer:

```bash
git clone https://github.com/jose-pr/coagent.git && cd coagent
python3 -m venv .pyvenv/local
.pyvenv/local/bin/pip install -e "m[dev]" -e "i[dev]"
```

| Extra/flag | Adds | Needed for |
| --- | --- | --- |
| `coagent-m[guatiao]` | `guatiao` (pinned to a commit) | terminal plugins in any language, such as the Rust `portable-pty` one |
| `coagent-m[conpty]` | `pywinpty` (Windows) | the Windows pseudo-console provider, once implemented |
| `coagent-i` | — | missions: briefs, the inbox, intake sources, `launch --next` |

## 30-second tour

Three mock agents — healthy, looping, hung at a prompt — with a 3-second window standing
in for the 300-second default:

```bash
python -m coagent_m.watchdog demo --timeout 3
```

Register a project as a station and give M an agent for it:

```toml
# ~/.config/coagent/stations.toml
[station.G]
name = "guatiao"
path = "~/devel/guatiao"

# ~/.config/coagent/profiles.toml
[launcher.cli]
command = ["agent-cli", "--model", "{model}"]

[profile.007]
aliases = ["JamesBond"]
launcher = "cli"
model = "<model-id>"
roles = ["implement"]
```

Hand M a mission from anything that can write JSON, and let her take it:

```bash
echo '{"id": "GUA-12", "title": "Fix the flaky test", "station": "G", "role": "implement"}' \
  | python -m coagent_i submit
python -m coagent_m.watchdog launch --next
```

## Learn more

- [Architecture](architecture.md): the five divisions and how they stay apart.
- [Organization](organization.md): the conventions the names carry.
- Guides: [stations](stations.md), [missions and intake](missions.md),
  [agents](agents.md), [hooks](hooks.md), [terminals](terminals.md),
  [the watchdog](watchdog.md).
- API reference: [M](api/m.md), [I Branch](api/i.md), and the Rust crates.
- [Changelog](changelog.md)
