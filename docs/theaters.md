# Theaters

A **Theater** is where a Session actually runs: the host or environment whose processes
and files the agent harness uses. A Station is the node that places and supervises the
Session; the Theater is the place it runs. They are separate on purpose, the way a
Kubernetes node is separate from the container runtime it drives.

## Today: the Station's own host

Every Session runs on its Station's host, in a terminal provider the Station picks
(preferring `pty`, or the one pinned with `ai6 serve --terminal`). An Operation with a local `path` gets a safe
house under the Station's state directory for each Mission, made by the safe-house provider
its `workspace` names (a Git worktree for `git`). An
Operation can only run on a Station that has its checkout and whose labels match its
`station_labels`.

## Not yet: remote Theaters

An Operation can name a `theater` instead of a local `path`, but the Station refuses to
admit its Missions: there is no remote terminal provider yet. Remote Theaters need:

- an SSH terminal provider with process-tree termination on the remote side;
- remote safe-house creation and cleanup;
- forwarding of the per-Session Cable MCP endpoint;
- a durable way for a Session to outlive a dropped connection, so a Station can
  reattach after it restarts.

Do not point a production Operation at a remote Theater and expect it to execute.
