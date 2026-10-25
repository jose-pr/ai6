# Theaters

A **Theater** is where a Session actually runs: the host or environment whose processes
and files the agent harness uses. A Station is the node that places and supervises the
Session; the Theater is the place it runs. They are separate on purpose, the way a
Kubernetes node is separate from the container runtime it drives.

## Declaring Theaters

A Station reaches its Theaters through its own `theaters.toml` in the configuration
directory:

```toml
[theater.build-box]
access = { ssh = { host = "build.example.net", user = "ci", port = 22, identity = "~/.ssh/ci_ed25519" } }
labels = { os = "linux", gpu = "yes" }
```

`access` is `"local"` or `{ ssh = { host, user, port, identity } }`; `user`, `port` and
`identity` are optional and left to the SSH client when absent. A Station's own host is
always reachable as the Theater `local` without any configuration; declaring
`[theater.local]` only adds labels to it. A Station publishes the Theaters it reaches at
each check-in: ID, access (`local` or `ssh`), the SSH host and labels. The SSH user, port
and key path stay on the Station. `GET /v1/stations` and Moneypenny's Stations view list
them.

A Session records the Station that supervises it and the Theater its harness runs in as two
separate values (`station_id`, `theater_id`); Moneypenny shows both on the Mission detail.
Records written before this was tracked have neither and still load.

## Where Sessions run today

Every Session runs on its Station's own host (the `local` Theater, or another Theater the
Station reaches with `access = "local"`), in a terminal provider the Station picks
(preferring `pty`, or the one pinned with `ai6 serve --terminal`). An Operation with a local `path` gets a safe
house under the Station's state directory for each Mission, made by the safe-house provider
its `workspace` names (a Git worktree for `git`). An Operation can only run on a Station
that has its checkout and whose labels match its `station_labels`. An Operation's `theater`
binds it to one Theater by ID. See [Placement](stations.md#placement) for how the Station,
Theater and Agent are chosen and how to ask why.

## Not yet: SSH Theaters

An `ssh` Theater is recorded and shown, but placement never chooses it, and its placement
preview says so. The harness must run on the Theater under a supervisor that survives
losing the Station's SSH link, and no such runner exists yet. SSH Theaters need:

- an SSH terminal provider with process-tree termination on the remote side;
- remote safe-house creation and cleanup;
- forwarding of the per-Session Cable MCP endpoint;
- a durable way for a Session to outlive a dropped connection, so a Station can
  reattach after it restarts.

Do not bind a production Operation to an SSH Theater and expect it to execute.
