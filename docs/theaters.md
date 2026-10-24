# Remote theaters

A theater (of operations) is a machine missions run on, reached over ssh. M can
keep agents on Linux, macOS and Windows machines at once, from one place.

```toml
# ~/.config/coagent/theaters.toml
[theater.lab]
ssh = "builder@lab.example.internal"   # anything ssh accepts as a destination
os = "linux"                           # linux, macos, windows
port = 22
identity = "~/.ssh/coagent_lab"
options = { ServerAliveInterval = "15" }
safe_houses = "/srv/coagent/safe-houses"   # where mission safe houses go on the theater
tags = ["gpu"]
```

Each theater is a terminal provider, `ssh:<id>`:

- the agent runs through `ssh -tt` on the best local terminal, so the remote side gets a
  real pty (ConPTY on a Windows theater) and the watchdog, the cursor handshake, hooks and
  transcripts work exactly as they do locally;
- only the environment M and its hooks **added** is forwarded — the supervisor's own
  environment stays on the supervisor's machine;
- on a POSIX theater the remote command reports its session in an escape sequence no
  terminal shows, then becomes the agent, so `kill` ends every process in that session
  over a second connection. Dropping the connection alone would only send a hangup.

On a Windows theater commands run, but a kill only drops the connection for now.

## A station on a theater

```toml
[station.L]
name = "guatiao"
theater = "lab"
path = "/srv/src/guatiao"          # the checkout, on the theater
config = "~/coagent/stations/L"    # a local directory with the station's hooks and profiles
```

The station's missions run on the theater whatever terminal their profile names. Safe houses
are cut on the theater under its `safe_houses`, by git over ssh; without a `safe_houses`,
missions run in the checkout itself. Hooks run on the supervisor's machine and see
`COAGENT_THEATER`; the agent sees it too.
