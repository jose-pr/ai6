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

## Safe houses

Each mission at a station gets its own safe house: a git worktree, cut from the
station's checkout at `base` onto a new branch named by `branch` (`{id}`, `{slug}`, `{station}`; default
`{id}-{slug}`). Safe houses live under M's state directory
(`$XDG_STATE_HOME/coagent/safe-houses/`), never inside the checkout. When the mission
ends, a safe house the agent left untouched is removed with its branch; one with commits
or changes is kept, and its path, branch and commits go into the mission's report. Set
`safe_houses = false` to run missions in the checkout itself.

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

## Cover

Universal Exports: what the work an agent leaves behind may say about who did it.

```toml
[station.G]
cover = "partial"                                            # covert (default), partial, overt, or your own
identity = { name = "Field Office", email = "office@example.com" }
on_blown = "fail"                                            # or "flag" (default): report only
```

M sets git's author and committer to the station's `identity` for the agent, tells the
agent its cover in its context (`COAGENT_COVER` names it), and after the mission checks
every commit it made — in its safe house, or in the checkout from the commit it started at.
The report carries `cover: {policy, commits, blown, violations}`; with `on_blown = "fail"`
a blown cover fails the mission (state `blown`).

| Cover | Author | Attribution trailers | The model's name | "Generated with" notices |
| --- | --- | --- | --- | --- |
| `covert` | the identity only | none | never | never |
| `partial` | the identity only | `Assisted-by` (suggested: `Assisted-by: AI`) | never | never |
| `overt` | anyone | any | allowed | allowed |

Define your own in `~/.config/coagent/covers.toml`, field by field:

```toml
[cover.house]
description = "AI help is acknowledged in a house footer, nothing more."
agent_author = false                 # the agent may not author or commit as itself
trailers = ["Assisted-by"]           # attribution trailers allowed; "*" for any
suggest = "Assisted-by: AI ({profile})"
model = false                        # may the model be named
announce = false                     # may messages say "generated with"
footer = "Universal Exports"         # every commit message must end with this
forbid = ['internal\.example\.com']   # patterns never allowed
```

Trailers that are about the change rather than its author (`Fixes`, `Refs`,
`Signed-off-by`) are never checked.
