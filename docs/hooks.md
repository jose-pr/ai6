# Hooks

Hooks run around every session: before it starts (add environment, add context, rewrite
the command line), on every state change, and once it ends. They are how a tool that
knows a directory — a dotfiles manager, a secrets helper, a context builder — reaches the
agent without coagent depending on it.

## Scopes

Read broadest first, so the narrowest wins:

| Scope | Where |
| --- | --- |
| headquarters | the `coagent.hooks` entry-point group, then `~/.config/coagent/hooks.toml` |
| station | `<station>/.coagent/hooks.toml`, for the station holding the agent's directory |
| agent | the profile's own environment and context, or hooks passed in code |

A project's `.coagent/` is read only when the project is a registered
[station](stations.md) (or with `COAGENT_TRUST_PROJECT=1`, for a checkout under
development): its hooks run inside the supervisor, which holds the credentials.

## `hooks.toml`

```toml
[[hook]]
name = "dir-env"
command = ["dotagents", "env"]
output = "env"            # K=V or export K=V lines

[[hook]]
name = "dir-context"
command = ["dotagents", "context"]
output = "context"        # text the agent is given

[[hook]]
name = "wrapper"
command = ["./wrap.py"]   # relative to this file
output = "json"           # {"env": {...}, "context": "...", "argv": [...]}
required = true           # a failure stops the mission
timeout = 30

[[hook]]
python = "mypackage.hooks:Audit"   # an object, or a class built with no arguments
```

A command hook runs in the agent's directory with `COAGENT_HOOK`, `COAGENT_AGENT`,
`COAGENT_RUN_DIR` and `COAGENT_CWD` set, and reads the agent's details as JSON on stdin.
A Python hook has any of `prepare(ctx)`, `on_event(event)` and `finished(ctx, outcome)`.

All the context collected reaches the agent as one file, named by `COAGENT_CONTEXT_FILE`.

```bash
python -m coagent_m.watchdog hooks     # what a run here would use
```
