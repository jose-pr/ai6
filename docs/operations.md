# Operations

An Operation is a registered project or workspace and its policy. Operations are global:
no Station owns one, and any Station whose labels match the Operation's `station_labels` (and
that has its checkout) can run its Missions. The configuration owns the checkout, Agent
roster, clearance ceiling, concurrency budget, workspace and placement. AI6 reads Operations from `operations.toml` in its configuration directory.
Only registered Operations may contribute a local `.ai6/agents.toml` override.

```toml
[operation.G]
name = "guatiao"
path = "~/projects/guatiao"
roster = ["007", "006"]
clearance = "field"
max_concurrent = 3
workspace = { provider = "git", base = "origin/main", branch = "agent/{id}-{slug}", isolate = true }
station_labels = { gpu = "yes", os = "linux" }
stations = ["lab-gpu", "hq-1"]
cover = "covert"
on_blown = "flag"
sources = { jira = "GUA" }
```

The Operation ID (`G`) and project name (`guatiao`) both resolve to the same registered
Operation. `path` is the local checkout. `theater` binds the Operation to one
[Theater](theaters.md) by ID; Sessions run on a Theater a Station reaches with local access,
and an SSH Theater is not chosen yet. `roster` limits which Agents may be assigned; when omitted or empty, every
configured Agent is eligible. `clearance` sets the highest authority available to its
Missions. `station_labels` is matched against the labels of an execution Station and its
Theater. `stations` optionally lists the Stations allowed to run the Operation's Missions;
empty or absent allows every Station. A Mission can narrow that list but never widen it.
See [Placement](stations.md#placement) for the full set of checks.

## Workspace

`workspace` says where a Mission's Agent works. A **safe-house provider** makes the
workspace: `git` cuts a worktree on a new branch under the AI6 state directory. An
untouched safe house is removed when the Mission ends; one with changes is retained and
recorded in the Mission report.

| Key | Default | Meaning |
| --- | --- | --- |
| `provider` | `git` when the path is a Git checkout, else none | The safe-house provider ID. With none, Missions work in `path` directly and cover is not checked |
| `source` | the Operation's `path` | Where the work comes from (a local path) |
| `base` | `HEAD` | Revision a safe house starts from |
| `branch` | `{id}-{slug}` | Branch name template; `{id}`, `{slug}` and `{operation}` are filled in |
| `isolate` | `true` | `false` works in `source` itself instead of a separate copy |

Two providers ship: `git` (a worktree on its own branch; commits are audited) and `copy`
(a plain copy of the directory, always kept, no history).

Providers are loaded by the Station that runs the Session (`ai6 serve --plugin-path`). A
provider that keeps no history makes the Mission's cover report say it was **not
checkable** instead of passing. The earlier keys `repos`, `safe_houses`, `branch` and
`base` are no longer accepted; loading fails and names the `workspace` field to use.

## Cover

Operations can set a cover policy and author identity for commits:

```toml
[operation.G]
cover = "partial" # covert, partial, overt or a configured policy
cover_identity = { name = "Field Office", email = "field@example.invalid" }
on_blown = "fail" # or "flag"
```

The identity is applied as the Git author and committer. AI6 checks Mission commits
against the configured cover policy and records any violations in the Mission report.

## Provider limits

When a provider refuses work until a quota resets (a usage or rate limit), the Operation
decides what happens to the Mission:

```toml
[operation.G]
on_limit = "wait"          # or "backup"
limit_fallback_secs = 1800 # how long to park when the provider gives no reset time
```

| Key | Default | Meaning |
| --- | --- | --- |
| `on_limit` | `wait` | `wait` parks the Mission and resumes the same provider Session when the quota resets. `backup` posts the next qualified Agent whose handler is different, as a new attempt in the same safe house, and parks only when there is none |
| `limit_fallback_secs` | `1800` | Seconds to park when the provider reports no reset time (or one already past). Must be above zero |

Both are overridable per Operation and an unknown `on_limit` value fails the load. See
[Recovery](missions.md#recovery).

## Sources

`sources` lists where an Operation's work comes from. Each key names a Source provider and
holds that provider's settings; the Station holding the Headquarters lease polls them and
queues what it finds. See [Sources](missions.md#sources).

```toml
[operation.G.sources.github]
repo = "jose-pr/ai6"
labels = ["ai6:ready"]
interval_secs = 120

[operation.G.sources.jira]
site = "https://example.atlassian.net"
project = "GUA"
email_env = "JIRA_EMAIL"
```

Credentials are never configured: a source names the environment variable that holds its
token (`token_env`), and the Station reads it when it polls. A bare string is shorthand for
the provider's main target, as in `sources = { jira = "GUA" }`.

## Sleepers

`sleepers` holds standing orders that queue a brief on a cron schedule, once an operator
authorizes them with `ai6 sleeper authorize`. See [Sleepers](missions.md#sleepers).

```toml
[operation.G.sleepers.nightly-deps]
schedule = "0 3 * * *"
title = "Update dependencies"
prompt = "Update the lockfile and run the tests."
```

Missions and provider Sessions are visible in the Moneypenny app and through `ai6ctl`. The Mission ID identifies
the work; the provider-issued Session ID identifies one resumable agent run. See
[Missions and intake](missions.md) and [Agents and clearance](agents.md).
