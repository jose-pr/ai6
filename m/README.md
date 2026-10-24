# coagent-m

**M**, the supervisor daemon of [C.O.A.G.E.N.T.](https://github.com/jose-pr/coagent):
it takes tasks in, gives each sub-agent a git worktree and a sandbox, and **kills any
agent that hangs at a prompt or loops**.

Implemented today: `coagent_m.watchdog`, which runs one agent on a pseudo-terminal and
SIGKILLs its whole process group on a stall or a loop. Its contract is in
[`src/coagent_m/AGENTS.md`](https://github.com/jose-pr/coagent/blob/main/m/src/coagent_m/AGENTS.md).

Planned, in this package:

| Module | Purpose |
| --- | --- |
| `coagent_m.intake` | Jira JQL polling and webhook receiver → tasks |
| `coagent_m.worktree` | `git worktree add` per task on its own branch; cleanup on exit |
| `coagent_m.sessions` | session backends behind one interface: `pty`, `tmux`, `systemd` |
| `coagent_m.bus` | the Unix-socket frame stream Moneypenny's gateway reads |
| `coagent_m.daemon` | the service entry point, with a `duho` CLI |

```bash
python3 -m venv .pyvenv/local && .pyvenv/local/bin/pip install -e ".[dev]"
.pyvenv/local/bin/pytest -q
.pyvenv/local/bin/python -m coagent_m.watchdog demo --timeout 3
```
