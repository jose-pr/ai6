# coagent-m

**M**, the supervisor daemon of [C.O.A.G.E.N.T.](https://github.com/jose-pr/coagent):
it takes tasks in, gives each sub-agent a git worktree and a sandbox, and **kills any
agent that hangs at a prompt or loops**.

Implemented:

| Module | Purpose |
| --- | --- |
| `coagent_m.watchdog` | runs one agent on a terminal and kills it, with every process it started, on a stall or a loop |
| `coagent_m.terminal` | terminal providers: `posix-pty`, `pipes`, `conpty` (scaffold), entry points, guatiao plugins |
| `coagent_m.hooks` | global, project and agent hooks: environment, context, command line, events |
| `coagent_m.profiles` | launchers and agent profiles: which model configuration runs which task |
| `coagent_m.config` | the global config directory, project discovery and trust |

The contract is in
[`src/coagent_m/AGENTS.md`](https://github.com/jose-pr/coagent/blob/main/m/src/coagent_m/AGENTS.md).

Planned, in this package:

| Module | Purpose |
| --- | --- |
| `coagent_m.intake` | Jira JQL polling and webhook receiver → tasks |
| `coagent_m.worktree` | `git worktree add` per task on its own branch; cleanup on exit |
| `coagent_m.bus` | the frame stream Moneypenny's gateway reads |
| `coagent_m.daemon` | the service entry point and scheduler, honouring `max_concurrent` |

```bash
python3 -m venv .pyvenv/local && .pyvenv/local/bin/pip install -e ".[dev]"
.pyvenv/local/bin/pytest -q
.pyvenv/local/bin/python -m coagent_m.watchdog demo --timeout 3
```
