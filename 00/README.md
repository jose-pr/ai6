# 00 Section

The field agents of [C.O.A.G.E.N.T.](https://github.com/jose-pr/coagent): **the coding
agents that actually execute the work, and how to drive each one.**

Design only, except for what M already reads: a roster of profiles and launchers in
`profiles.toml` (see `coagent_m.profiles`). This directory will hold:

| Path | What |
| --- | --- |
| `roster.example.toml` | an example roster: launchers for agent CLIs and the codenamed profiles built on them |
| `adapters/` | one package per agent CLI: its flags, where its context goes, how it says "done" or "waiting for approval", and how to read its transcript |

## A profile is an agent on the roster

```toml
[profile.007]
aliases = ["JamesBond"]
launcher = "cli"
model = "<model-id>"
effort = "high"
roles = ["plan", "review"]
clearance = "00"
```

M assigns profiles to missions by role and tags, among those the mission's station
accepts. The model configurations are whatever you define; nothing here is built in.

## Clearance

| Clearance | May |
| --- | --- |
| `desk` | read, plan and propose; nothing leaves the worktree |
| `field` | change its worktree and run its tests; anything irreversible needs M's authorization |
| `00` | act unattended, including irreversible steps such as a push or a merge |

A station's `clearance` is the highest any mission there may use; M refuses a profile
cleared above it. What each clearance permits is enforced by the adapter (the agent's
own permission mode) and by Q (which credentials and sandbox it is issued).

## Adapters

An adapter turns a profile into a working agent session and reads the session back:

- the command line, from the profile's model, effort and arguments;
- the context: an instructions file, a system-prompt flag or stdin;
- the agent's own signals for finished and for waiting on approval, which tell the
  watchdog more than silence does;
- the transcript, for I's debriefs.

Until an adapter exists for a CLI, a launcher in `profiles.toml` is its adapter.
