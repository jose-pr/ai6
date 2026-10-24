# Stations

A station is a project M runs missions for, registered in the global `stations.toml`:

```toml
[station.G]
name = "guatiao"
path = "~/devel/guatiao"
repos = ["https://github.com/Boriken-Dev/guatiao"]   # a station may span many repos
roster = ["007", "006"]                              # absent: every profile
clearance = "field"                                  # the highest a mission here may use
max_concurrent = 3
intake = { jira = "GUA" }                            # for intake sources; M ignores it
```

The id is any short code (a letter is the convention, not the limit), and a station
answers to its id and to its project name. Registering a project is what makes its
`.coagent/` (its hooks and profiles) readable: a directory no station holds is never
read. Configuration runs headquarters → station → agent, the narrowest winning.

## Clearance at a station

A station's `clearance` is the highest a mission there may use, and its `roster`, when
set, is the only profiles it takes. `launch --role` and `launch --next` pick the best
profile the station accepts; naming a profile the station refuses is an error that says
why. See [agents](agents.md).

## Worktrees

Each mission at a station gets its own git worktree, cut from the station's checkout at
`base` onto a new branch named by `branch` (`{id}`, `{slug}`, `{station}`; default
`{id}-{slug}`). Worktrees live under M's state directory
(`$XDG_STATE_HOME/coagent/worktrees/`), never inside the checkout. When the mission
ends, a worktree the agent left untouched is removed with its branch; one with commits
or changes is kept, and its path, branch and commits go into the mission's report. Set
`worktrees = false` to run missions in the checkout itself.

```toml
[station.G]
path = "~/devel/guatiao"
branch = "agent/{id}-{slug}"
base = "origin/main"
```

## Commands

```bash
python -m coagent_m.watchdog stations            # what is registered
python -m coagent_m.watchdog launch --station G --role implement "fix the build"
```
