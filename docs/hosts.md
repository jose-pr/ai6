# Remote hosts

A host is a theatre of operations: a machine missions run on, reached over ssh. M can
keep agents on Linux, macOS and Windows machines at once, from one place.

```toml
# ~/.config/coagent/hosts.toml
[host.lab]
ssh = "builder@lab.example.internal"   # anything ssh accepts as a destination
os = "linux"                           # linux, macos, windows
port = 22
identity = "~/.ssh/coagent_lab"
options = { ServerAliveInterval = "15" }
work_root = "/srv/coagent/worktrees"   # where mission worktrees go on the host
tags = ["gpu"]
```

Each host is a terminal provider, `ssh:<id>`:

- the agent runs through `ssh -tt` on the best local terminal, so the remote side gets a
  real pty (ConPTY on a Windows host) and the watchdog, the cursor handshake, hooks and
  transcripts work exactly as they do locally;
- only the environment M and its hooks **added** is forwarded — the supervisor's own
  environment stays on the supervisor's machine;
- on a POSIX host the remote command reports its session in an escape sequence no
  terminal shows, then becomes the agent, so `kill` ends every process in that session
  over a second connection. Dropping the connection alone would only send a hangup.

On a Windows host commands run, but a kill only drops the connection for now.

## A station on a host

```toml
[station.L]
name = "guatiao"
host = "lab"
path = "/srv/src/guatiao"          # the checkout, on the host
config = "~/coagent/stations/L"    # a local directory with the station's hooks and profiles
```

The station's missions run on the host whatever terminal their profile names. Worktrees
are cut on the host under its `work_root`, by git over ssh; without a `work_root`,
missions run in the checkout itself. Hooks run on the supervisor's machine and see
`COAGENT_HOST`; the agent sees it too.
