# Q

Q Branch of [C.O.A.G.E.N.T.](https://github.com/jose-pr/coagent): **the gear issued
for a mission.** At spawn, M asks Q for a **loadout** and passes it to the terminal
provider. Q is not on the hot path after that.

Q equips; I informs. Q owns the tools that *change* things (shell, file writes, git,
deploy) and the credentials and sandboxes they need. Sources that only *inform* (docs,
search, a code index, tracker reads) are I Branch's. A loadout is the one place that
issues both to an agent. Credentials are issued per station: one station's secrets
never reach another station's mission, and a profile's clearance bounds what it gets.

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
