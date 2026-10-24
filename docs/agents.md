# Agents, profiles and clearance

The 00 Section is the agents that execute. M does not hard-code which agent runs a
mission: a **profile** names a model configuration under a codename, and M assigns
profiles to missions.

## Launchers and profiles

A **launcher** says how to start one agent CLI. A **profile** fills it in.

```toml
# ~/.config/coagent/profiles.toml (or a station's .coagent/profiles.toml)
[launcher.cli]
command = ["agent-cli", "--model", "{model}"]
prompt = "arg"                     # arg appends it, stdin types it, none drops it

[profile.007]
aliases = ["JamesBond"]
description = "Plans and reviews; cleared to act unattended."
launcher = "cli"
model = "<model-id>"
effort = "high"
roles = ["plan", "review"]
tags = ["security"]
clearance = "00"
priority = 10
env = { AGENT_LOG = "debug" }
args = ["--verbose"]
terminal = "posix-pty"
limits = { stall_timeout = 600 }
context = "You review for security first."

[profile.006]
aliases = ["Trevelyan"]
extends = "007"
effort = "medium"
roles = ["implement"]
clearance = "field"
```

- Placeholders in a launcher's command: `{model}`, `{effort}`, `{profile}`, `{prompt}`,
  `{cwd}` and a profile's `vars`.
- `extends` inherits every unset field; `env`, `vars` and `limits` merge; aliases are
  never inherited.
- `roles` match by prefix: a profile listing `review` suits `review/security`.
- The agent gets `COAGENT_PROFILE`, `COAGENT_MODEL`, `COAGENT_EFFORT`, `COAGENT_STATION`
  and, when there is context, `COAGENT_CONTEXT_FILE`.
- An unknown key in a profile is an error, never a silently missing setting.

**Codenames are slots; models are their occupants.** Missions and roles name `007`,
never a model, so swapping the model behind `007` changes one profile.

## Adapters

A launcher can name an **adapter** instead of a command: a 00 Section package that knows
one agent CLI's flags, where its context goes, how clearance and cover map onto its own
settings, and how to read its transcript. `coagent-00` ships `claude`, for Claude Code:

```toml
[launcher.claude]
adapter = "claude"
options = { mode = "print" }          # print (headless, stream-json) or interactive

[profile.007]
launcher = "claude"
model = "opus"
effort = "high"
clearance = "00"
```

| The profile's | becomes |
| --- | --- |
| clearance `desk` / `field` / `00` | `--permission-mode plan` / `acceptEdits` (and `git push` denied) / `bypassPermissions` |
| station cover `covert` / `partial` / `overt` | Claude Code's `attribution` setting: hidden / the cover's suggested trailer / its default |
| `model`, `effort` | `--model`, `--effort` |
| the gathered context | `--append-system-prompt` |

In print mode a mission's report gains a `debrief` from Claude Code's final `result`
record: session id, turns, duration, cost and how many permissions it was denied.
Adapters for other CLIs plug in through the `coagent.adapters` entry-point group.

## Clearance

| Clearance | May |
| --- | --- |
| `desk` | read, plan and propose; nothing leaves the worktree |
| `field` | change its worktree and run its tests; anything irreversible needs M's authorization |
| `00` | act unattended, including irreversible steps such as a push or a merge |

A station refuses a profile cleared above it, and one not on its roster. What each
clearance permits inside the agent is the adapter's job (the CLI's own permission mode);
which credentials it is issued is Q Branch's.

## Commands

```bash
python -m coagent_m.watchdog profiles
python -m coagent_m.watchdog launch --profile JamesBond "plan the intake module"
python -m coagent_m.watchdog launch --role review/security --station G "audit the gateway"
```

`00/roster.example.toml` in the repository is a starting roster.
