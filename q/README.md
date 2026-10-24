# Q

The armory of [C.O.A.G.E.N.T.](https://github.com/jose-pr/coagent): what an agent is
allowed to hold. At spawn, M asks Q for a **loadout** and passes it to the session
backend. Q is not on the hot path after that.

Design only; nothing here builds yet. A loadout is one TOML file:

```toml
[loadout.backend-fixer]
sandbox = "worktree-rw"            # a profile from [sandbox.*]
mcp = ["filesystem", "shell"]      # servers from [mcp.*]
secrets = ["github-pr", "jira-ro"] # names from the broker, never values

[sandbox.worktree-rw]
read_write = ["$WORKTREE"]
read_only = ["/usr", "/etc/ssl"]
network = ["api.github.com:443", "*.atlassian.net:443"]

[mcp.filesystem]
command = ["mcp-server-filesystem", "$WORKTREE"]

[mcp.shell]
command = ["mcp-server-shell"]
deny = ["git push --force", "rm -rf /"]
```

Planned, as the Python package `coagent_q`:

| Module | Purpose |
| --- | --- |
| `coagent_q.secrets` | broker: resolves secret names from a backend (OS keyring, Vault, SOPS file) to values with a TTL, scoped to one run, and revokes them when the run ends |
| `coagent_q.sandbox` | turns a profile into backend arguments: bubblewrap binds, `systemd-run` properties, network allow-lists |
| `coagent_q.mcp` | writes the agent's MCP client config for the servers its loadout names |
| `coagent_q.loadout` | parses and validates loadout files |

Rules: a secret value is never written to the worktree, the event log or the terminal
stream; an agent gets only what its loadout names.
