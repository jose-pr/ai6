# Operations

An Operation is a registered project or workspace and its policy. Operations are global:
no Station owns one, and any Station whose labels match the Operation's `station_labels` (and
that has its checkout) can run its Missions. The configuration owns the checkout, Agent
roster, clearance ceiling, concurrency budget, safe-house policy and placement. AI6 reads Operations from `operations.toml` in its configuration directory.
Only registered Operations may contribute a local `.ai6/agents.toml` override.

```toml
[operation.G]
name = "guatiao"
path = "~/projects/guatiao"
repos = ["https://github.com/Boriken-Dev/guatiao"]
roster = ["007", "006"]
clearance = "field"
max_concurrent = 3
safe_houses = true
branch = "agent/{id}-{slug}"
base = "origin/main"
station_labels = { gpu = "yes", os = "linux" }
cover = "covert"
on_blown = "flag"
sources = { jira = "GUA" }
```

The Operation ID (`G`) and project name (`guatiao`) both resolve to the same registered
Operation. `path` is the local checkout. `theater` names a remote execution environment instead;
remote Theaters are not implemented yet, so such an Operation is not admitted. `roster` limits which Agents may be assigned; when omitted or empty, every
configured Agent is eligible. `clearance` sets the highest authority available to its
Missions. `station_labels` is matched against the labels of an execution Station.

Each Mission can receive a dedicated Git worktree under the AI6 state directory. An
untouched worktree is removed when the Mission ends; a worktree with changes or commits
is retained and recorded in the Mission report. Set `safe_houses = false` to use the
registered checkout directly.

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

Missions and provider Sessions are visible in the Moneypenny app and through `ai6ctl`. The Mission ID identifies
the work; the provider-issued Session ID identifies one resumable agent run. See
[Missions and intake](missions.md) and [Agents and clearance](agents.md).
